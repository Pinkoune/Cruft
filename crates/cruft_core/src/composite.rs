use crate::snapshot::{clone_reflected, existing, is_hierarchy};
use crate::{CommandError, EditorCommand, failure, report};
use bevy::prelude::*;

/// Several commands applied as ONE history entry (multi-selection, agent proposals).
/// A failing child rolls back the children already applied.
pub struct CompositeCommand {
    label: String,
    commands: Vec<Box<dyn EditorCommand>>,
    error: Option<CommandError>,
}

impl CompositeCommand {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            commands: Vec::new(),
            error: None,
        }
    }

    pub fn push(&mut self, command: impl EditorCommand) {
        self.commands.push(Box::new(command));
    }

    pub fn push_boxed(&mut self, command: Box<dyn EditorCommand>) {
        self.commands.push(command);
    }

    pub fn with(mut self, command: impl EditorCommand) -> Self {
        self.push(command);
        self
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Typed access to children, e.g. to read back spawned entities.
    pub fn children(&self) -> impl Iterator<Item = &dyn EditorCommand> {
        self.commands.iter().map(|command| command.as_ref())
    }
}

impl EditorCommand for CompositeCommand {
    fn apply(&mut self, world: &mut World) {
        self.error = None;
        if self.commands.is_empty() {
            self.error = Some(failure("Aucune modification"));
            return;
        }
        for index in 0..self.commands.len() {
            self.commands[index].apply(world);
            if let Some(error) = self.commands[index].error().cloned() {
                for done in self.commands[..index].iter_mut().rev() {
                    done.undo(world);
                }
                self.error = Some(error);
                return;
            }
        }
    }

    fn undo(&mut self, world: &mut World) {
        self.error = None;
        let count = self.commands.len();
        for index in (0..count).rev() {
            self.commands[index].undo(world);
            if let Some(error) = self.commands[index].error().cloned() {
                for undone in self.commands[index + 1..].iter_mut() {
                    undone.apply(world);
                }
                self.error = Some(error);
                return;
            }
        }
    }

    fn label(&self) -> String {
        self.label.clone()
    }

    fn error(&self) -> Option<&CommandError> {
        self.error.as_ref()
    }
}

/// Adds a component that the entity does not have yet; undo removes it.
pub struct InsertComponent {
    entity: Entity,
    type_path: String,
    value: Box<dyn Reflect>,
    applied: bool,
    error: Option<CommandError>,
}

impl InsertComponent {
    /// `value` must be a concrete component whose type is registered.
    pub fn new(entity: Entity, value: Box<dyn Reflect>) -> Self {
        Self {
            entity,
            type_path: value.reflect_type_path().to_owned(),
            value,
            applied: false,
            error: None,
        }
    }

    pub fn typed<T: Component + Reflect>(entity: Entity, value: T) -> Self {
        Self::new(entity, Box::new(value))
    }

    fn access(&self, world: &World) -> Result<ReflectComponent, CommandError> {
        let registry = world.resource::<AppTypeRegistry>().read();
        let registration = registry
            .get_with_type_path(&self.type_path)
            .ok_or_else(|| failure(format!("Type non enregistré : {}", self.type_path)))?;
        if is_hierarchy(registration.type_id()) {
            return Err(failure("Utiliser Reparent pour modifier la hiérarchie"));
        }
        registration
            .data::<ReflectComponent>()
            .cloned()
            .ok_or_else(|| failure("Type non composant"))
    }

    fn try_apply(&mut self, world: &mut World) -> Result<(), CommandError> {
        if self.applied {
            return Err(failure("Ajout déjà appliqué"));
        }
        let entity = existing(world, self.entity)?;
        let access = self.access(world)?;
        if access.contains(world.entity(entity)) {
            return Err(failure("Composant déjà présent"));
        }
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let value = clone_reflected(self.value.as_ref(), &registry)?;
        access.insert(
            &mut world.entity_mut(entity),
            value.as_partial_reflect(),
            &registry,
        );
        self.applied = true;
        Ok(())
    }

    fn try_undo(&mut self, world: &mut World) -> Result<(), CommandError> {
        if !self.applied {
            return Err(failure("Ajout non appliqué"));
        }
        let entity = existing(world, self.entity)?;
        let access = self.access(world)?;
        access.remove(&mut world.entity_mut(entity));
        self.applied = false;
        Ok(())
    }
}

impl EditorCommand for InsertComponent {
    fn apply(&mut self, world: &mut World) {
        let result = self.try_apply(world);
        self.error = report(world, result);
    }
    fn undo(&mut self, world: &mut World) {
        let result = self.try_undo(world);
        self.error = report(world, result);
    }
    fn label(&self) -> String {
        let short = self
            .type_path
            .rsplit("::")
            .next()
            .unwrap_or(&self.type_path);
        format!("Ajouter {short}")
    }
    fn error(&self) -> Option<&CommandError> {
        self.error.as_ref()
    }
}
