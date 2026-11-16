//! Editor commands, history and selection, independent of the UI.

mod commands;
mod composite;
pub mod console;
mod history;
mod inspector;
mod instances;
pub mod json;
mod primitive;
pub mod proposals;
mod selection;
mod snapshot;
mod state;

pub use commands::{DespawnEntity, Reparent, SetComponentField, SpawnEntity};
pub use composite::{CompositeCommand, InsertComponent};
pub use history::{EditorCommand, History};
pub use inspector::{ComponentView, FieldKind, FieldView, component_views};
pub use instances::{instance_is_loaded, preserve_restored_instance};
pub use primitive::{Primitive, PrimitiveShape};
pub use proposals::{
    AgentActivity, AgentMode, DiffOp, GhostPreview, Proposal, ProposalStatus, Proposals,
    ProposedChange, SubmitOutcome,
};
pub use selection::Selection;
pub use snapshot::{SnapshotExclusions, remap_entity_ids, resolve_entity};
pub use state::{EditorOnly, EditorState, Gameplay, SceneEntity};

use bevy::prelude::*;
use std::fmt;

/// Immediate Bevy event, consumed by UI observers.
#[derive(Event, Clone, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    Toast(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandError(pub String);

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CommandError {}

pub(crate) fn failure(message: impl Into<String>) -> CommandError {
    CommandError(message.into())
}

pub(crate) fn report(world: &mut World, result: Result<(), CommandError>) -> Option<CommandError> {
    if let Err(error) = result {
        world.trigger(EditorEvent::Toast(error.to_string()));
        Some(error)
    } else {
        None
    }
}

pub struct CruftCorePlugin;

impl Plugin for CruftCorePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bevy::state::app::StatesPlugin>() {
            app.add_plugins(bevy::state::app::StatesPlugin);
        }
        app.init_state::<EditorState>()
            .add_observer(instances::instance_ready)
            .add_systems(
                SpawnScene,
                instances::reload_restored_instances
                    .before(bevy::world_serialization::world_instance_spawner),
            )
            .configure_sets(Update, Gameplay.run_if(in_state(EditorState::Playing)))
            .configure_sets(FixedUpdate, Gameplay.run_if(in_state(EditorState::Playing)))
            .register_type::<SceneEntity>()
            .register_type::<Primitive>();
        app.init_resource::<History>()
            .init_resource::<SnapshotExclusions>()
            .init_resource::<Selection>()
            .init_resource::<Proposals>()
            .init_resource::<AgentMode>()
            .init_resource::<AgentActivity>()
            .init_resource::<snapshot::EntityAliases>()
            .init_resource::<AppTypeRegistry>()
            .register_type::<Name>()
            .register_type::<Transform>()
            .register_type::<GlobalTransform>()
            .register_type::<bevy::transform::components::TransformTreeChanged>()
            .register_type::<ChildOf>()
            .register_type::<Children>();
    }
}

#[cfg(test)]
mod tests;
