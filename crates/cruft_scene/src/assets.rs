use bevy::prelude::*;
use cruft_core::{CommandError, History, SpawnEntity};
use std::path::{Path, PathBuf};

/// Bevy 0.20 renamed SceneRoot to WorldAssetRoot.
pub use bevy::world_serialization::WorldAssetRoot as SceneRoot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    Glb,
    Png,
    Exr,
    Ron,
    Scene,
}
impl AssetKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Glb => "GLB",
            Self::Png => "PNG",
            Self::Exr => "EXR",
            Self::Ron => "RON",
            Self::Scene => "SCÈNE",
        }
    }
    fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "glb" => Some(Self::Glb),
            "png" => Some(Self::Png),
            "exr" => Some(Self::Exr),
            "scene" => Some(Self::Scene),
            "ron" => Some(if path.to_string_lossy().ends_with(".scn.ron") {
                Self::Scene
            } else {
                Self::Ron
            }),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetEntry {
    pub path: String,
    pub kind: AssetKind,
}

#[derive(Resource)]
pub struct AssetCatalog {
    root: PathBuf,
    entries: Vec<AssetEntry>,
    pub errors: Vec<String>,
}
impl Default for AssetCatalog {
    fn default() -> Self {
        Self::new("assets")
    }
}
impl AssetCatalog {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            entries: Vec::new(),
            errors: Vec::new(),
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn entries(&self) -> &[AssetEntry] {
        &self.entries
    }
    /// Recursive, deterministic scan. Symlinks are omitted (no loops/outside roots).
    pub fn refresh(&mut self) {
        self.entries.clear();
        self.errors.clear();
        if !self.root.exists() {
            return;
        }
        let mut pending = vec![self.root.clone()];
        while let Some(dir) = pending.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(error) => {
                    self.errors.push(format!("{} : {error}", dir.display()));
                    continue;
                }
            };
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        self.errors.push(error.to_string());
                        continue;
                    }
                };
                let ty = match entry.file_type() {
                    Ok(ty) => ty,
                    Err(error) => {
                        self.errors.push(error.to_string());
                        continue;
                    }
                };
                if ty.is_symlink() {
                    continue;
                }
                let path = entry.path();
                if ty.is_dir() {
                    pending.push(path);
                } else if ty.is_file()
                    && let Some(kind) = AssetKind::from_path(&path)
                {
                    self.entries.push(AssetEntry {
                        path: path
                            .strip_prefix(&self.root)
                            .expect("inside root")
                            .to_string_lossy()
                            .replace('\\', "/"),
                        kind,
                    });
                }
            }
        }
        self.entries.sort_by(|a, b| a.path.cmp(&b.path));
    }
}

pub(crate) fn scan_assets(mut catalog: ResMut<AssetCatalog>) {
    catalog.refresh();
}

/// Instantiate Scene0 of a catalogued GLB through the common command history.
/// Loading is asynchronous; loader errors are captured by the tracing console.
pub fn import_glb(world: &mut World, path: &str, translation: Vec3) -> Result<(), CommandError> {
    let catalog = world
        .get_resource::<AssetCatalog>()
        .ok_or_else(|| CommandError("Catalogue d'assets absent".into()))?;
    if !catalog
        .entries
        .iter()
        .any(|e| e.path == path && e.kind == AssetKind::Glb)
    {
        return Err(CommandError("GLB absent du catalogue assets/".into()));
    }
    let full = catalog
        .root
        .join(path)
        .canonicalize()
        .map_err(|e| CommandError(e.to_string()))?;
    let root = catalog
        .root
        .canonicalize()
        .map_err(|e| CommandError(e.to_string()))?;
    if !full.starts_with(root) {
        return Err(CommandError("Asset hors du dossier assets/".into()));
    }
    let server = world
        .get_resource::<AssetServer>()
        .ok_or_else(|| CommandError("AssetServer absent".into()))?;
    let handle =
        server.load(bevy::asset::AssetPath::from(PathBuf::from(path)).with_label("Scene0"));
    let name = Path::new(path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    History::execute(
        world,
        SpawnEntity::new(name)
            .with_component(SceneRoot(handle))
            .with_component(Transform::from_translation(translation)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scans_nested_supported_types_and_refreshes() {
        let root = std::env::temp_dir().join(format!("cruft-assets-{}", std::process::id()));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        for file in [
            "nested/model.GLB",
            "texture.png",
            "light.exr",
            "data.ron",
            "map.scene",
            "skip.txt",
        ] {
            std::fs::write(root.join(file), []).unwrap();
        }
        let mut catalog = AssetCatalog::new(&root);
        catalog.refresh();
        assert_eq!(catalog.entries.len(), 5);
        assert!(
            catalog
                .entries
                .iter()
                .any(|e| e.path == "nested/model.GLB" && e.kind == AssetKind::Glb)
        );
        std::fs::remove_file(root.join("data.ron")).unwrap();
        catalog.refresh();
        assert_eq!(catalog.entries.len(), 4);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn glb_import_is_a_single_undoable_spawn_with_a_path_backed_scene_root() {
        use crate::CruftScenePlugin;
        use bevy::world_serialization::WorldSerializationPlugin;
        use cruft_core::CruftCorePlugin;
        let root = std::env::temp_dir().join(format!("cruft-import-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("test.glb"),
            include_bytes!("../tests/fixtures/triangle.glb"),
        )
        .unwrap();
        let mut catalog = AssetCatalog::new(&root);
        catalog.refresh();
        let mut app = App::new();
        app.insert_resource(catalog).add_plugins((
            MinimalPlugins,
            AssetPlugin {
                file_path: root.to_string_lossy().into_owned(),
                ..default()
            },
            WorldSerializationPlugin,
            CruftCorePlugin,
            CruftScenePlugin,
        ));
        let world = app.world_mut();
        assert!(import_glb(world, "../test.glb", Vec3::ZERO).is_err());
        assert_eq!(world.resource::<History>().undo_len(), 0);
        import_glb(world, "test.glb", Vec3::new(1.0, 2.0, 3.0)).unwrap();
        let entity = world
            .query_filtered::<Entity, With<SceneRoot>>()
            .single(world)
            .unwrap();
        assert_eq!(
            world
                .get::<SceneRoot>(entity)
                .unwrap()
                .0
                .path()
                .unwrap()
                .to_string(),
            "test.glb#Scene0"
        );
        assert_eq!(
            world.get::<Transform>(entity).unwrap().translation,
            Vec3::new(1.0, 2.0, 3.0)
        );
        assert_eq!(world.resource::<History>().undo_len(), 1);
        History::undo(world).unwrap();
        assert!(world.get_entity(entity).is_err());
        History::redo(world).unwrap();
        assert_eq!(world.query::<&SceneRoot>().iter(world).count(), 1);
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }
}
