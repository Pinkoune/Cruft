//! Bevy side of the MCP bridge. Requests arrive through a bounded channel and
//! are executed by ONE exclusive Bevy system each frame. Tokio never touches the
//! World: it only sends a `Request` and awaits a oneshot reply with a timeout.
use base64::Engine;
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
};
use crossbeam_channel::{Receiver, Sender, bounded};
use cruft_core::{
    AgentActivity, AgentMode, EditorEvent, History, Proposals, ProposedChange,
    json::{entity_id, parse_entity, reflect_to_json, resolve_component},
    proposals::{apply_proposal, reject_proposal, submit_changes},
};
use cruft_scene::{AssetCatalog, PlayAction, request_play_action};
use serde_json::{Map, Value, json};
use std::path::{Component as PathComponent, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

pub type Reply = Result<Value, String>;

pub(crate) struct Request {
    pub tool: String,
    pub arguments: Value,
    pub client: Option<String>,
    pub sent: Instant,
    pub reply: oneshot::Sender<Reply>,
}

/// Cloneable handle used by the tokio side.
#[derive(Clone)]
pub struct McpClient {
    sender: Sender<Request>,
}

impl McpClient {
    pub async fn call(&self, tool: &str, arguments: Value, client: Option<String>) -> Reply {
        let (reply, receive) = oneshot::channel();
        self.sender
            .try_send(Request {
                tool: tool.to_owned(),
                arguments,
                client,
                sent: Instant::now(),
                reply,
            })
            .map_err(|_| "File de requêtes MCP pleine ou fermée".to_owned())?;
        tokio::time::timeout(Duration::from_secs(10), receive)
            .await
            .map_err(|_| "Bevy n'a pas répondu en 10 secondes".to_owned())?
            .map_err(|_| "Bevy s'est arrêté avant de répondre".to_owned())?
    }
}

#[derive(Resource)]
pub struct McpBridge {
    receiver: Receiver<Request>,
    pub client: McpClient,
}

impl Default for McpBridge {
    fn default() -> Self {
        let (sender, receiver) = bounded(128);
        Self {
            receiver,
            client: McpClient { sender },
        }
    }
}

/// Image captured by `screenshot_viewport`. The editor binary points it at the
/// viewport render target; `None` captures the whole window.
#[derive(Resource, Default)]
pub struct McpScreenshotTarget(pub Option<Handle<Image>>);

enum Outcome {
    Value(Value),
    Screenshot { full_window: bool },
}

pub(crate) fn process_requests(world: &mut World) {
    let requests: Vec<_> = world
        .resource::<McpBridge>()
        .receiver
        .try_iter()
        .take(32)
        .collect();
    for request in requests {
        // A disconnected/timed-out caller must not trigger a queued mutation.
        if request.reply.is_closed() {
            continue;
        }
        let started = Instant::now();
        let result = dispatch(world, &request.tool, &request.arguments);
        let ok = result.is_ok();
        {
            let mut activity = world.resource_mut::<AgentActivity>();
            if request.client.is_some() {
                activity.client.clone_from(&request.client);
            }
            activity.record(
                &request.tool,
                request.sent.elapsed().max(started.elapsed()),
                ok,
            );
        }
        match result {
            Ok(Outcome::Value(value)) => {
                let _ = request.reply.send(Ok(value));
            }
            Ok(Outcome::Screenshot { full_window }) => capture(world, request.reply, full_window),
            Err(error) => {
                let _ = request.reply.send(Err(error));
            }
        }
    }
}

fn capture(world: &mut World, reply: oneshot::Sender<Reply>, full_window: bool) {
    let screenshot = match world
        .get_resource::<McpScreenshotTarget>()
        .and_then(|target| target.0.clone())
        .filter(|_| !full_window)
    {
        Some(image) => Screenshot::image(image),
        None => Screenshot::primary_window(),
    };
    let reply = Mutex::new(Some(reply));
    world
        .spawn(screenshot)
        .observe(move |captured: On<ScreenshotCaptured>| {
            let Some(reply) = reply.lock().ok().and_then(|mut r| r.take()) else {
                return;
            };
            let _ = reply.send(encode_png(captured.image.clone()));
        });
}

fn encode_png(image: Image) -> Reply {
    let dynamic = image.try_into_dynamic().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    dynamic
        .to_rgba8()
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "png_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
    }))
}

fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a Value> {
    args.get(key).filter(|v| !v.is_null())
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    arg(args, key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Paramètre `{key}` (chaîne) requis"))
}

fn id_arg(args: &Value, key: &str) -> Result<String, String> {
    let value = arg(args, key).ok_or_else(|| format!("Paramètre `{key}` requis"))?;
    parse_entity(value)
        .map(entity_id)
        .map_err(|e| e.to_string())
}

fn entity_json(world: &World, entity: EntityRef) -> Value {
    json!({
        "id": entity_id(entity.id()),
        "name": entity.get::<Name>().map(Name::as_str).unwrap_or(""),
        "parent": entity.get::<ChildOf>().map(|c| entity_id(c.parent())),
        "children": entity.get::<Children>().map_or(0, |c| c.len()),
        "has_mesh": entity_has_short(world, entity, "Mesh3d"),
    })
}

fn entity_has_short(world: &World, entity: EntityRef, short: &str) -> bool {
    let registry = world.resource::<AppTypeRegistry>().read();
    resolve_component(&registry, short)
        .ok()
        .and_then(|r| r.data::<ReflectComponent>())
        .is_some_and(|access| access.contains(entity))
}

/// Restrict file tools to the assets folder.
fn asset_path(world: &World, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, PathComponent::Normal(_) | PathComponent::CurDir))
    {
        return Err("Chemin relatif au dossier assets/ attendu (sans ..)".into());
    }
    let root = world
        .get_resource::<AssetCatalog>()
        .map(|c| c.root().to_path_buf())
        .ok_or("Catalogue d'assets absent")?;
    Ok(root.join(path))
}

fn submit(
    world: &mut World,
    title: String,
    changes: Vec<ProposedChange>,
) -> Result<Outcome, String> {
    let outcome = submit_changes(world, title, "mcp", changes).map_err(|e| e.to_string())?;
    Ok(Outcome::Value(
        serde_json::to_value(outcome).map_err(|e| e.to_string())?,
    ))
}

fn dispatch(world: &mut World, tool: &str, args: &Value) -> Result<Outcome, String> {
    let value = match tool {
        "list_entities" | "find_entities" => {
            let query = arg(args, "query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            let component = arg(args, "component").and_then(Value::as_str);
            let access = match component {
                Some(name) => {
                    let registry = world.resource::<AppTypeRegistry>().read();
                    Some(
                        resolve_component(&registry, name)
                            .map_err(|e| e.to_string())?
                            .data::<ReflectComponent>()
                            .cloned()
                            .ok_or("Type non composant")?,
                    )
                }
                None => None,
            };
            let entities = cruft_scene::scene_entities(world);
            json!(
                entities
                    .into_iter()
                    .map(|id| world.entity(id))
                    .filter(|e| e
                        .get::<Name>()
                        .map(Name::as_str)
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query))
                    .filter(|e| access.as_ref().is_none_or(|a| a.contains(*e)))
                    .map(|e| entity_json(world, e))
                    .collect::<Vec<_>>()
            )
        }
        "get_entity" => {
            let entity = parse_entity(arg(args, "id").ok_or("Paramètre `id` requis")?)
                .map_err(|e| e.to_string())?;
            let entity = cruft_core::resolve_entity(world, entity);
            let entity_ref = world
                .get_entity(entity)
                .map_err(|_| "Entité introuvable".to_owned())?;
            let registry = world.resource::<AppTypeRegistry>().read();
            let mut components = Map::new();
            for component in entity_ref.archetype().components() {
                let Some(type_id) = world
                    .components()
                    .get_info(*component)
                    .and_then(|i| i.type_id())
                else {
                    continue;
                };
                let Some(registration) = registry.get(type_id) else {
                    continue;
                };
                let Some(access) = registration.data::<ReflectComponent>() else {
                    continue;
                };
                let Some(reflected) = access.reflect(entity_ref) else {
                    continue;
                };
                let value = reflect_to_json(reflected.as_partial_reflect(), &registry)
                    .unwrap_or_else(|_| json!("<non sérialisable>"));
                components.insert(
                    registration
                        .type_info()
                        .type_path_table()
                        .short_path()
                        .to_owned(),
                    value,
                );
            }
            let mut base = entity_json(world, entity_ref);
            base["components"] = Value::Object(components);
            base
        }
        "spawn_entity" => {
            let name = str_arg(args, "name")?.to_owned();
            let change = ProposedChange::Spawn {
                name: name.clone(),
                parent: arg(args, "parent")
                    .map(|_| id_arg(args, "parent"))
                    .transpose()?,
                components: arg(args, "components")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default(),
                primitive: arg(args, "primitive")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                color: arg(args, "color")
                    .map(|c| serde_json::from_value(c.clone()))
                    .transpose()
                    .map_err(|e| format!("color : {e}"))?,
            };
            return submit(world, format!("Créer {name}"), vec![change]);
        }
        "despawn_entity" => {
            let entity = id_arg(args, "id")?;
            return submit(
                world,
                "Supprimer une entité".into(),
                vec![ProposedChange::Despawn { entity }],
            );
        }
        "set_component" => {
            let component = str_arg(args, "component")?.to_owned();
            let change = ProposedChange::SetComponent {
                entity: id_arg(args, "id")?,
                component: component.clone(),
                value: arg(args, "value")
                    .cloned()
                    .ok_or("Paramètre `value` requis")?,
            };
            return submit(world, format!("Modifier {component}"), vec![change]);
        }
        "set_field" => {
            let component = str_arg(args, "component")?.to_owned();
            let path = str_arg(args, "path")?.to_owned();
            let change = ProposedChange::SetField {
                entity: id_arg(args, "id")?,
                component: component.clone(),
                path: path.clone(),
                value: str_arg(args, "value")?.to_owned(),
            };
            return submit(world, format!("Modifier {component}.{path}"), vec![change]);
        }
        "reparent" => {
            let change = ProposedChange::Reparent {
                entity: id_arg(args, "id")?,
                parent: arg(args, "parent")
                    .map(|_| id_arg(args, "parent"))
                    .transpose()?,
            };
            return submit(world, "Changer le parent".into(), vec![change]);
        }
        "propose_changes" => {
            let title = str_arg(args, "title")?.to_owned();
            let changes: Vec<ProposedChange> =
                serde_json::from_value(arg(args, "changes").cloned().ok_or("`changes` requis")?)
                    .map_err(|e| format!("changes : {e}"))?;
            return submit(world, title, changes);
        }
        "list_proposals" => {
            let mode = match *world.resource::<AgentMode>() {
                AgentMode::Propose => "propose",
                AgentMode::Direct => "direct",
            };
            let proposals = world.resource::<Proposals>();
            json!({
                "mode": mode,
                "proposals": proposals.items().iter().rev().take(20).map(|p| json!({
                    "id": p.id,
                    "title": p.title,
                    "status": p.status,
                    "changes": p.changes,
                })).collect::<Vec<_>>(),
            })
        }
        "apply_proposal" => {
            let id = arg(args, "id").and_then(Value::as_u64);
            json!({"applied": apply_proposal(world, id).map_err(|e| e.to_string())?})
        }
        "reject_proposal" => {
            let id = arg(args, "id").and_then(Value::as_u64);
            json!({"rejected": reject_proposal(world, id).map_err(|e| e.to_string())?})
        }
        "list_assets" => {
            let mut catalog = world.resource_mut::<AssetCatalog>();
            catalog.refresh();
            json!(
                catalog
                    .entries()
                    .iter()
                    .map(|a| json!({"path": a.path, "kind": a.kind.label()}))
                    .collect::<Vec<_>>()
            )
        }
        "load_scene" => {
            let path = asset_path(world, str_arg(args, "path")?)?;
            cruft_scene::load_scene_file(world, &path).map_err(|e| e.to_string())?;
            world.trigger(EditorEvent::Toast(format!(
                "Assistant : scène chargée ({})",
                path.display()
            )));
            json!({"loaded": path.display().to_string()})
        }
        "save_scene" => {
            let relative = str_arg(args, "path")?;
            let path = asset_path(world, relative)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let entities = cruft_scene::scene_entities(world);
            let count = entities.len();
            cruft_scene::save_scene_file(world, entities, &path).map_err(|e| e.to_string())?;
            world.trigger(EditorEvent::Toast(format!(
                "Assistant : scène enregistrée ({relative})"
            )));
            json!({"saved": path.display().to_string(), "entities": count})
        }
        "enter_play_mode" => {
            request_play_action(world, PlayAction::Play);
            world.trigger(EditorEvent::Toast(
                "Assistant : lancement du play mode".into(),
            ));
            json!({"requested": "play"})
        }
        "stop_play_mode" => {
            request_play_action(world, PlayAction::Stop);
            world.trigger(EditorEvent::Toast("Assistant : arrêt du play mode".into()));
            json!({"requested": "stop"})
        }
        "read_logs" => {
            let n = arg(args, "n")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .min(2000) as usize;
            let logs = world
                .get_resource::<cruft_core::console::ConsoleLog>()
                .map(|log| log.entries())
                .unwrap_or_default();
            json!(
                logs.iter()
                    .skip(logs.len().saturating_sub(n))
                    .map(|log| json!({"level": log.level.to_string(), "target": log.target, "message": log.message}))
                    .collect::<Vec<_>>()
            )
        }
        "screenshot_viewport" => {
            return Ok(Outcome::Screenshot {
                full_window: arg(args, "full_window")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            });
        }
        "undo" => {
            let label = world.resource::<History>().undo_label();
            let done = History::undo(world).map_err(|e| e.to_string())?;
            json!({"undone": done.then_some(label).flatten()})
        }
        "redo" => {
            let label = world.resource::<History>().redo_label();
            let done = History::redo(world).map_err(|e| e.to_string())?;
            json!({"redone": done.then_some(label).flatten()})
        }
        _ => return Err(format!("Outil inconnu : {tool}")),
    };
    Ok(Outcome::Value(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cruft_core::SceneEntity;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            cruft_core::CruftCorePlugin,
            crate::CruftMcpPlugin::headless(),
        ));
        app.insert_resource(AssetCatalog::new(
            std::env::temp_dir().join("cruft_mcp_test_assets"),
        ));
        app
    }

    fn call(app: &mut App, tool: &str, arguments: Value) -> Reply {
        let sender = app.world().resource::<McpBridge>().client.sender.clone();
        let (reply, mut receive) = oneshot::channel();
        sender
            .try_send(Request {
                tool: tool.into(),
                arguments,
                client: Some("test".into()),
                sent: Instant::now(),
                reply,
            })
            .unwrap();
        assert!(receive.try_recv().is_err(), "only Bevy services requests");
        app.update();
        receive.try_recv().expect("answered within one frame")
    }

    #[test]
    fn cancelled_requests_are_skipped() {
        let mut app = app();
        let sender = app.world().resource::<McpBridge>().client.sender.clone();
        let (reply, receive) = oneshot::channel();
        drop(receive);
        sender
            .try_send(Request {
                tool: "undo".into(),
                arguments: json!({}),
                client: None,
                sent: Instant::now(),
                reply,
            })
            .unwrap();
        app.update();
        assert!(app.world().resource::<McpBridge>().receiver.is_empty());
        assert!(app.world().resource::<AgentActivity>().calls.is_empty());
    }

    #[test]
    fn spawn_is_proposed_then_applied_then_undone() {
        let mut app = app();
        app.world_mut()
            .spawn((SceneEntity, Name::new("Cube"), Transform::default()));
        let listed = call(&mut app, "list_entities", json!({})).unwrap();
        assert_eq!(listed[0]["name"], "Cube");

        let result = call(
            &mut app,
            "spawn_entity",
            json!({"name": "Crate", "components": {"Transform": {"translation": [1, 0, 0]}}}),
        )
        .unwrap();
        assert_eq!(result["status"], "proposed");
        assert_eq!(
            call(&mut app, "find_entities", json!({"query": "crate"}))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            0
        );

        call(&mut app, "apply_proposal", json!({})).unwrap();
        let found = call(&mut app, "find_entities", json!({"query": "crate"})).unwrap();
        assert_eq!(found.as_array().unwrap().len(), 1);
        let entity = call(&mut app, "get_entity", json!({"id": found[0]["id"]})).unwrap();
        assert_eq!(entity["components"]["Transform"]["translation"][0], 1.0);

        assert!(call(&mut app, "undo", json!({})).unwrap()["undone"].is_string());
        assert_eq!(
            call(&mut app, "find_entities", json!({"query": "crate"}))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            app.world().resource::<AgentActivity>().client.as_deref(),
            Some("test")
        );
    }

    #[test]
    fn direct_mode_and_reject_and_errors() {
        let mut app = app();
        *app.world_mut().resource_mut::<AgentMode>() = AgentMode::Direct;
        let result = call(&mut app, "spawn_entity", json!({"name": "Now"})).unwrap();
        assert_eq!(result["status"], "applied");
        *app.world_mut().resource_mut::<AgentMode>() = AgentMode::Propose;
        call(
            &mut app,
            "propose_changes",
            json!({"title": "t", "changes": [{"op": "despawn", "entity": result["spawned"][0]}]}),
        )
        .unwrap();
        call(&mut app, "reject_proposal", json!({})).unwrap();
        assert_eq!(
            call(&mut app, "find_entities", json!({"query": "now"}))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(call(&mut app, "nope", json!({})).is_err());
        assert!(call(&mut app, "save_scene", json!({"path": "../evil.ron"})).is_err());
        assert!(call(&mut app, "despawn_entity", json!({"id": "123456789"})).is_err());
    }

    #[test]
    fn save_then_load_roundtrips_through_assets_folder() {
        let mut app = app();
        app.world_mut().spawn((
            SceneEntity,
            Name::new("Saved"),
            Transform::from_xyz(1.0, 2.0, 3.0),
        ));
        let saved = call(
            &mut app,
            "save_scene",
            json!({"path": "scenes/mcp_test.scn.ron"}),
        )
        .unwrap();
        assert_eq!(saved["entities"], 1);
        call(
            &mut app,
            "load_scene",
            json!({"path": "scenes/mcp_test.scn.ron"}),
        )
        .unwrap();
        app.update();
        assert_eq!(
            call(&mut app, "find_entities", json!({"query": "saved"}))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}
