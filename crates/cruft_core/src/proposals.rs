//! Agent proposals: the AI never edits the scene directly in "propose" mode.
//! A proposal is a pending, *unapplied* change set shown as ghosts by the UI.
//! Applying it executes ONE undoable `CompositeCommand`; rejecting it never
//! touches the scene.
use crate::json::{component_from_json, entity_id, parse_entity, resolve_component};
use crate::snapshot::is_hierarchy;
use crate::{
    CommandError, CompositeCommand, DespawnEntity, EditorEvent, History, InsertComponent, Reparent,
    SetComponentField, SpawnEntity, failure, resolve_entity,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// One change, as sent by an agent (`propose_changes`) or built by the UI.
/// Entity IDs are decimal strings from `Entity::to_bits`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ProposedChange {
    /// `components` maps a type path (or short name, e.g. "Transform") to its JSON value.
    /// `primitive` ("cube", "sphere", "plane", "capsule", "cylinder") adds a mesh + material.
    Spawn {
        name: String,
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        components: Map<String, Value>,
        #[serde(default)]
        primitive: Option<String>,
        #[serde(default)]
        color: Option<[f32; 3]>,
    },
    /// Replaces a whole component (merged over its default), inserting it if missing.
    SetComponent {
        entity: String,
        component: String,
        value: Value,
    },
    /// Sets one reflected field (`translation.x`) from a RON value (`2.5`).
    SetField {
        entity: String,
        component: String,
        path: String,
        value: String,
    },
    Despawn {
        entity: String,
    },
    Reparent {
        entity: String,
        #[serde(default)]
        parent: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffOp {
    Add,
    Modify,
    Remove,
}

impl DiffOp {
    pub fn symbol(self) -> char {
        match self {
            DiffOp::Add => '+',
            DiffOp::Modify => '~',
            DiffOp::Remove => '-',
        }
    }
}

/// Box to draw as a ghost in the viewport (world space).
#[derive(Clone, Debug, PartialEq)]
pub struct GhostPreview {
    pub op: DiffOp,
    pub label: String,
    pub entity: Option<Entity>,
    pub transform: Transform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Applied,
    Rejected,
}

impl ProposalStatus {
    pub fn label(self) -> &'static str {
        match self {
            ProposalStatus::Pending => "en attente",
            ProposalStatus::Applied => "appliqué",
            ProposalStatus::Rejected => "rejeté",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Proposal {
    pub id: u64,
    pub title: String,
    pub source: String,
    pub changes: Vec<ProposedChange>,
    pub status: ProposalStatus,
    pub created: Instant,
}

/// Where agent write tools go. `Propose` is the default.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentMode {
    #[default]
    Propose,
    Direct,
}

#[derive(Resource, Default)]
pub struct Proposals {
    items: Vec<Proposal>,
    next_id: u64,
}

impl Proposals {
    pub const LIMIT: usize = 100;

    pub fn items(&self) -> &[Proposal] {
        &self.items
    }

    /// Most recent pending proposal (the one the floating card shows).
    pub fn pending(&self) -> Option<&Proposal> {
        self.items
            .iter()
            .rev()
            .find(|p| p.status == ProposalStatus::Pending)
    }

    pub fn get(&self, id: u64) -> Option<&Proposal> {
        self.items.iter().find(|p| p.id == id)
    }

    fn push(
        &mut self,
        title: String,
        source: String,
        changes: Vec<ProposedChange>,
        status: ProposalStatus,
    ) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.items.push(Proposal {
            id,
            title,
            source,
            changes,
            status,
            created: Instant::now(),
        });
        if self.items.len() > Self::LIMIT {
            self.items.remove(0);
        }
        id
    }

    fn take_pending(&mut self, id: Option<u64>) -> Result<&mut Proposal, CommandError> {
        let proposal = match id {
            Some(id) => self.items.iter_mut().find(|p| p.id == id),
            None => self
                .items
                .iter_mut()
                .rev()
                .find(|p| p.status == ProposalStatus::Pending),
        }
        .ok_or_else(|| failure("Aucune proposition en attente"))?;
        if proposal.status != ProposalStatus::Pending {
            return Err(failure(format!(
                "Proposition {} déjà {}",
                proposal.id,
                proposal.status.label()
            )));
        }
        Ok(proposal)
    }
}

/// One MCP tool call, for the Agent journal.
#[derive(Clone, Debug)]
pub struct ToolCall {
    pub tool: String,
    pub duration: Duration,
    pub ok: bool,
    pub at: Instant,
}

/// Live MCP activity, written by `cruft_mcp` from inside a Bevy system.
#[derive(Resource, Default)]
pub struct AgentActivity {
    pub calls: VecDeque<ToolCall>,
    pub client: Option<String>,
    pub server_running: bool,
    /// e.g. "http://127.0.0.1:7457/mcp" (+ " · stdio").
    pub endpoint: Option<String>,
    pub tool_count: usize,
}

impl AgentActivity {
    pub const LIMIT: usize = 200;

    pub fn record(&mut self, tool: &str, duration: Duration, ok: bool) {
        self.calls.push_back(ToolCall {
            tool: tool.to_owned(),
            duration,
            ok,
            at: Instant::now(),
        });
        if self.calls.len() > Self::LIMIT {
            self.calls.pop_front();
        }
    }

    /// A client is considered connected if it called a tool in the last 2 minutes.
    pub fn connected(&self) -> bool {
        self.calls
            .back()
            .is_some_and(|call| call.at.elapsed() < Duration::from_secs(120))
    }
}

fn entity_of(world: &World, id: &str) -> Result<Entity, CommandError> {
    let entity = resolve_entity(world, parse_entity(&Value::String(id.to_owned()))?);
    if world.get_entity(entity).is_err() {
        return Err(failure(format!("Entité introuvable : {id}")));
    }
    Ok(entity)
}

fn name_of(world: &World, entity: Entity) -> String {
    world
        .get::<Name>(entity)
        .map(|n| n.as_str().to_owned())
        .unwrap_or_else(|| format!("#{}", entity.index()))
}

fn spawn_transform(world: &World, components: &Map<String, Value>) -> Option<Transform> {
    let registry = world.resource::<AppTypeRegistry>().read();
    let (_, value) = components.iter().find(|(key, _)| {
        resolve_component(&registry, key)
            .is_ok_and(|r| r.type_id() == std::any::TypeId::of::<Transform>())
    })?;
    let registration = registry.get(std::any::TypeId::of::<Transform>())?;
    component_from_json(&registry, registration, value)
        .ok()
        .and_then(|value| value.downcast_ref::<Transform>().copied())
}

impl ProposedChange {
    /// One line of the journal diff.
    pub fn diff(&self, world: &World) -> (DiffOp, String) {
        let name = |id: &str| {
            entity_of(world, id)
                .map(|e| name_of(world, e))
                .unwrap_or_else(|_| format!("#{id}?"))
        };
        match self {
            ProposedChange::Spawn {
                name: new_name,
                components,
                primitive,
                ..
            } => {
                let mut text = new_name.clone();
                if let Some(primitive) = primitive {
                    text.push_str(&format!(" [{primitive}]"));
                }
                if let Some(t) = spawn_transform(world, components) {
                    text.push_str(&format!(
                        " ({:.1}, {:.1}, {:.1})",
                        t.translation.x, t.translation.y, t.translation.z
                    ));
                }
                (DiffOp::Add, text)
            }
            ProposedChange::SetComponent {
                entity, component, ..
            } => (DiffOp::Modify, format!("{}.{component}", name(entity))),
            ProposedChange::SetField {
                entity,
                component,
                path,
                value,
            } => {
                let short = component.rsplit("::").next().unwrap_or(component);
                (
                    DiffOp::Modify,
                    format!("{}.{short}.{path} = {value}", name(entity)),
                )
            }
            ProposedChange::Despawn { entity } => (DiffOp::Remove, name(entity)),
            ProposedChange::Reparent { entity, parent } => (
                DiffOp::Modify,
                format!(
                    "{} → {}",
                    name(entity),
                    parent
                        .as_deref()
                        .map(name)
                        .unwrap_or_else(|| "racine".into())
                ),
            ),
        }
    }

    /// Ghost to draw in the viewport for this change, if it has a location.
    pub fn ghost(&self, world: &World) -> Option<GhostPreview> {
        match self {
            ProposedChange::Spawn {
                name,
                parent,
                components,
                ..
            } => {
                let local = spawn_transform(world, components).unwrap_or_default();
                let parent_global = parent
                    .as_deref()
                    .and_then(|id| entity_of(world, id).ok())
                    .and_then(|p| world.get::<GlobalTransform>(p).copied())
                    .unwrap_or_default();
                Some(GhostPreview {
                    op: DiffOp::Add,
                    label: name.clone(),
                    entity: None,
                    transform: (parent_global * GlobalTransform::from(local)).compute_transform(),
                })
            }
            ProposedChange::SetComponent {
                entity,
                component,
                value,
            } => {
                let entity = entity_of(world, entity).ok()?;
                let registry = world.resource::<AppTypeRegistry>().read();
                let registration = resolve_component(&registry, component).ok()?;
                if registration.type_id() != std::any::TypeId::of::<Transform>() {
                    return None;
                }
                let local = *component_from_json(&registry, registration, value)
                    .ok()?
                    .downcast_ref::<Transform>()?;
                let parent_global = world
                    .get::<ChildOf>(entity)
                    .and_then(|c| world.get::<GlobalTransform>(c.parent()).copied())
                    .unwrap_or_default();
                Some(GhostPreview {
                    op: DiffOp::Modify,
                    label: name_of(world, entity),
                    entity: Some(entity),
                    transform: (parent_global * GlobalTransform::from(local)).compute_transform(),
                })
            }
            ProposedChange::Despawn { entity } => {
                let entity = entity_of(world, entity).ok()?;
                Some(GhostPreview {
                    op: DiffOp::Remove,
                    label: name_of(world, entity),
                    entity: Some(entity),
                    transform: world
                        .get::<GlobalTransform>(entity)
                        .map(GlobalTransform::compute_transform)
                        .unwrap_or_default(),
                })
            }
            _ => None,
        }
    }
}

/// Validates `changes` and converts them into one composite command.
pub fn build_command(
    world: &mut World,
    label: impl Into<String>,
    changes: &[ProposedChange],
) -> Result<CompositeCommand, CommandError> {
    let mut composite = CompositeCommand::new(label);
    if changes.is_empty() {
        return Err(failure("Aucune modification proposée"));
    }
    for change in changes {
        match change {
            ProposedChange::Spawn {
                name,
                parent,
                components,
                primitive,
                color,
            } => {
                let parent = parent
                    .as_deref()
                    .map(|id| entity_of(world, id))
                    .transpose()?;
                let mut spawn = SpawnEntity::new(name.clone()).with_parent(parent);
                {
                    let registry = world.resource::<AppTypeRegistry>().clone();
                    let registry = registry.read();
                    for (key, value) in components {
                        let registration = resolve_component(&registry, key)?;
                        if registration.type_id() == std::any::TypeId::of::<Name>() {
                            continue;
                        }
                        if is_hierarchy(registration.type_id()) {
                            return Err(failure("Utiliser `parent` pour la hiérarchie"));
                        }
                        spawn =
                            spawn.with_boxed(component_from_json(&registry, registration, value)?);
                    }
                    if !components.keys().any(|key| {
                        resolve_component(&registry, key)
                            .is_ok_and(|r| r.type_id() == std::any::TypeId::of::<Transform>())
                    }) {
                        spawn = spawn.with_component(Transform::default());
                    }
                }
                if let Some(primitive) = primitive {
                    let shape = crate::PrimitiveShape::parse(primitive)?;
                    let color = color.map_or(crate::Primitive::default().color, Vec3::from);
                    spawn = spawn.with_component(crate::Primitive::new(shape, color));
                }
                composite.push(spawn);
            }
            ProposedChange::SetComponent {
                entity,
                component,
                value,
            } => {
                let entity = entity_of(world, entity)?;
                let registry = world.resource::<AppTypeRegistry>().clone();
                let registry = registry.read();
                let registration = resolve_component(&registry, component)?;
                if is_hierarchy(registration.type_id()) {
                    return Err(failure("Utiliser reparent pour la hiérarchie"));
                }
                let value = component_from_json(&registry, registration, value)?;
                let present = registration
                    .data::<ReflectComponent>()
                    .is_some_and(|access| access.contains(world.entity(entity)));
                if present {
                    let ron = crate::json::reflect_to_ron(value.as_partial_reflect(), &registry)?;
                    composite.push(SetComponentField::new(
                        entity,
                        registration.type_info().type_path(),
                        "",
                        ron,
                    ));
                } else {
                    composite.push(InsertComponent::new(entity, value));
                }
            }
            ProposedChange::SetField {
                entity,
                component,
                path,
                value,
            } => {
                let entity = entity_of(world, entity)?;
                let type_path = {
                    let registry = world.resource::<AppTypeRegistry>().read();
                    resolve_component(&registry, component)?
                        .type_info()
                        .type_path()
                        .to_owned()
                };
                composite.push(SetComponentField::new(
                    entity,
                    type_path,
                    path.clone(),
                    value.clone(),
                ));
            }
            ProposedChange::Despawn { entity } => {
                composite.push(DespawnEntity::new(entity_of(world, entity)?));
            }
            ProposedChange::Reparent { entity, parent } => {
                let entity = entity_of(world, entity)?;
                let parent = parent
                    .as_deref()
                    .map(|id| entity_of(world, id))
                    .transpose()?;
                composite.push(Reparent::new(entity, parent));
            }
        }
    }
    Ok(composite)
}

/// Stores a pending proposal (validated, NOT applied). Returns its ID.
pub fn propose(
    world: &mut World,
    title: impl Into<String>,
    source: impl Into<String>,
    changes: Vec<ProposedChange>,
) -> Result<u64, CommandError> {
    let title = title.into();
    build_command(world, title.clone(), &changes)?;
    world.init_resource::<Proposals>();
    let count = changes.len();
    let id = world.resource_mut::<Proposals>().push(
        title.clone(),
        source.into(),
        changes,
        ProposalStatus::Pending,
    );
    world.trigger(EditorEvent::Toast(format!(
        "Assistant : proposition en attente — {title} ({count} changement{})",
        if count > 1 { "s" } else { "" }
    )));
    Ok(id)
}

/// Applies a pending proposal (latest if `id` is None) as ONE undoable command.
pub fn apply_proposal(world: &mut World, id: Option<u64>) -> Result<u64, CommandError> {
    world.init_resource::<Proposals>();
    let (id, title, changes) = {
        let mut proposals = world.resource_mut::<Proposals>();
        let proposal = proposals.take_pending(id)?;
        (
            proposal.id,
            proposal.title.clone(),
            proposal.changes.clone(),
        )
    };
    let command = build_command(world, format!("Assistant : {title}"), &changes)?;
    world.resource_mut::<History>().finish_group();
    History::execute(world, command)?;
    world.resource_mut::<History>().finish_group();
    if let Ok(proposal) = world.resource_mut::<Proposals>().take_pending(Some(id)) {
        proposal.status = ProposalStatus::Applied;
    }
    Ok(id)
}

/// Rejects a pending proposal. Never touches the scene.
pub fn reject_proposal(world: &mut World, id: Option<u64>) -> Result<u64, CommandError> {
    world.init_resource::<Proposals>();
    let (id, title) = {
        let mut proposals = world.resource_mut::<Proposals>();
        let proposal = proposals.take_pending(id)?;
        proposal.status = ProposalStatus::Rejected;
        (proposal.id, proposal.title.clone())
    };
    world.trigger(EditorEvent::Toast(format!(
        "Assistant : proposition rejetée — {title}"
    )));
    Ok(id)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SubmitOutcome {
    /// Stored as a pending proposal (ghosts), awaiting apply/reject.
    Proposed { proposal_id: u64 },
    /// Applied immediately as one undoable command (direct mode).
    Applied {
        proposal_id: u64,
        spawned: Vec<String>,
    },
}

/// Entry point for every agent write: honours `AgentMode`.
pub fn submit_changes(
    world: &mut World,
    title: impl Into<String>,
    source: impl Into<String>,
    changes: Vec<ProposedChange>,
) -> Result<SubmitOutcome, CommandError> {
    let mode = world
        .get_resource::<AgentMode>()
        .copied()
        .unwrap_or_default();
    match mode {
        AgentMode::Propose => Ok(SubmitOutcome::Proposed {
            proposal_id: propose(world, title, source, changes)?,
        }),
        AgentMode::Direct => {
            let title = title.into();
            let command = build_command(world, format!("Assistant : {title}"), &changes)?;
            let before: std::collections::HashSet<Entity> =
                world.iter_entities().map(|e| e.id()).collect();
            world.resource_mut::<History>().finish_group();
            History::execute(world, command)?;
            world.resource_mut::<History>().finish_group();
            let spawned = world
                .iter_entities()
                .filter(|e| e.contains::<crate::SceneEntity>() && !before.contains(&e.id()))
                .map(|e| entity_id(e.id()))
                .collect();
            world.init_resource::<Proposals>();
            let proposal_id = world.resource_mut::<Proposals>().push(
                title,
                source.into(),
                changes,
                ProposalStatus::Applied,
            );
            Ok(SubmitOutcome::Applied {
                proposal_id,
                spawned,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CruftCorePlugin;
    use serde_json::json;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(CruftCorePlugin);
        app
    }

    fn count_named(world: &mut World, name: &str) -> usize {
        world
            .query::<&Name>()
            .iter(world)
            .filter(|n| n.as_str() == name)
            .count()
    }

    #[test]
    fn propose_does_not_touch_scene_and_apply_is_one_undo_entry() {
        let mut app = app();
        let world = app.world_mut();
        let cube = world
            .spawn((crate::SceneEntity, Name::new("Cube"), Transform::default()))
            .id();
        let changes: Vec<ProposedChange> = serde_json::from_value(json!([
            {"op": "spawn", "name": "Cover_A", "components": {"Transform": {"translation": [3.0, 0.5, -2.5]}}},
            {"op": "spawn", "name": "Cover_B", "parent": entity_id(cube)},
            {"op": "set_field", "entity": entity_id(cube), "component": "Transform", "path": "translation.y", "value": "2.0"}
        ]))
        .unwrap();
        let id = propose(world, "Couverts", "test", changes).unwrap();
        assert_eq!(count_named(world, "Cover_A"), 0);
        assert_eq!(world.resource::<History>().undo_len(), 0);
        let pending = world.resource::<Proposals>().pending().unwrap().clone();
        assert_eq!(pending.id, id);
        let ghost = pending.changes[0].ghost(world).unwrap();
        assert_eq!(ghost.transform.translation, Vec3::new(3.0, 0.5, -2.5));
        assert_eq!(pending.changes[2].diff(world).0, DiffOp::Modify);

        apply_proposal(world, None).unwrap();
        assert_eq!(count_named(world, "Cover_A"), 1);
        assert_eq!(world.get::<Transform>(cube).unwrap().translation.y, 2.0);
        assert_eq!(world.resource::<History>().undo_len(), 1);
        assert_eq!(
            world.resource::<Proposals>().get(id).unwrap().status,
            ProposalStatus::Applied
        );
        assert!(apply_proposal(world, Some(id)).is_err());

        History::undo(world).unwrap();
        assert_eq!(count_named(world, "Cover_A"), 0);
        assert_eq!(count_named(world, "Cover_B"), 0);
        assert_eq!(world.get::<Transform>(cube).unwrap().translation.y, 0.0);
        History::redo(world).unwrap();
        assert_eq!(count_named(world, "Cover_B"), 1);
    }

    #[test]
    fn reject_leaves_scene_and_history_untouched() {
        let mut app = app();
        let world = app.world_mut();
        let changes = vec![ProposedChange::Spawn {
            name: "X".into(),
            parent: None,
            components: Map::new(),
            primitive: None,
            color: None,
        }];
        let id = propose(world, "x", "test", changes).unwrap();
        reject_proposal(world, None).unwrap();
        assert_eq!(count_named(world, "X"), 0);
        assert_eq!(world.resource::<History>().undo_len(), 0);
        assert_eq!(
            world.resource::<Proposals>().get(id).unwrap().status,
            ProposalStatus::Rejected
        );
        assert!(world.resource::<Proposals>().pending().is_none());
    }

    #[test]
    fn invalid_proposals_are_rejected_upfront_and_failed_apply_rolls_back() {
        let mut app = app();
        let world = app.world_mut();
        assert!(
            propose(
                world,
                "x",
                "t",
                vec![ProposedChange::Despawn {
                    entity: "999999".into()
                }]
            )
            .is_err()
        );
        assert!(world.resource::<Proposals>().items().is_empty());
        let cube = world
            .spawn((crate::SceneEntity, Name::new("Cube"), Transform::default()))
            .id();
        let changes = vec![
            ProposedChange::Spawn {
                name: "Y".into(),
                parent: None,
                components: Map::new(),
                primitive: None,
                color: None,
            },
            ProposedChange::SetField {
                entity: entity_id(cube),
                component: "Transform".into(),
                path: "nope".into(),
                value: "1.0".into(),
            },
        ];
        propose(world, "bad", "t", changes).unwrap();
        assert!(apply_proposal(world, None).is_err());
        assert_eq!(count_named(world, "Y"), 0);
        assert_eq!(world.resource::<History>().undo_len(), 0);
    }

    #[test]
    fn direct_mode_applies_and_set_component_inserts_missing_components() {
        let mut app = app();
        let world = app.world_mut();
        *world.resource_mut::<AgentMode>() = AgentMode::Direct;
        let entity = world.spawn((crate::SceneEntity, Name::new("Vide"))).id();
        let outcome = submit_changes(
            world,
            "t",
            "test",
            vec![ProposedChange::SetComponent {
                entity: entity_id(entity),
                component: "Transform".into(),
                value: json!({"scale": [2.0, 2.0, 2.0]}),
            }],
        )
        .unwrap();
        assert!(matches!(outcome, SubmitOutcome::Applied { .. }));
        assert_eq!(
            world.get::<Transform>(entity).unwrap().scale,
            Vec3::splat(2.0)
        );
        History::undo(world).unwrap();
        assert!(world.get::<Transform>(entity).is_none());
    }
}
