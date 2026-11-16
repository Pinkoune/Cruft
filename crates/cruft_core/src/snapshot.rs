use crate::{CommandError, failure};
use bevy::{
    ecs::{
        component::ComponentCloneBehavior,
        entity::{EntityHashMap, EntityMapper},
    },
    prelude::*,
    reflect::{ReflectFromReflect, TypeRegistry},
};
use std::any::TypeId;

/// Explicit engine caches which are regenerated after entity restoration.
/// Unknown application components still reject undo instead of losing data.
#[derive(Resource, Default)]
pub struct SnapshotExclusions(std::collections::HashSet<TypeId>);

impl SnapshotExclusions {
    pub fn with<T: Component>(mut self) -> Self {
        self.0.insert(TypeId::of::<T>());
        self
    }
    pub fn contains(&self, ty: TypeId) -> bool {
        self.0.contains(&ty)
    }
}

#[derive(Resource, Default, Clone)]
pub(crate) struct EntityAliases(pub EntityHashMap<Entity>);

/// Bevy assigns a new generation when a despawned entity is restored.
/// Commands accept original IDs; UI/MCP can use this function before querying.
pub fn resolve_entity(world: &World, entity: Entity) -> Entity {
    world
        .get_resource::<EntityAliases>()
        .and_then(|aliases| aliases.0.get(&entity).copied())
        .unwrap_or(entity)
}

/// Publish old-to-new IDs after an editor command recreates entities. Keys must
/// be IDs from this World, never IDs read from a scene file. All targets must be
/// live entities. Historical aliases and the current selection are updated.
pub fn remap_entity_ids(
    world: &mut World,
    remap: &EntityHashMap<Entity>,
) -> Result<(), CommandError> {
    for target in remap.values() {
        if world.get_entity(*target).is_err() {
            return Err(failure("Cible de remappage absente"));
        }
    }
    world.init_resource::<EntityAliases>();
    let mut aliases = world.resource_mut::<EntityAliases>();
    for value in aliases.0.values_mut() {
        if let Some(target) = remap.get(value) {
            *value = *target;
        }
    }
    for (source, target) in remap {
        if source != target {
            aliases.0.insert(*source, *target);
        }
    }
    let aliases = aliases.clone();
    if let Some(mut selection) = world.get_resource_mut::<crate::Selection>() {
        selection.remap(|entity| aliases.0.get(&entity).copied().unwrap_or(entity));
    }
    Ok(())
}

pub(crate) fn existing(world: &World, entity: Entity) -> Result<Entity, CommandError> {
    let entity = resolve_entity(world, entity);
    world
        .get_entity(entity)
        .map(|_| entity)
        .map_err(|_| failure(format!("Entité absente : {entity:?}")))
}

pub(crate) fn is_hierarchy(type_id: TypeId) -> bool {
    type_id == TypeId::of::<ChildOf>() || type_id == TypeId::of::<Children>()
}

pub(crate) fn clone_reflected(
    value: &dyn Reflect,
    registry: &TypeRegistry,
) -> Result<Box<dyn Reflect>, CommandError> {
    value
        .reflect_clone()
        .or_else(|_| {
            registry
                .get(value.as_any().type_id())
                .and_then(|registration| registration.data::<ReflectFromReflect>())
                .and_then(|from| from.from_reflect(value.as_partial_reflect()))
                .ok_or(bevy::reflect::ReflectCloneError::NotImplemented {
                    type_path: value.reflect_type_path().to_owned().into(),
                })
        })
        .map_err(|error| failure(error.to_string()))
}

pub(crate) struct ComponentSnapshot {
    pub value: Box<dyn Reflect>,
    pub access: ReflectComponent,
}

pub(crate) struct EntitySnapshot {
    pub entity: Entity,
    pub parent: Option<Entity>,
    pub index: usize,
    pub components: Vec<ComponentSnapshot>,
    pub loaded_instance: bool,
}

pub(crate) struct Snapshot(pub Vec<EntitySnapshot>);

pub(crate) fn parent_position(world: &World, entity: Entity) -> (Option<Entity>, usize) {
    let parent = world.get::<ChildOf>(entity).map(ChildOf::parent);
    let index = parent
        .and_then(|parent| world.get::<Children>(parent))
        .and_then(|children| children.iter().position(|child| child == entity))
        .unwrap_or(0);
    (parent, index)
}

pub(crate) fn attach(world: &mut World, child: Entity, parent: Option<Entity>, index: usize) {
    world.entity_mut(child).remove::<ChildOf>();
    if let Some(parent) = parent {
        let len = world
            .get::<Children>(parent)
            .map_or(0, |children| children.len());
        world
            .entity_mut(parent)
            .insert_children(index.min(len), &[child]);
    }
}

impl Snapshot {
    /// Validate and clone the whole subtree before removing anything.
    pub fn capture(world: &World, root: Entity) -> Result<Self, CommandError> {
        let registry = world.resource::<AppTypeRegistry>().read();
        let mut pending = vec![root];
        let mut snapshots = Vec::new();
        while let Some(entity) = pending.pop() {
            let entity_ref = world.entity(entity);
            if let Some(children) = entity_ref.get::<Children>() {
                pending.extend(children.iter().rev());
            }
            let mut components = Vec::new();
            for component_id in entity_ref.archetype().components() {
                let info = world
                    .components()
                    .get_info(*component_id)
                    .expect("component exists");
                let type_id = info
                    .type_id()
                    .ok_or_else(|| failure("Composant dynamique non pris en charge"))?;
                if is_hierarchy(type_id)
                    || type_id == TypeId::of::<bevy::world_serialization::WorldInstance>()
                    || world
                        .get_resource::<SnapshotExclusions>()
                        .is_some_and(|excluded| excluded.contains(type_id))
                    || matches!(info.clone_behavior(), ComponentCloneBehavior::Ignore)
                {
                    continue;
                }
                let access = registry
                    .get(type_id)
                    .and_then(|r| r.data::<ReflectComponent>())
                    .ok_or_else(|| {
                        failure(format!(
                            "Composant non enregistré pour l'undo : {}",
                            info.name()
                        ))
                    })?;
                let value = access
                    .reflect(entity_ref)
                    .ok_or_else(|| failure("Composant inaccessible"))?;
                components.push(ComponentSnapshot {
                    value: clone_reflected(value, &registry)?,
                    access: access.clone(),
                });
            }
            let (parent, index) = parent_position(world, entity);
            snapshots.push(EntitySnapshot {
                loaded_instance: crate::instance_is_loaded(world, entity),
                entity,
                parent,
                index,
                components,
            });
        }
        Ok(Self(snapshots))
    }

    pub fn restore(&self, world: &mut World) -> Result<(), CommandError> {
        // Parents outside the snapshot must still exist. Validate before spawning.
        for snapshot in &self.0 {
            if let Some(parent) = snapshot.parent
                && !self.0.iter().any(|entry| entry.entity == parent)
            {
                existing(world, parent)?;
            }
        }
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        // Clone first: a reflection failure must leave the world untouched.
        let mut components = self
            .0
            .iter()
            .map(|snapshot| {
                snapshot
                    .components
                    .iter()
                    .map(|component| {
                        Ok((
                            component.access.clone(),
                            clone_reflected(component.value.as_ref(), &registry)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, CommandError>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        world.init_resource::<EntityAliases>();
        let mut aliases = world.resource::<EntityAliases>().clone();
        for snapshot in &self.0 {
            let restored = world.spawn_empty().id();
            let previous = resolve_entity(world, snapshot.entity);
            for value in aliases.0.values_mut() {
                if *value == previous {
                    *value = restored;
                }
            }
            aliases.0.insert(snapshot.entity, restored);
            aliases.0.insert(previous, restored);
        }
        let mut mapper = AliasMapper { aliases: &aliases };
        for (snapshot, components) in self.0.iter().zip(&mut components) {
            let entity = aliases.0[&snapshot.entity];
            for (access, value) in components {
                access.map_entities(value.as_mut(), &mut mapper);
                access.insert(
                    &mut world.entity_mut(entity),
                    value.as_partial_reflect(),
                    &registry,
                );
            }
        }
        for snapshot in &self.0 {
            let entity = aliases.0[&snapshot.entity];
            let parent = snapshot
                .parent
                .map(|parent| aliases.0.get(&parent).copied().unwrap_or(parent));
            attach(world, entity, parent, snapshot.index);
        }
        for snapshot in &self.0 {
            if snapshot.loaded_instance {
                crate::preserve_restored_instance(world, aliases.0[&snapshot.entity]);
            }
        }
        world.insert_resource(aliases);
        Ok(())
    }
}

struct AliasMapper<'a> {
    aliases: &'a EntityAliases,
}

impl EntityMapper for AliasMapper<'_> {
    fn get_mapped(&mut self, source: Entity) -> Entity {
        self.aliases.0.get(&source).copied().unwrap_or(source)
    }

    fn set_mapped(&mut self, _source: Entity, _target: Entity) {
        // All IDs are allocated before component insertion.
    }
}
