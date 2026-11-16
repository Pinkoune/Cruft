use crate::persistence::{error, parse};
use bevy::{
    ecs::entity::{EntityHashMap, EntityMapper},
    prelude::*,
};
use cruft_core::{
    CommandError, EditorCommand, EditorEvent, Selection, remap_entity_ids, resolve_entity,
};
use std::collections::HashSet;

/// Import a RON scene. Undo removes only imported entities; redo recreates them
/// and remaps IDs so commands later in the redo stack keep working.
pub struct LoadScene {
    ron: String,
    entities: EntityHashMap<Entity>,
    applied: bool,
    error: Option<CommandError>,
}

impl LoadScene {
    pub fn new(ron: impl Into<String>) -> Self {
        Self {
            ron: ron.into(),
            entities: EntityHashMap::default(),
            applied: false,
            error: None,
        }
    }

    /// Source-file IDs -> IDs allocated at the last apply. Use `resolve_entity`
    /// before querying if a subsequent DespawnEntity was undone.
    pub fn entity_map(&self) -> &EntityHashMap<Entity> {
        &self.entities
    }

    fn try_apply(&mut self, world: &mut World) -> Result<(), CommandError> {
        if self.applied {
            return Err(error("Scène déjà chargée"));
        }
        let mut prepared = parse(world, &self.ron)?;
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let map: EntityHashMap<_> = prepared
            .iter()
            .map(|entity| (entity.source, world.spawn(cruft_core::SceneEntity).id()))
            .collect();
        let mut mapper = SceneMapper(&map);
        for entity in &mut prepared {
            for component in &mut entity.components {
                component
                    .access
                    .map_entities(component.value.as_mut(), &mut mapper);
                // Insertion replaces required defaults exactly (Reflect::apply
                // would merge lists and could leave extra default elements).
                component.access.insert(
                    &mut world.entity_mut(map[&entity.source]),
                    component.value.as_partial_reflect(),
                    &registry,
                );
            }
        }
        // All entities and components exist before hierarchy hooks are run.
        for entity in &prepared {
            if let Some(children) = &entity.children {
                let children: Vec<_> = children.iter().map(|child| map[child]).collect();
                world
                    .entity_mut(map[&entity.source])
                    .add_children(&children);
            }
        }
        for entity in &prepared {
            if let Some(parent) = entity.parent
                && world.get::<ChildOf>(map[&entity.source]).is_none()
            {
                world
                    .entity_mut(map[&entity.source])
                    .insert(ChildOf(map[&parent]));
            }
        }
        for entity in &prepared {
            if entity
                .children
                .as_ref()
                .is_some_and(|children| !children.is_empty())
            {
                // Serialized model children are already authoritative. Avoid
                // appending another copy when its source asset finishes loading.
                cruft_core::preserve_restored_instance(world, map[&entity.source]);
            }
        }
        let remap = self
            .entities
            .iter()
            .map(|(source, previous)| (resolve_entity(world, *previous), map[source]))
            .collect();
        remap_entity_ids(world, &remap)?;
        self.entities = map;
        self.applied = true;
        Ok(())
    }

    fn try_undo(&mut self, world: &mut World) -> Result<(), CommandError> {
        if !self.applied {
            return Err(error("Scène non chargée"));
        }
        let mut entities: HashSet<_> = self
            .entities
            .values()
            .map(|entity| resolve_entity(world, *entity))
            .collect();
        // A root-only scene import may have instantiated loader-owned children
        // since apply. Those belong to this import, unlike manually attached nodes.
        if let Some(spawner) =
            world.get_resource::<bevy::world_serialization::WorldInstanceSpawner>()
        {
            let generated: Vec<_> = entities
                .iter()
                .filter_map(|entity| world.get::<bevy::world_serialization::WorldInstance>(*entity))
                .flat_map(|instance| spawner.iter_instance_entities(**instance))
                .collect();
            entities.extend(generated);
        }
        // Refuse external hierarchy changes rather than deleting foreign children.
        for entity in &entities {
            let entity_ref = world.get_entity(*entity).map_err(error)?;
            if entity_ref
                .get::<Children>()
                .is_some_and(|children| children.iter().any(|child| !entities.contains(&child)))
            {
                return Err(error(
                    "Une entité extérieure a été attachée à la scène chargée",
                ));
            }
        }
        if let Some(mut selection) = world.get_resource_mut::<Selection>() {
            for entity in &entities {
                selection.remove(*entity);
            }
        }
        for entity in entities {
            if world.get_entity(entity).is_ok() {
                world.despawn(entity);
            }
        }
        self.applied = false;
        Ok(())
    }

    fn report(&mut self, world: &mut World, result: Result<(), CommandError>) {
        self.error = result.err();
        if let Some(error) = &self.error {
            world.trigger(EditorEvent::Toast(error.to_string()));
        }
    }
}

impl EditorCommand for LoadScene {
    fn apply(&mut self, world: &mut World) {
        let result = self.try_apply(world);
        self.report(world, result);
    }
    fn undo(&mut self, world: &mut World) {
        let result = self.try_undo(world);
        self.report(world, result);
    }
    fn label(&self) -> String {
        "Charger la scène".into()
    }
    fn error(&self) -> Option<&CommandError> {
        self.error.as_ref()
    }
}

struct SceneMapper<'a>(&'a EntityHashMap<Entity>);

impl EntityMapper for SceneMapper<'_> {
    fn get_mapped(&mut self, source: Entity) -> Entity {
        self.0.get(&source).copied().unwrap_or(source)
    }
    fn set_mapped(&mut self, _source: Entity, _target: Entity) {}
}
