use bevy::prelude::*;

#[derive(States, Default, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditorState {
    #[default]
    Editing,
    Playing,
    Paused,
}

/// Put gameplay systems in this set (Update or FixedUpdate).
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Gameplay;

/// Authored scene entity. Commands and scene imports add this automatically.
#[derive(Component, Reflect, Default, Clone, Copy)]
#[reflect(Component, Default)]
pub struct SceneEntity;

/// Editor infrastructure excluded from Play snapshots and runtime cleanup.
#[derive(Component)]
pub struct EditorOnly;
