use crate::snapshot::{Snapshot, attach, clone_reflected, existing, is_hierarchy, parent_position};
use crate::{CommandError, EditorCommand, Selection, failure, report, resolve_entity};
use bevy::{
    prelude::*,
    reflect::{
        GetPath, GetTypeRegistration, ReflectFromReflect, TypeRegistry,
        serde::{TypedReflectDeserializer, TypedReflectSerializer},
    },
};
use serde::de::DeserializeSeed;
use std::any::Any;

struct SpawnComponent {
    value: Box<dyn Reflect>,
    register: fn(&mut TypeRegistry),
}

/// Spawn a named entity with reflected components. Required components are
/// inserted by Bevy. Use `Reparent` to establish hierarchy links.
pub struct SpawnEntity {
    name: String,
    components: Vec<SpawnComponent>,
    parent: Option<Entity>,
    entity: Option<Entity>,
    deletion: Option<DespawnEntity>,
    applied: bool,
    error: Option<CommandError>,
}

impl SpawnEntity {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            components: Vec::new(),
            parent: None,
            entity: None,
            deletion: None,
            applied: false,
            error: None,
        }
    }

    pub fn with_component<T: Component + Reflect + GetTypeRegistration>(
        mut self,
        value: T,
    ) -> Self {
        self.components.push(SpawnComponent {
            value: Box::new(value),
            register: TypeRegistry::register::<T>,
        });
        self
    }

    /// Adds an already reflected component (e.g. deserialized from JSON).
    /// Its type must already be registered in the `AppTypeRegistry`.
    pub fn with_boxed(mut self, value: Box<dyn Reflect>) -> Self {
        self.components.push(SpawnComponent {
            value,
            register: |_| {},
        });
        self
    }

    /// Attach the new entity under `parent` (last child).
    pub fn with_parent(mut self, parent: Option<Entity>) -> Self {
        self.parent = parent;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Original ID. Resolve it with `resolve_entity` after undo/redo.
    pub fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn try_apply(&mut self, world: &mut World) -> Result<(), CommandError> {
        if self.applied {
            return Err(failure("Création déjà appliquée"));
        }
        if let Some(deletion) = &mut self.deletion {
            deletion.try_undo(world)?;
        } else {
            let parent = self
                .parent
                .map(|parent| existing(world, parent))
                .transpose()?;
            world.init_resource::<AppTypeRegistry>();
            let registry = world.resource::<AppTypeRegistry>().clone();
            let mut registry = registry.write();
            let mut prepared = Vec::new();
            for component in &self.components {
                (component.register)(&mut registry);
                let type_id = component.value.as_any().type_id();
                if is_hierarchy(type_id) {
                    return Err(failure("Utiliser Reparent pour modifier la hiérarchie"));
                }
                let access = registry
                    .get(type_id)
                    .and_then(|r| r.data::<ReflectComponent>())
                    .ok_or_else(|| failure("Le composant doit déclarer #[reflect(Component)]"))?;
                prepared.push((
                    access.clone(),
                    clone_reflected(component.value.as_ref(), &registry)?,
                ));
            }
            registry.register::<crate::SceneEntity>();
            let entity = world
                .spawn((Name::new(self.name.clone()), crate::SceneEntity))
                .id();
            for (access, value) in prepared {
                access.insert(
                    &mut world.entity_mut(entity),
                    value.as_partial_reflect(),
                    &registry,
                );
            }
            if parent.is_some() {
                attach(world, entity, parent, usize::MAX);
            }
            if let Some(previous) = self.entity {
                crate::remap_entity_ids(
                    world,
                    &bevy::ecs::entity::EntityHashMap::from_iter([(previous, entity)]),
                )?;
            }
            self.entity = Some(entity);
        }
        self.applied = true;
        Ok(())
    }

    fn try_undo(&mut self, world: &mut World) -> Result<(), CommandError> {
        if !self.applied {
            return Err(failure("Création non appliquée"));
        }
        let deletion = self
            .deletion
            .get_or_insert_with(|| DespawnEntity::new(self.entity.expect("applied entity")));
        deletion.try_apply(world)?;
        self.applied = false;
        Ok(())
    }
}

impl EditorCommand for SpawnEntity {
    fn apply(&mut self, world: &mut World) {
        let result = self.try_apply(world);
        self.error = report(world, result);
    }
    fn undo(&mut self, world: &mut World) {
        let result = self.try_undo(world);
        self.error = report(world, result);
    }
    fn label(&self) -> String {
        format!("Créer {}", self.name)
    }
    fn error(&self) -> Option<&CommandError> {
        self.error.as_ref()
    }
}

/// Removes an entire subtree, retaining reflected components and sibling order.
/// Unregistered persistent components reject deletion instead of losing data.
pub struct DespawnEntity {
    entity: Entity,
    snapshot: Option<Snapshot>,
    selection: Option<Selection>,
    applied: bool,
    error: Option<CommandError>,
}

impl DespawnEntity {
    pub fn new(entity: Entity) -> Self {
        Self {
            entity,
            snapshot: None,
            selection: None,
            applied: false,
            error: None,
        }
    }

    fn try_apply(&mut self, world: &mut World) -> Result<(), CommandError> {
        if self.applied {
            return Err(failure("Suppression déjà appliquée"));
        }
        let entity = existing(world, self.entity)?;
        let snapshot = Snapshot::capture(world, entity)?;
        self.selection = world.get_resource::<Selection>().cloned();
        if let Some(mut selection) = world.get_resource_mut::<Selection>() {
            for entry in &snapshot.0 {
                selection.remove(entry.entity);
            }
        }
        world.despawn(entity);
        self.snapshot = Some(snapshot);
        self.applied = true;
        Ok(())
    }

    fn try_undo(&mut self, world: &mut World) -> Result<(), CommandError> {
        if !self.applied {
            return Err(failure("Suppression non appliquée"));
        }
        self.snapshot
            .as_ref()
            .expect("applied snapshot")
            .restore(world)?;
        if let Some(mut selection) = self.selection.clone() {
            selection.remap(|entity| resolve_entity(world, entity));
            world.insert_resource(selection);
        }
        self.applied = false;
        Ok(())
    }
}

impl EditorCommand for DespawnEntity {
    fn apply(&mut self, world: &mut World) {
        let result = self.try_apply(world);
        self.error = report(world, result);
    }
    fn undo(&mut self, world: &mut World) {
        let result = self.try_undo(world);
        self.error = report(world, result);
    }
    fn label(&self) -> String {
        "Supprimer l'entité".into()
    }
    fn error(&self) -> Option<&CommandError> {
        self.error.as_ref()
    }
}

/// Edit a reflected path (e.g. `translation.x`) with a typed RON value (e.g. `2.5`).
/// Component names are full reflection type paths, not ambiguous short names.
pub struct SetComponentField {
    entity: Entity,
    component: String,
    path: String,
    value: String,
    before: Option<String>,
    applied: bool,
    error: Option<CommandError>,
}

impl SetComponentField {
    pub fn new(
        entity: Entity,
        component: impl Into<String>,
        path: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            entity,
            component: component.into(),
            path: path.into(),
            value: value.into(),
            before: None,
            applied: false,
            error: None,
        }
    }

    pub fn typed<T: Component + TypePath>(
        entity: Entity,
        path: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self::new(entity, T::type_path(), path, value)
    }

    fn write(&self, world: &mut World, serialized: &str) -> Result<String, CommandError> {
        let entity = existing(world, self.entity)?;
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let registration = registry
            .get_with_type_path(&self.component)
            .ok_or_else(|| failure(format!("Type non enregistré : {}", self.component)))?;
        if is_hierarchy(registration.type_id()) {
            return Err(failure("Utiliser Reparent pour modifier la hiérarchie"));
        }
        let access = registration
            .data::<ReflectComponent>()
            .ok_or_else(|| failure("Type non composant"))?;
        let current = access
            .reflect(world.entity(entity))
            .ok_or_else(|| failure("Composant absent"))?;
        let mut updated = clone_reflected(current, &registry)?;
        let field = updated
            .reflect_path_mut(self.path.as_str())
            .map_err(|error| failure(error.to_string()))?;
        let type_info = field
            .get_represented_type_info()
            .ok_or_else(|| failure("Champ sans type reflect"))?;
        let field_type = registry
            .get(type_info.type_id())
            .ok_or_else(|| failure("Type du champ non enregistré"))?;
        let before = ron::to_string(&TypedReflectSerializer::new(field, &registry))
            .map_err(|error| failure(error.to_string()))?;
        let mut deserializer =
            ron::Deserializer::from_str(serialized).map_err(|error| failure(error.to_string()))?;
        let value = TypedReflectDeserializer::new(field_type, &registry)
            .deserialize(&mut deserializer)
            .map_err(|error| failure(error.to_string()))?;
        deserializer
            .end()
            .map_err(|error| failure(error.to_string()))?;
        let value = field_type
            .data::<ReflectFromReflect>()
            .and_then(|from| from.from_reflect(value.as_ref()))
            .ok_or_else(|| failure("Impossible de reconstruire le type du champ"))?;
        field
            .try_as_reflect_mut()
            .ok_or_else(|| failure("Champ dynamique non modifiable"))?
            .set(value)
            .map_err(|_| failure("Type de valeur incompatible"))?;
        // Replace from a validated concrete copy; this also supports immutable
        // components and ensures their insertion hooks run.
        access.insert(
            &mut world.entity_mut(entity),
            updated.as_partial_reflect(),
            &registry,
        );
        Ok(before)
    }

    fn try_apply(&mut self, world: &mut World) -> Result<(), CommandError> {
        if self.applied {
            return Err(failure("Modification déjà appliquée"));
        }
        let before = self.write(world, &self.value)?;
        if self.before.is_none() {
            self.before = Some(before);
        }
        self.applied = true;
        Ok(())
    }

    fn try_undo(&mut self, world: &mut World) -> Result<(), CommandError> {
        if !self.applied {
            return Err(failure("Modification non appliquée"));
        }
        self.write(world, self.before.as_ref().expect("applied value"))?;
        self.applied = false;
        Ok(())
    }
}

impl EditorCommand for SetComponentField {
    fn apply(&mut self, world: &mut World) {
        let result = self.try_apply(world);
        self.error = report(world, result);
    }
    fn undo(&mut self, world: &mut World) {
        let result = self.try_undo(world);
        self.error = report(world, result);
    }
    fn label(&self) -> String {
        format!("Modifier {}.{}", self.component, self.path)
    }
    fn error(&self) -> Option<&CommandError> {
        self.error.as_ref()
    }

    fn merge(&mut self, newer: &dyn EditorCommand) -> bool {
        let Some(newer) = (newer as &dyn Any).downcast_ref::<Self>() else {
            return false;
        };
        if self.applied
            && newer.applied
            && self.entity == newer.entity
            && self.component == newer.component
            && self.path == newer.path
        {
            self.value.clone_from(&newer.value);
            true
        } else {
            false
        }
    }
}

/// Changes the parent while preserving the local Transform. `None` detaches.
pub struct Reparent {
    entity: Entity,
    parent: Option<Entity>,
    previous: Option<(Option<Entity>, usize)>,
    applied: bool,
    error: Option<CommandError>,
}

impl Reparent {
    pub fn new(entity: Entity, parent: Option<Entity>) -> Self {
        Self {
            entity,
            parent,
            previous: None,
            applied: false,
            error: None,
        }
    }

    fn validate(
        world: &World,
        entity: Entity,
        parent: Option<Entity>,
    ) -> Result<(Entity, Option<Entity>), CommandError> {
        let entity = existing(world, entity)?;
        let parent = parent.map(|parent| existing(world, parent)).transpose()?;
        let mut ancestor = parent;
        let mut visited = std::collections::HashSet::new();
        while let Some(current) = ancestor {
            if current == entity || !visited.insert(current) {
                return Err(failure("Cycle dans la hiérarchie"));
            }
            ancestor = world.get::<ChildOf>(current).map(ChildOf::parent);
        }
        Ok((entity, parent))
    }

    fn try_apply(&mut self, world: &mut World) -> Result<(), CommandError> {
        if self.applied {
            return Err(failure("Parentage déjà appliqué"));
        }
        let (entity, parent) = Self::validate(world, self.entity, self.parent)?;
        if self.previous.is_none() {
            self.previous = Some(parent_position(world, entity));
        }
        if world.get::<ChildOf>(entity).map(ChildOf::parent) != parent {
            attach(world, entity, parent, usize::MAX);
        }
        self.applied = true;
        Ok(())
    }

    fn try_undo(&mut self, world: &mut World) -> Result<(), CommandError> {
        if !self.applied {
            return Err(failure("Parentage non appliqué"));
        }
        let (parent, index) = self.previous.expect("applied parent");
        let (entity, parent) = Self::validate(world, self.entity, parent)?;
        attach(world, entity, parent, index);
        self.applied = false;
        Ok(())
    }
}

impl EditorCommand for Reparent {
    fn apply(&mut self, world: &mut World) {
        let result = self.try_apply(world);
        self.error = report(world, result);
    }
    fn undo(&mut self, world: &mut World) {
        let result = self.try_undo(world);
        self.error = report(world, result);
    }
    fn label(&self) -> String {
        "Changer le parent".into()
    }
    fn error(&self) -> Option<&CommandError> {
        self.error.as_ref()
    }
}
