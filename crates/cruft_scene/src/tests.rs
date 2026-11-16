use super::*;
use bevy::{ecs::entity::EntityHashMap, world_serialization::DynamicWorld};
use cruft_core::{
    CruftCorePlugin, EditorCommand, History, Selection, SetComponentField, resolve_entity,
};
use std::collections::HashMap;

#[derive(Reflect, Debug, PartialEq)]
enum Mode {
    Idle,
    Active { power: f32 },
}

#[derive(Component, Reflect, Debug, PartialEq)]
#[reflect(Component)]
struct Data {
    title: String,
    values: Vec<i32>,
    weights: HashMap<String, f32>,
    mode: Mode,
}

#[derive(Component, Reflect)]
#[reflect(Component)]
struct Link(#[entities] Entity);

#[derive(Component, Reflect)]
#[reflect(Component)]
struct Unregistered(u32);

#[derive(Component, Reflect)]
#[component(clone_behavior = Ignore)]
#[reflect(Component)]
struct Transient;

fn app() -> App {
    let mut app = App::new();
    app.insert_resource(AppTypeRegistry::default());
    app.add_plugins((CruftCorePlugin, CruftScenePlugin))
        .register_type::<Data>()
        .register_type::<Link>()
        .register_type::<Transient>();
    app
}

fn fixture() -> (App, Vec<Entity>) {
    let mut app = app();
    let world = app.world_mut();
    let root = world
        .spawn((
            Name::new("Root"),
            Transform::from_xyz(1.0, 2.0, 3.0),
            Data {
                title: "Test \"RON\"\n世界".into(),
                values: vec![1, -2, 3],
                weights: HashMap::from([("weight".into(), 0.5)]),
                mode: Mode::Active { power: 8.5 },
            },
            Unregistered(10),
            Transient,
        ))
        .id();
    let a = world
        .spawn((
            Name::new("A"),
            Transform::from_xyz(4.0, 5.0, 6.0),
            Link(root),
        ))
        .id();
    let b = world.spawn((Name::new("B"), Transform::default())).id();
    world.entity_mut(root).add_children(&[b, a]).insert(Link(a));
    let empty = world.spawn_empty().id();
    (app, vec![root, a, b, empty])
}

fn assert_scene(source: &World, target: &World, ids: &[Entity], map: &EntityHashMap<Entity>) {
    assert_eq!(map.len(), ids.len());
    for original in ids {
        let restored = map[original];
        assert_eq!(source.get::<Name>(*original), target.get::<Name>(restored));
        assert_eq!(
            source.get::<Transform>(*original),
            target.get::<Transform>(restored)
        );
        assert_eq!(source.get::<Data>(*original), target.get::<Data>(restored));
        if let Some(link) = source.get::<Link>(*original) {
            assert_eq!(target.get::<Link>(restored).unwrap().0, map[&link.0]);
        }
        if let Some(parent) = source.get::<ChildOf>(*original) {
            assert_eq!(
                target.get::<ChildOf>(restored).unwrap().parent(),
                map[&parent.parent()]
            );
        }
        if let Some(children) = source.get::<Children>(*original) {
            assert_eq!(
                target
                    .get::<Children>(restored)
                    .unwrap()
                    .iter()
                    .collect::<Vec<_>>(),
                children.iter().map(|child| map[&child]).collect::<Vec<_>>()
            );
        }
        assert!(target.get::<Unregistered>(restored).is_none());
        assert!(target.get::<Transient>(restored).is_none());
    }
}

#[test]
fn save_load_preserves_registered_components_hierarchy_references_and_empty_entities() {
    let (source, ids) = fixture();
    let ron = save_scene(source.world(), ids.clone()).unwrap();
    assert!(!ron.contains(Unregistered::type_path()));
    assert!(!ron.contains(Transient::type_path()));
    assert!(!ron.contains("History"));
    let mut target = app();
    let sentinel = target.world_mut().spawn(Name::new("Existing")).id();
    let mut command = LoadScene::new(&ron);
    command.apply(target.world_mut());
    assert_eq!(command.error(), None);
    assert_scene(source.world(), target.world(), &ids, command.entity_map());
    assert_eq!(
        target.world().get::<Name>(sentinel).unwrap().as_str(),
        "Existing"
    );

    // A second serialization/load must still be equivalent after IDs change again.
    let again = save_scene(target.world(), command.entity_map().values().copied()).unwrap();
    let mut third = app();
    let mut second_load = LoadScene::new(again);
    second_load.apply(third.world_mut());
    assert_eq!(second_load.error(), None);
    let composed = command
        .entity_map()
        .iter()
        .map(|(original, intermediate)| (*original, second_load.entity_map()[intermediate]))
        .collect();
    assert_scene(source.world(), third.world(), &ids, &composed);
}

#[test]
fn loading_is_one_undoable_entry_and_later_edits_survive_redo() {
    let (source, ids) = fixture();
    let ron = save_scene(source.world(), ids).unwrap();
    let mut target = app();
    let world = target.world_mut();
    load_scene(world, &ron).unwrap();
    assert_eq!(world.resource::<History>().undo_len(), 1);
    let root = world
        .query_filtered::<Entity, With<Data>>()
        .single(world)
        .unwrap();
    world.resource_mut::<Selection>().select_only(root);
    History::execute(
        world,
        SetComponentField::typed::<Data>(root, "title", "\"Edited\""),
    )
    .unwrap();
    for _ in 0..3 {
        History::undo(world).unwrap();
        History::undo(world).unwrap();
        assert!(world.get_entity(resolve_entity(world, root)).is_err());
        assert_eq!(world.resource::<Selection>().primary(), None);
        History::redo(world).unwrap();
        History::redo(world).unwrap();
        assert_eq!(
            world
                .get::<Data>(resolve_entity(world, root))
                .unwrap()
                .title,
            "Edited"
        );
    }
}

#[test]
fn invalid_loads_leave_world_and_history_unchanged() {
    let mut target = app();
    let world = target.world_mut();
    let sentinel = world.spawn(Name::new("Keep")).id();
    let before = world.entities().len();
    for input in [
        "not RON",
        "(resources: {}, entities: {}) trailing",
        "(resources: {}, entities: {0: (components: {\"unknown::Type\": ()})})",
    ] {
        assert!(load_scene(world, input).is_err());
        assert_eq!(world.entities().len(), before);
        assert_eq!(world.get::<Name>(sentinel).unwrap().as_str(), "Keep");
        assert_eq!(world.resource::<History>().undo_len(), 0);
    }
}

#[test]
fn save_rejects_missing_registry_missing_entities_and_external_references() {
    let (mut source, ids) = fixture();
    assert!(save_scene(source.world(), [ids[0]]).is_err());
    assert!(save_scene(&World::new(), []).is_err());
    let absent = source.world_mut().spawn_empty().id();
    source.world_mut().despawn(absent);
    assert!(save_scene(source.world(), [absent]).is_err());
}

#[test]
fn resource_payload_is_rejected() {
    #[derive(Resource, Reflect)]
    #[reflect(Resource)]
    struct Secret(u32);
    let mut target = app();
    target.register_type::<Secret>();
    target.world_mut().insert_resource(Secret(7));
    let scene = DynamicWorld {
        resources: vec![Box::new(Secret(99))],
        entities: vec![],
    };
    let ron = scene
        .serialize(&target.world().resource::<AppTypeRegistry>().read())
        .unwrap();
    assert!(load_scene(target.world_mut(), &ron).is_err());
    assert_eq!(target.world().resource::<Secret>().0, 7);
}

#[test]
fn hierarchy_cycle_is_rejected_before_spawning() {
    let mut target = app();
    let mut staged = World::new();
    let a = staged.spawn_empty().id();
    let b = staged.spawn_empty().id();
    // Construct the serialized payload directly, never insert a cycle in a World.
    let scene = DynamicWorld {
        resources: vec![],
        entities: vec![
            bevy::world_serialization::DynamicEntity {
                entity: a,
                components: vec![Box::new(ChildOf(b))],
            },
            bevy::world_serialization::DynamicEntity {
                entity: b,
                components: vec![Box::new(ChildOf(a))],
            },
        ],
    };
    let ron = scene
        .serialize(&target.world().resource::<AppTypeRegistry>().read())
        .unwrap();
    let before = target.world().entities().len();
    assert!(load_scene(target.world_mut(), &ron).is_err());
    assert_eq!(target.world().entities().len(), before);
}

#[test]
fn file_roundtrip_and_missing_file_errors() {
    let (source, ids) = fixture();
    let path = std::env::temp_dir().join(format!(
        "cruft-scene-{}-{}.ron",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    save_scene_file(source.world(), ids, &path).unwrap();
    let mut target = app();
    load_scene_file(target.world_mut(), &path).unwrap();
    assert_eq!(
        target
            .world_mut()
            .query::<&Data>()
            .iter(target.world())
            .count(),
        1
    );
    std::fs::remove_file(&path).unwrap();
    assert!(load_scene_file(target.world_mut(), &path).is_err());
}

#[test]
fn ephemeral_assets_are_rejected_instead_of_serialized_as_defaults() {
    #[derive(Asset, TypePath)]
    struct TestAsset;
    #[derive(Component, Reflect)]
    #[reflect(Component)]
    struct AssetRef(Handle<TestAsset>);
    let mut source = app();
    source
        .register_type::<AssetRef>()
        .init_resource::<Assets<TestAsset>>();
    let handle = source
        .world_mut()
        .resource_mut::<Assets<TestAsset>>()
        .add(TestAsset);
    let entity = source.world_mut().spawn(AssetRef(handle)).id();
    assert!(save_scene(source.world(), [entity]).is_err());
}
