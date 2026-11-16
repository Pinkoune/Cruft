//! RON scene persistence using Bevy 0.20's reflected `DynamicWorld` format.
//! Loading imports entities as one undoable command; existing entities/resources
//! are preserved. Saving takes an explicit set of entities, excluding editor UI.
mod assets;
mod command;
mod persistence;
mod play;

pub use assets::{AssetCatalog, AssetEntry, AssetKind, SceneRoot, import_glb};
pub use command::LoadScene;
pub use persistence::{
    DerivedComponents, load_scene, load_scene_file, save_scene, save_scene_file, scene_entities,
    scene_roots,
};
pub use play::{PlayAction, PlayMode, SceneSnapshot, request_play_action};

use bevy::prelude::*;

pub struct CruftScenePlugin;

impl Plugin for CruftScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AppTypeRegistry>()
            .register_type::<Visibility>()
            .register_type::<InheritedVisibility>()
            .register_type::<ViewVisibility>()
            .init_resource::<PlayMode>()
            .init_resource::<AssetCatalog>()
            .add_systems(Startup, assets::scan_assets)
            .add_systems(
                OnEnter(cruft_core::EditorState::Editing),
                play::restore_editing_scene,
            )
            .add_systems(PreUpdate, play::process_play_actions);
    }
}

#[cfg(test)]
mod tests;
