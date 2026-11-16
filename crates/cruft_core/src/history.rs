use crate::{CommandError, EditorEvent};
use bevy::prelude::*;
use std::{any::Any, collections::VecDeque};

/// All scene mutations from UI and MCP go through this interface.
/// Implementations must validate before mutation and expose failures via `error`.
pub trait EditorCommand: Any + Send + Sync {
    fn apply(&mut self, world: &mut World);
    fn undo(&mut self, world: &mut World);
    fn label(&self) -> String;

    /// Absorb an already applied command, preserving the oldest undo value.
    fn merge(&mut self, _newer: &dyn EditorCommand) -> bool {
        false
    }

    fn error(&self) -> Option<&CommandError> {
        None
    }
}

#[derive(Resource, Default)]
pub struct History {
    undo: VecDeque<Box<dyn EditorCommand>>,
    redo: Vec<Box<dyn EditorCommand>>,
    merge_open: bool,
}

impl History {
    pub const LIMIT: usize = 200;

    /// Uses `resource_scope` internally, so callers need only an exclusive World.
    pub fn execute(world: &mut World, command: impl EditorCommand) -> Result<(), CommandError> {
        Self::execute_boxed(world, Box::new(command))
    }

    pub fn execute_boxed(
        world: &mut World,
        mut command: Box<dyn EditorCommand>,
    ) -> Result<(), CommandError> {
        ensure_editing(world)?;
        world.init_resource::<Self>();
        world.resource_scope(|world, mut history: Mut<Self>| {
            command.apply(world);
            if let Some(error) = command.error() {
                return Err(error.clone());
            }
            let label = command.label();
            history.redo.clear();
            let merged = history.merge_open
                && history
                    .undo
                    .back_mut()
                    .is_some_and(|previous| previous.merge(command.as_ref()));
            if !merged {
                history.undo.push_back(command);
                if history.undo.len() > Self::LIMIT {
                    history.undo.pop_front();
                }
            }
            history.merge_open = true;
            world.trigger(EditorEvent::Toast(label));
            Ok(())
        })
    }

    pub fn undo(world: &mut World) -> Result<bool, CommandError> {
        ensure_editing(world)?;
        world.init_resource::<Self>();
        world.resource_scope(|world, mut history: Mut<Self>| {
            history.finish_group();
            let Some(mut command) = history.undo.pop_back() else {
                return Ok(false);
            };
            command.undo(world);
            if let Some(error) = command.error().cloned() {
                history.undo.push_back(command);
                return Err(error);
            }
            world.trigger(EditorEvent::Toast(format!("Annuler : {}", command.label())));
            history.redo.push(command);
            Ok(true)
        })
    }

    pub fn redo(world: &mut World) -> Result<bool, CommandError> {
        ensure_editing(world)?;
        world.init_resource::<Self>();
        world.resource_scope(|world, mut history: Mut<Self>| {
            history.finish_group();
            let Some(mut command) = history.redo.pop() else {
                return Ok(false);
            };
            command.apply(world);
            if let Some(error) = command.error().cloned() {
                history.redo.push(command);
                return Err(error);
            }
            world.trigger(EditorEvent::Toast(format!(
                "Rétablir : {}",
                command.label()
            )));
            history.undo.push_back(command);
            Ok(true)
        })
    }

    /// Call on drag end, focus change, or before an independent MCP edit.
    pub fn finish_group(&mut self) {
        self.merge_open = false;
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.finish_group();
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
    pub fn undo_label(&self) -> Option<String> {
        self.undo.back().map(|command| command.label())
    }
    pub fn redo_label(&self) -> Option<String> {
        self.redo.last().map(|command| command.label())
    }
}

fn ensure_editing(world: &mut World) -> Result<(), CommandError> {
    if world
        .get_resource::<State<crate::EditorState>>()
        .is_some_and(|state| *state.get() != crate::EditorState::Editing)
    {
        let error = CommandError("Arrêtez Play avant de modifier la scène".into());
        world.trigger(EditorEvent::Toast(error.to_string()));
        return Err(error);
    }
    Ok(())
}
