use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use cruft_core::{
    CruftCorePlugin, EditorCommand, Gameplay, Primitive, PrimitiveShape, SpawnEntity,
};
use cruft_mcp::CruftMcpPlugin;
use cruft_scene::{AssetCatalog, CruftScenePlugin};
use cruft_ui::{CruftUiPlugin, ViewportRenderTarget};
use std::collections::HashMap;

fn main() {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let display = cruft_ui::DisplaySettings::load();
    App::new()
        .insert_resource(display.clone())
        .insert_resource(render_snapshot_exclusions())
        .insert_resource(AssetCatalog::new(&assets))
        .insert_resource(ClearColor(Color::srgb(0.035, 0.05, 0.065)))
        .insert_resource(derived_components())
        .add_plugins(
            DefaultPlugins
                .set(bevy::log::LogPlugin {
                    custom_layer: cruft_core::console::capture_layer,
                    // stdout carries the MCP protocol in stdio mode.
                    fmt_layer: if cruft_mcp::stdio_requested() {
                        stderr_fmt_layer
                    } else {
                        |_| None
                    },
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: assets.to_string_lossy().into_owned(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(display.initial_window("Cruft")),
                    ..default()
                }),
        )
        .add_plugins((
            CruftCorePlugin,
            CruftScenePlugin,
            CruftMcpPlugin::default(),
            CruftUiPlugin,
        ))
        .add_systems(Startup, setup_scene)
        .add_systems(Update, animate_test_cubes.in_set(Gameplay))
        .add_systems(Update, sync_screenshot_target)
        .add_systems(PostUpdate, materialize_primitives)
        .run();
}

fn stderr_fmt_layer(_: &mut App) -> Option<bevy::log::BoxedFmtLayer> {
    Some(Box::new(
        bevy::log::tracing_subscriber::fmt::Layer::default().with_writer(std::io::stderr),
    ))
}

/// `screenshot_viewport` captures the viewport texture, not the whole window.
fn sync_screenshot_target(
    viewport: Option<Res<ViewportRenderTarget>>,
    mut target: ResMut<cruft_mcp::McpScreenshotTarget>,
) {
    let image = viewport.map(|v| v.image.clone());
    if target.0 != image {
        target.0 = image;
    }
}

/// `Mesh3d` / material are rebuilt from `Primitive`, so they are not saved.
fn derived_components() -> cruft_scene::DerivedComponents {
    use std::any::TypeId;
    cruft_scene::DerivedComponents::default()
        .with::<Primitive>(&[
            TypeId::of::<Mesh3d>(),
            TypeId::of::<MeshMaterial3d<StandardMaterial>>(),
        ])
        // Render-internal components required by cameras (CameraMainTextureUsages,
        // ColorGrading…) are re-inserted by Bevy; some are not serializable.
        .lenient::<Camera3d>()
}

/// Derives render data (mesh + material) from the serializable `Primitive`
/// component, for new, loaded or edited primitives. Not a scene edit: like
/// `GlobalTransform`, it is recomputed from scene data.
#[allow(clippy::type_complexity)]
fn materialize_primitives(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cache: Local<HashMap<PrimitiveShape, Handle<Mesh>>>,
    primitives: Query<
        (Entity, &Primitive),
        Or<(
            Changed<Primitive>,
            Without<Mesh3d>,
            Without<MeshMaterial3d<StandardMaterial>>,
        )>,
    >,
) {
    for (entity, primitive) in &primitives {
        let mesh = cache
            .entry(primitive.shape)
            .or_insert_with(|| {
                meshes.add(match primitive.shape {
                    PrimitiveShape::Cube => Mesh::from(Cuboid::new(1.0, 1.0, 1.0)),
                    PrimitiveShape::Sphere => Sphere::new(0.5).mesh().uv(32, 18),
                    PrimitiveShape::Plane => Plane3d::default().mesh().size(1.0, 1.0).into(),
                    PrimitiveShape::Capsule => Capsule3d::new(0.5, 1.0).into(),
                    PrimitiveShape::Cylinder => Cylinder::new(0.5, 1.0).into(),
                })
            })
            .clone();
        let [r, g, b] = primitive.color.to_array();
        let material = materials.add(Color::srgb(r, g, b));
        commands
            .entity(entity)
            .insert((Mesh3d(mesh), MeshMaterial3d(material)));
    }
}

fn render_snapshot_exclusions() -> cruft_core::SnapshotExclusions {
    cruft_core::SnapshotExclusions::default().with::<bevy::pbr::PreviousGlobalTransform>()
}

/// Even the initial scene is built through EditorCommand. Bootstrap commands
/// are applied directly so the user's history starts empty.
fn setup_scene(world: &mut World) {
    let commands = [
        SpawnEntity::new("Sol")
            .with_component(Primitive::new(
                PrimitiveShape::Plane,
                Vec3::new(0.13, 0.18, 0.19),
            ))
            .with_component(Transform::from_scale(Vec3::new(12.0, 1.0, 12.0))),
        SpawnEntity::new("Cube bleu")
            .with_component(Primitive::new(
                PrimitiveShape::Cube,
                Vec3::new(0.26, 0.60, 0.75),
            ))
            .with_component(Transform::from_xyz(-1.2, 0.5, 0.0)),
        SpawnEntity::new("Cube rouge")
            .with_component(Primitive::new(
                PrimitiveShape::Cube,
                Vec3::new(0.65, 0.13, 0.27),
            ))
            .with_component(Transform::from_xyz(1.2, 0.5, 0.0)),
        SpawnEntity::new("Lumière")
            .with_component(PointLight {
                intensity: 1_500_000.0,
                shadow_maps_enabled: true,
                ..default()
            })
            .with_component(Transform::from_xyz(4.0, 7.0, 4.0)),
        SpawnEntity::new("Caméra")
            .with_component(Camera3d::default())
            .with_component(Transform::from_xyz(6.0, 5.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y)),
    ];
    for mut command in commands {
        command.apply(world);
        if let Some(error) = command.error() {
            panic!("Échec de la scène de test : {error}");
        }
    }
    world
        .run_system_once(materialize_primitives)
        .expect("materialize primitives");
}

fn animate_test_cubes(time: Res<Time>, mut cubes: Query<(&Name, &mut Transform)>) {
    for (name, mut transform) in &mut cubes {
        if name.as_str() == "Cube bleu" || name.as_str() == "Cube rouge" {
            transform.rotate_y(time.delta_secs() * 0.6);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cruft_core::History;

    #[test]
    fn real_glb_loads_and_survives_play_stop_and_import_undo_redo() {
        use cruft_scene::{PlayAction, import_glb, request_play_action};
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../cruft_scene/tests/fixtures");
        let mut catalog = AssetCatalog::new(&root);
        catalog.refresh();
        let mut app = App::new();
        app.insert_resource(catalog)
            .insert_resource(render_snapshot_exclusions())
            .add_plugins((
                MinimalPlugins,
                AssetPlugin {
                    file_path: root.to_string_lossy().into_owned(),
                    ..default()
                },
                bevy::world_serialization::WorldSerializationPlugin,
                bevy::mesh::MeshPlugin,
                bevy::image::ImagePlugin::default(),
                bevy::gltf::GltfPlugin::default(),
                CruftCorePlugin,
                CruftScenePlugin,
            ))
            .init_asset::<StandardMaterial>()
            .init_asset::<AnimationClip>();
        app.finish();
        app.cleanup();
        import_glb(app.world_mut(), "triangle.glb", Vec3::ZERO).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.update();
            if app.world_mut().query::<&Mesh3d>().iter(app.world()).count() == 1 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "GLB did not load within five seconds"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // This renderer cache is inserted only after a real rendered frame.
        // Include it in the headless test so import undo matches the live editor.
        let meshes: Vec<_> = app
            .world_mut()
            .query_filtered::<Entity, With<Mesh3d>>()
            .iter(app.world())
            .collect();
        for mesh in meshes {
            app.world_mut()
                .entity_mut(mesh)
                .insert(bevy::pbr::PreviousGlobalTransform::default());
        }
        request_play_action(app.world_mut(), PlayAction::Play);
        app.update();
        request_play_action(app.world_mut(), PlayAction::Stop);
        app.update();
        app.update();
        assert_eq!(
            app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
            1
        );
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<cruft_scene::SceneRoot>>()
            .single(app.world())
            .unwrap();
        let mut subtree = vec![root];
        let mut index = 0;
        while index < subtree.len() {
            if let Some(children) = app.world().get::<Children>(subtree[index]) {
                subtree.extend(children.iter());
            }
            index += 1;
        }
        let ron = cruft_scene::save_scene(app.world(), subtree).unwrap();
        cruft_scene::load_scene(app.world_mut(), &ron).unwrap();
        app.update();
        app.update();
        assert_eq!(
            app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
            2
        );
        History::undo(app.world_mut()).unwrap();
        app.update();
        assert_eq!(
            app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
            1
        );
        History::undo(app.world_mut()).unwrap();
        app.update();
        assert_eq!(
            app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
            0
        );
        History::redo(app.world_mut()).unwrap();
        app.update();
        app.update();
        assert_eq!(
            app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
            1
        );
    }

    #[test]
    fn bootstrap_creates_ground_two_cubes_light_and_camera_with_empty_history() {
        let mut app = App::new();
        app.add_plugins(CruftCorePlugin)
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>();
        let world = app.world_mut();
        setup_scene(world);
        assert_eq!(world.query::<&Name>().iter(world).count(), 5);
        assert_eq!(world.query::<&Mesh3d>().iter(world).count(), 3);
        assert_eq!(world.query::<&PointLight>().iter(world).count(), 1);
        assert_eq!(world.query::<&Camera3d>().iter(world).count(), 1);
        assert_eq!(world.resource::<History>().undo_len(), 0);
        let snapshot = cruft_scene::SceneSnapshot::capture(world).unwrap();
        let entity = world
            .query_filtered::<Entity, With<Mesh3d>>()
            .iter(world)
            .next()
            .unwrap();
        let mesh = world.get::<Mesh3d>(entity).unwrap().0.clone();
        world.entity_mut(entity).remove::<Mesh3d>();
        snapshot.restore(world).unwrap();
        assert_eq!(world.get::<Mesh3d>(entity).unwrap().0, mesh);
        assert!(world.resource::<Assets<Mesh>>().get(&mesh).is_some());
    }

    #[test]
    fn primitive_scene_saves_without_ephemeral_meshes_and_reloads_visible() {
        let mut app = App::new();
        app.add_plugins(CruftCorePlugin)
            .insert_resource(derived_components())
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .register_type::<Mesh3d>()
            .register_type::<MeshMaterial3d<StandardMaterial>>()
            .add_systems(PostUpdate, materialize_primitives);
        let world = app.world_mut();
        setup_scene(world);
        let ron = cruft_scene::save_scene(world, cruft_scene::scene_entities(world))
            .expect("primitives are saved as data, not ephemeral handles");
        assert!(ron.contains("Primitive"));
        assert!(!ron.contains("Mesh3d"));
        cruft_scene::load_scene(world, &ron).unwrap();
        app.update();
        let world = app.world_mut();
        assert_eq!(world.query::<&Primitive>().iter(world).count(), 6);
        assert_eq!(world.query::<&Mesh3d>().iter(world).count(), 6);
    }
}
