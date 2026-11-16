//! In-memory scene snapshots retain strong asset handles (including procedural assets).
use bevy::{
    ecs::{
        component::{ComponentCloneBehavior, ComponentId},
        entity::{EntityHashMap, EntityMapper},
        resource::IS_RESOURCE,
    },
    prelude::*,
    reflect::ReflectFromReflect,
    world_serialization::WorldAssetRoot,
};
use cruft_core::{
    CommandError, EditorEvent, EditorOnly, EditorState, History, SceneEntity, Selection,
    remap_entity_ids,
};
use std::{
    any::TypeId,
    collections::{HashSet, VecDeque},
};

struct SavedComponent {
    access: ReflectComponent,
    value: Box<dyn Reflect>,
}
struct SavedEntity {
    id: Entity,
    ids: Vec<ComponentId>,
    components: Vec<SavedComponent>,
    children: Vec<Entity>,
    parent: Option<Entity>,
    loaded_instance: bool,
}

/// Snapshots registered reflected scene components, never editor resources.
/// Existing entities retain their IDs and unregistered engine caches on restore.
pub struct SceneSnapshot {
    entities: Vec<SavedEntity>,
    baseline: HashSet<Entity>,
}

impl SceneSnapshot {
    pub fn capture(world: &World) -> Result<Self, CommandError> {
        let registry = world
            .get_resource::<AppTypeRegistry>()
            .ok_or_else(|| CommandError("TypeRegistry absent".into()))?
            .read();
        let baseline = world.iter_entities().map(|e| e.id()).collect();
        let mut pending: Vec<_> = world
            .iter_entities()
            .filter(|e| e.contains::<SceneEntity>() && !e.contains::<EditorOnly>())
            .map(|e| e.id())
            .collect();
        let mut seen = HashSet::new();
        let mut entities = Vec::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let entity = world.entity(id);
            if entity.contains::<EditorOnly>() || entity.contains_id(IS_RESOURCE) {
                continue;
            }
            let children: Vec<_> = entity
                .get::<Children>()
                .map(|c| c.iter().collect())
                .unwrap_or_default();
            pending.extend(children.iter().copied());
            let ids = entity.archetype().components().to_vec();
            let mut components = Vec::new();
            for cid in &ids {
                let info = world.components().get_info(*cid).expect("live component");
                if matches!(info.clone_behavior(), ComponentCloneBehavior::Ignore) {
                    continue;
                }
                let Some(ty) = info.type_id() else {
                    continue;
                };
                if ty == TypeId::of::<ChildOf>() || ty == TypeId::of::<Children>() {
                    continue;
                }
                let Some(registration) = registry.get(ty) else {
                    continue;
                };
                let Some(access) = registration.data::<ReflectComponent>() else {
                    continue;
                };
                let Some(value) = access.reflect(entity) else {
                    continue;
                };
                let value = value
                    .reflect_clone()
                    .ok()
                    .or_else(|| {
                        registration
                            .data::<ReflectFromReflect>()
                            .and_then(|from| from.from_reflect(value.as_partial_reflect()))
                    })
                    .ok_or_else(|| {
                        CommandError(format!("Snapshot impossible : {}", info.name()))
                    })?;
                components.push(SavedComponent {
                    access: access.clone(),
                    value,
                });
            }
            entities.push(SavedEntity {
                id,
                ids,
                components,
                children,
                loaded_instance: cruft_core::instance_is_loaded(world, id),
                parent: entity.get::<ChildOf>().map(ChildOf::parent),
            });
        }
        Ok(Self { entities, baseline })
    }

    pub fn restore(mut self, world: &mut World) -> Result<(), CommandError> {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let mut map = EntityHashMap::default();
        for saved in &self.entities {
            let live = if world.get_entity(saved.id).is_ok() {
                saved.id
            } else {
                world.spawn_empty().id()
            };
            map.insert(saved.id, live);
        }
        // Detach before deleting runtime entities: a runtime parent can own an
        // original scene child. Never cascade-delete that child or editor helpers.
        let runtime: Vec<_> = world
            .iter_entities()
            .filter(|e| {
                !self.baseline.contains(&e.id())
                    && !map.values().any(|id| *id == e.id())
                    && !e.contains_id(IS_RESOURCE)
                    && !e.contains::<EditorOnly>()
            })
            .map(|e| e.id())
            .collect();
        let runtime_set: HashSet<_> = runtime.iter().copied().collect();
        let detach: Vec<_> = world
            .iter_entities()
            .filter(|e| {
                map.values().any(|id| *id == e.id())
                    || e.get::<ChildOf>()
                        .is_some_and(|p| runtime_set.contains(&p.parent()))
            })
            .map(|e| e.id())
            .collect();
        for id in detach {
            world.entity_mut(id).remove::<ChildOf>();
        }
        for id in runtime {
            world.despawn(id);
        }

        let mut mapper = RestoreMapper(&map);
        for saved in &mut self.entities {
            let live = map[&saved.id];
            let extra: Vec<_> = world
                .entity(live)
                .archetype()
                .components()
                .iter()
                .filter(|id| !saved.ids.contains(id))
                .copied()
                .collect();
            world.entity_mut(live).remove_by_ids(&extra);
            for component in &mut saved.components {
                component
                    .access
                    .map_entities(component.value.as_mut(), &mut mapper);
                // Unchanged roots must not trigger Bevy's async scene respawner.
                let unchanged_root = component.value.is::<WorldAssetRoot>()
                    && component
                        .access
                        .reflect(world.entity(live))
                        .is_some_and(|value| {
                            value.reflect_partial_eq(component.value.as_partial_reflect())
                                == Some(true)
                        });
                if !unchanged_root {
                    component.access.insert(
                        &mut world.entity_mut(live),
                        component.value.as_partial_reflect(),
                        &registry,
                    );
                }
            }
        }
        for saved in &self.entities {
            let children: Vec<_> = saved
                .children
                .iter()
                .filter_map(|id| map.get(id).copied())
                .collect();
            world.entity_mut(map[&saved.id]).add_children(&children);
            if let Some(parent) = saved.parent.filter(|p| !map.contains_key(p))
                && world.get_entity(parent).is_ok()
            {
                world.entity_mut(parent).add_child(map[&saved.id]);
            }
        }
        for saved in &self.entities {
            if saved.loaded_instance {
                cruft_core::preserve_restored_instance(world, map[&saved.id]);
            }
        }
        remap_entity_ids(world, &map)
    }
}

struct RestoreMapper<'a>(&'a EntityHashMap<Entity>);
impl EntityMapper for RestoreMapper<'_> {
    fn get_mapped(&mut self, source: Entity) -> Entity {
        self.0.get(&source).copied().unwrap_or(source)
    }
    fn set_mapped(&mut self, _source: Entity, _target: Entity) {}
}

#[derive(Clone, Copy, Debug)]
pub enum PlayAction {
    Play,
    Pause,
    Resume,
    Stop,
}

#[derive(Resource, Default)]
pub struct PlayMode {
    pending: VecDeque<PlayAction>,
    snapshot: Option<SceneSnapshot>,
    selection: Option<Selection>,
    history: Option<History>,
    time_was_paused: bool,
}

/// Queued until PreUpdate, before Bevy applies state transitions/gameplay.
pub fn request_play_action(world: &mut World, action: PlayAction) {
    world.resource_mut::<PlayMode>().pending.push_back(action);
}

pub(crate) fn process_play_actions(world: &mut World) {
    world.resource_scope(|world, mut mode: Mut<PlayMode>| {
        // One transition per frame: two rapid clicks cannot overwrite a pending state.
        let Some(action) = mode.pending.pop_front() else {
            return;
        };
        let state = *world.resource::<State<EditorState>>().get();
        let next = match (state, action) {
            (EditorState::Editing, PlayAction::Play) => {
                match SceneSnapshot::capture(world) {
                    Ok(snapshot) => mode.snapshot = Some(snapshot),
                    Err(error) => {
                        world.trigger(EditorEvent::Toast(error.to_string()));
                        return;
                    }
                }
                mode.selection = world.get_resource::<Selection>().cloned();
                mode.history = world.remove_resource::<History>();
                world.init_resource::<History>();
                mode.time_was_paused = world
                    .get_resource::<Time<Virtual>>()
                    .is_some_and(|t| t.is_paused());
                EditorState::Playing
            }
            (EditorState::Playing, PlayAction::Pause) => EditorState::Paused,
            (EditorState::Paused, PlayAction::Resume) => EditorState::Playing,
            (EditorState::Playing | EditorState::Paused, PlayAction::Stop) => EditorState::Editing,
            _ => return,
        };
        if let Some(mut time) = world.get_resource_mut::<Time<Virtual>>() {
            if next == EditorState::Paused || (next == EditorState::Editing && mode.time_was_paused)
            {
                time.pause();
            } else {
                time.unpause();
            }
        }
        world.resource_mut::<NextState<EditorState>>().set(next);
        let message = match next {
            EditorState::Editing => return, // OnEnter restores after gameplay OnExit hooks.
            EditorState::Playing => "Simulation en cours",
            EditorState::Paused => "Simulation en pause",
        };
        info!("{message}");
        world.trigger(EditorEvent::Toast(message.into()));
    });
}

pub(crate) fn restore_editing_scene(world: &mut World) {
    world.resource_scope(|world, mut mode: Mut<PlayMode>| {
        let Some(snapshot) = mode.snapshot.take() else {
            return;
        };
        if let Some(selection) = mode.selection.take() {
            world.insert_resource(selection);
        }
        if let Err(error) = snapshot.restore(world) {
            world.trigger(EditorEvent::Toast(error.to_string()));
            return;
        }
        if let Some(mut history) = mode.history.take() {
            history.finish_group();
            world.insert_resource(history);
        }
        info!("Mode jeu arrêté · scène restaurée");
        world.trigger(EditorEvent::Toast(
            "Mode jeu arrêté · scène restaurée".into(),
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CruftScenePlugin;
    use cruft_core::{CruftCorePlugin, Gameplay, SetComponentField, SpawnEntity, resolve_entity};

    #[derive(Component, Reflect, PartialEq, Debug)]
    #[reflect(Component)]
    struct Counter(u32);
    #[derive(Component, Reflect)]
    #[reflect(Component)]
    struct Link(#[entities] Entity);
    #[derive(Component)]
    struct RuntimeOnly;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, CruftCorePlugin, CruftScenePlugin))
            .register_type::<Counter>()
            .register_type::<Link>()
            .add_systems(
                Update,
                (|mut counters: Query<&mut Counter>| {
                    for mut counter in &mut counters {
                        counter.0 += 1;
                    }
                })
                .in_set(Gameplay),
            );
        app
    }
    fn action(app: &mut App, action: PlayAction) {
        request_play_action(app.world_mut(), action);
        app.update();
    }

    #[test]
    fn play_pause_resume_stop_restores_values_selection_and_history() {
        let mut app = app();
        History::execute(
            app.world_mut(),
            SpawnEntity::new("actor").with_component(Counter(10)),
        )
        .unwrap();
        let id = app
            .world_mut()
            .query_filtered::<Entity, With<Counter>>()
            .single(app.world())
            .unwrap();
        app.world_mut().resource_mut::<Selection>().select_only(id);
        app.update();
        assert_eq!(app.world().get::<Counter>(id).unwrap().0, 10);
        action(&mut app, PlayAction::Play);
        assert_eq!(app.world().get::<Counter>(id).unwrap().0, 11);
        assert!(
            History::execute(
                app.world_mut(),
                SetComponentField::typed::<Counter>(id, "0", "42")
            )
            .is_err()
        );
        action(&mut app, PlayAction::Pause);
        app.update();
        assert_eq!(app.world().get::<Counter>(id).unwrap().0, 11);
        assert!(app.world().resource::<Time<Virtual>>().is_paused());
        action(&mut app, PlayAction::Resume);
        assert_eq!(app.world().get::<Counter>(id).unwrap().0, 12);
        app.world_mut().resource_mut::<Selection>().clear();
        action(&mut app, PlayAction::Stop);
        assert_eq!(
            *app.world().resource::<State<EditorState>>().get(),
            EditorState::Editing
        );
        assert_eq!(app.world().get::<Counter>(id).unwrap().0, 10);
        assert_eq!(app.world().resource::<Selection>().primary(), Some(id));
        assert_eq!(app.world().resource::<History>().undo_len(), 1);
        assert!(History::undo(app.world_mut()).unwrap());
        assert!(app.world().get_entity(id).is_err());
        assert!(History::redo(app.world_mut()).unwrap());
        assert_eq!(
            app.world()
                .get::<Counter>(resolve_entity(app.world(), id))
                .unwrap()
                .0,
            10
        );
    }

    #[test]
    fn restores_deleted_entities_references_order_and_removed_components() {
        let mut app = app();
        let world = app.world_mut();
        let root = world.spawn((SceneEntity, Name::new("root"))).id();
        let first = world.spawn((Counter(3), ChildOf(root))).id();
        let second = world.spawn((Counter(4), Link(first), ChildOf(root))).id();
        world.resource_mut::<Selection>().select_only(first);
        History::execute(world, SetComponentField::typed::<Counter>(first, "0", "8")).unwrap();
        action(&mut app, PlayAction::Play);
        let world = app.world_mut();
        world.despawn(first);
        world
            .entity_mut(second)
            .remove::<Counter>()
            .insert(RuntimeOnly);
        let spawned = world.spawn(RuntimeOnly).id();
        let helper = world.spawn(EditorOnly).id();
        world.entity_mut(spawned).add_child(second);
        action(&mut app, PlayAction::Stop);
        let world = app.world_mut();
        let restored = resolve_entity(world, first);
        assert_ne!(restored, first);
        assert_eq!(world.get::<Counter>(restored).unwrap().0, 8);
        assert_eq!(world.get::<Counter>(second).unwrap().0, 4);
        assert_eq!(world.get::<Link>(second).unwrap().0, restored);
        assert_eq!(
            world
                .get::<Children>(root)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            [restored, second]
        );
        assert!(!world.entity(second).contains::<RuntimeOnly>());
        assert!(world.get_entity(spawned).is_err());
        assert!(world.get_entity(helper).is_ok());
        assert_eq!(world.resource::<Selection>().primary(), Some(restored));
        History::undo(world).unwrap();
        assert_eq!(world.get::<Counter>(restored).unwrap().0, 3);
    }

    #[test]
    fn repeated_sessions_preserve_redo_and_process_one_transition_per_frame() {
        let mut app = app();
        History::execute(
            app.world_mut(),
            SpawnEntity::new("actor").with_component(Counter(0)),
        )
        .unwrap();
        History::undo(app.world_mut()).unwrap();
        for _ in 0..2 {
            request_play_action(app.world_mut(), PlayAction::Play);
            request_play_action(app.world_mut(), PlayAction::Stop);
            app.update();
            assert_eq!(
                *app.world().resource::<State<EditorState>>().get(),
                EditorState::Playing
            );
            app.update();
            assert_eq!(
                *app.world().resource::<State<EditorState>>().get(),
                EditorState::Editing
            );
            assert_eq!(app.world().resource::<History>().redo_len(), 1);
        }
        assert!(History::redo(app.world_mut()).unwrap());
    }

    #[test]
    fn loaded_model_restores_deleted_root_and_edited_children_without_duplicates() {
        use bevy::world_serialization::{WorldAsset, WorldAssetRoot, WorldSerializationPlugin};
        let mut app = app();
        app.add_plugins((AssetPlugin::default(), WorldSerializationPlugin))
            .register_type::<Visibility>()
            .register_type::<InheritedVisibility>()
            .register_type::<ViewVisibility>();
        let mut source = World::new();
        source.spawn((Name::new("model child"), Counter(7)));
        let handle = app
            .world_mut()
            .resource_mut::<Assets<WorldAsset>>()
            .add(WorldAsset::new(source));
        History::execute(
            app.world_mut(),
            SpawnEntity::new("model").with_component(WorldAssetRoot(handle)),
        )
        .unwrap();
        app.update();
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<WorldAssetRoot>>()
            .single(app.world())
            .unwrap();
        let child = app.world().get::<Children>(root).unwrap()[0];
        History::execute(
            app.world_mut(),
            SetComponentField::typed::<Counter>(child, "0", "42"),
        )
        .unwrap();
        action(&mut app, PlayAction::Play);
        app.world_mut().despawn(root);
        action(&mut app, PlayAction::Stop);
        app.update();
        let restored_root = resolve_entity(app.world(), root);
        let restored_child = resolve_entity(app.world(), child);
        assert_eq!(app.world().get::<Counter>(restored_child).unwrap().0, 42);
        assert_eq!(app.world().get::<Children>(restored_root).unwrap().len(), 1);
        assert_eq!(
            app.world_mut()
                .query::<&Counter>()
                .iter(app.world())
                .count(),
            1
        );
        History::undo(app.world_mut()).unwrap();
        assert_eq!(app.world().get::<Counter>(restored_child).unwrap().0, 7);
        // Undo/redo of the import must work even after native instantiation.
        History::undo(app.world_mut()).unwrap();
        app.update();
        History::redo(app.world_mut()).unwrap();
        app.update();
        History::redo(app.world_mut()).unwrap();
        let child = resolve_entity(app.world(), child);
        assert_eq!(app.world().get::<Counter>(child).unwrap().0, 42);
        assert_eq!(
            app.world_mut()
                .query::<&Counter>()
                .iter(app.world())
                .count(),
            1
        );
        // A second Play session also preserves the reconciled subtree.
        action(&mut app, PlayAction::Play);
        action(&mut app, PlayAction::Stop);
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&Counter>()
                .iter(app.world())
                .count(),
            1
        );
        assert_eq!(app.world().get::<Counter>(child).unwrap().0, 42);
    }
}
