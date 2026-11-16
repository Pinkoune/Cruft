use super::*;

#[derive(Component, Reflect, Debug, PartialEq)]
#[reflect(Component)]
struct Stats {
    speed: f32,
    title: String,
    values: Vec<i32>,
}

#[derive(Component, Reflect)]
#[reflect(Component)]
struct Target(#[entities] Entity);

fn world() -> World {
    let mut app = App::new();
    app.add_plugins(CruftCorePlugin)
        .register_type::<Stats>()
        .register_type::<Target>();
    std::mem::take(app.world_mut())
}

fn stats(world: &mut World) -> Entity {
    world
        .spawn(Stats {
            speed: 1.0,
            title: "cube".into(),
            values: vec![1, 2, 3],
        })
        .id()
}

fn speed(world: &World, entity: Entity) -> f32 {
    world
        .get::<Stats>(resolve_entity(world, entity))
        .unwrap()
        .speed
}

fn edit(entity: Entity, value: f32) -> SetComponentField {
    SetComponentField::typed::<Stats>(entity, "speed", value.to_string())
}

#[test]
fn spawn_apply_undo_redo_preserves_components() {
    let mut world = world();
    let mut command = SpawnEntity::new("Cube").with_component(Transform::from_xyz(1.0, 2.0, 3.0));
    command.apply(&mut world);
    assert_eq!(command.error(), None);
    let original = command.entity().unwrap();
    assert_eq!(world.get::<Name>(original).unwrap().as_str(), "Cube");
    assert_eq!(
        world.get::<Transform>(original).unwrap().translation,
        Vec3::new(1.0, 2.0, 3.0)
    );
    for _ in 0..3 {
        command.undo(&mut world);
        assert_eq!(command.error(), None);
        assert!(world.get_entity(resolve_entity(&world, original)).is_err());
        command.apply(&mut world);
        assert_eq!(command.error(), None);
        let entity = resolve_entity(&world, original);
        assert_eq!(
            world.get::<Transform>(entity).unwrap().translation,
            Vec3::new(1.0, 2.0, 3.0)
        );
    }
}

#[test]
fn despawn_restores_subtree_components_order_selection_and_internal_references() {
    let mut world = world();
    let parent = world.spawn_empty().id();
    let first = world.spawn_empty().id();
    let root = stats(&mut world);
    let last = world.spawn_empty().id();
    let child = world.spawn(Name::new("Child")).id();
    world.entity_mut(parent).add_children(&[first, root, last]);
    world
        .entity_mut(root)
        .add_child(child)
        .insert(Target(child));
    world.entity_mut(child).insert(Target(parent));
    world.resource_mut::<Selection>().add(root);
    world.resource_mut::<Selection>().add(child);
    let mut command = DespawnEntity::new(root);
    for _ in 0..3 {
        command.apply(&mut world);
        assert_eq!(command.error(), None);
        assert!(world.get_entity(resolve_entity(&world, root)).is_err());
        assert!(world.get_entity(resolve_entity(&world, child)).is_err());
        assert!(world.resource::<Selection>().entities().is_empty());
        command.undo(&mut world);
        assert_eq!(command.error(), None);
        let restored = resolve_entity(&world, root);
        let restored_child = resolve_entity(&world, child);
        assert_eq!(
            world.get::<Children>(parent).unwrap().as_ref(),
            &[first, restored, last]
        );
        assert_eq!(
            world.get::<ChildOf>(restored_child).unwrap().parent(),
            restored
        );
        assert_eq!(world.get::<Target>(restored).unwrap().0, restored_child);
        assert_eq!(world.get::<Target>(restored_child).unwrap().0, parent);
        assert_eq!(world.get::<Stats>(restored).unwrap().values, [1, 2, 3]);
        assert_eq!(
            world.resource::<Selection>().primary(),
            Some(restored_child)
        );
        assert_eq!(
            world.resource::<Selection>().entities(),
            &[restored, restored_child]
        );
    }
}

#[test]
fn despawn_rejects_unregistered_components_atomically() {
    #[derive(Component)]
    struct Unregistered;
    let mut world = world();
    let root = stats(&mut world);
    let child = world.spawn(Unregistered).id();
    world.entity_mut(root).add_child(child);
    assert!(History::execute(&mut world, DespawnEntity::new(root)).is_err());
    assert!(world.get_entity(root).is_ok());
    assert!(world.get::<Unregistered>(child).is_some());
    assert_eq!(world.resource::<History>().undo_len(), 0);
}

#[test]
fn field_apply_undo_redo_for_nested_scalar_string_and_list() {
    let mut world = world();
    let entity = stats(&mut world);
    let mut scalar = edit(entity, 7.0);
    scalar.apply(&mut world);
    assert_eq!(scalar.error(), None);
    assert_eq!(speed(&world, entity), 7.0);
    scalar.undo(&mut world);
    assert_eq!(speed(&world, entity), 1.0);
    scalar.apply(&mut world);
    assert_eq!(speed(&world, entity), 7.0);

    for (path, value) in [("title", "\"renamed\""), ("values[1]", "42")] {
        let mut command = SetComponentField::typed::<Stats>(entity, path, value);
        command.apply(&mut world);
        assert_eq!(command.error(), None);
        command.undo(&mut world);
        assert_eq!(command.error(), None);
    }
    assert_eq!(world.get::<Stats>(entity).unwrap().title, "cube");
    assert_eq!(world.get::<Stats>(entity).unwrap().values, [1, 2, 3]);
    let mut list = SetComponentField::typed::<Stats>(entity, "values", "[8]");
    list.apply(&mut world);
    assert_eq!(list.error(), None);
    assert_eq!(world.get::<Stats>(entity).unwrap().values, [8]);
    list.undo(&mut world);
    assert_eq!(world.get::<Stats>(entity).unwrap().values, [1, 2, 3]);
    world.entity_mut(entity).insert(Transform::default());
    let mut nested = SetComponentField::typed::<Transform>(entity, "translation.x", "3.5");
    nested.apply(&mut world);
    assert_eq!(nested.error(), None);
    assert_eq!(world.get::<Transform>(entity).unwrap().translation.x, 3.5);
    nested.undo(&mut world);
    assert_eq!(world.get::<Transform>(entity).unwrap().translation.x, 0.0);
}

#[test]
fn invalid_field_edits_leave_scene_and_history_untouched() {
    let mut world = world();
    let entity = stats(&mut world);
    for (path, value) in [
        ("missing", "5.0"),
        ("speed", "\"wrong type\""),
        ("values[8]", "42"),
        ("speed", "2.0 trailing"),
    ] {
        assert!(
            History::execute(
                &mut world,
                SetComponentField::typed::<Stats>(entity, path, value)
            )
            .is_err()
        );
        assert_eq!(speed(&world, entity), 1.0);
        assert_eq!(world.resource::<History>().undo_len(), 0);
    }
}

#[test]
fn reparent_apply_undo_redo_detach_and_sibling_order() {
    let mut world = world();
    let old = world.spawn_empty().id();
    let new = world.spawn_empty().id();
    let first = world.spawn_empty().id();
    let child = world.spawn(Transform::from_xyz(1.0, 2.0, 3.0)).id();
    let last = world.spawn_empty().id();
    world.entity_mut(old).add_children(&[first, child, last]);
    for parent in [Some(new), None] {
        let mut command = Reparent::new(child, parent);
        for _ in 0..2 {
            command.apply(&mut world);
            assert_eq!(command.error(), None);
            assert_eq!(world.get::<ChildOf>(child).map(ChildOf::parent), parent);
            assert_eq!(
                world.get::<Transform>(child).unwrap().translation,
                Vec3::new(1.0, 2.0, 3.0)
            );
            command.undo(&mut world);
            assert_eq!(command.error(), None);
            assert_eq!(
                world.get::<Children>(old).unwrap().as_ref(),
                &[first, child, last]
            );
        }
    }
}

#[test]
fn reparent_rejects_self_cycles_and_missing_entities() {
    let mut world = world();
    let root = world.spawn_empty().id();
    let child = world.spawn(ChildOf(root)).id();
    let missing = world.spawn_empty().id();
    world.despawn(missing);
    for (entity, parent) in [
        (root, Some(root)),
        (root, Some(child)),
        (root, Some(missing)),
        (missing, Some(root)),
    ] {
        assert!(History::execute(&mut world, Reparent::new(entity, parent)).is_err());
        assert!(world.get::<ChildOf>(root).is_none());
        assert_eq!(world.get::<ChildOf>(child).unwrap().parent(), root);
    }
}

#[test]
fn history_merges_drag_and_separates_gestures_and_fields() {
    let mut world = world();
    let entity = stats(&mut world);
    for value in 2..12 {
        History::execute(&mut world, edit(entity, value as f32)).unwrap();
    }
    assert_eq!(world.resource::<History>().undo_len(), 1);
    History::undo(&mut world).unwrap();
    assert_eq!(speed(&world, entity), 1.0);
    History::redo(&mut world).unwrap();
    assert_eq!(speed(&world, entity), 11.0);
    History::execute(&mut world, edit(entity, 12.0)).unwrap();
    assert_eq!(world.resource::<History>().undo_len(), 2);
    world.resource_mut::<History>().finish_group();
    History::execute(&mut world, edit(entity, 13.0)).unwrap();
    History::execute(
        &mut world,
        SetComponentField::typed::<Stats>(entity, "title", "\"new\""),
    )
    .unwrap();
    History::execute(&mut world, edit(entity, 14.0)).unwrap();
    assert_eq!(world.resource::<History>().undo_len(), 5);
    History::undo(&mut world).unwrap();
    assert_eq!(speed(&world, entity), 13.0);
}

#[test]
fn history_limits_to_200_and_invalidates_redo_only_on_success() {
    let mut world = world();
    let entity = stats(&mut world);
    for value in 2..207 {
        world.resource_mut::<History>().finish_group();
        History::execute(&mut world, edit(entity, value as f32)).unwrap();
    }
    assert_eq!(world.resource::<History>().undo_len(), 200);
    for _ in 0..200 {
        assert!(History::undo(&mut world).unwrap());
    }
    assert!(!History::undo(&mut world).unwrap());
    assert_eq!(speed(&world, entity), 6.0);
    assert!(
        History::execute(
            &mut world,
            SetComponentField::typed::<Stats>(entity, "missing", "5")
        )
        .is_err()
    );
    assert_eq!(world.resource::<History>().redo_len(), 200);
    History::execute(&mut world, edit(entity, 42.0)).unwrap();
    assert_eq!(world.resource::<History>().redo_len(), 0);
    assert!(!History::redo(&mut world).unwrap());
}

#[test]
fn earlier_commands_follow_entities_across_delete_undo_and_spawn_redo() {
    let mut world = world();
    History::execute(
        &mut world,
        SpawnEntity::new("Cube").with_component(Stats {
            speed: 1.0,
            title: "cube".into(),
            values: vec![],
        }),
    )
    .unwrap();
    let entity = world
        .query_filtered::<Entity, With<Stats>>()
        .single(&world)
        .unwrap();
    History::execute(&mut world, edit(entity, 5.0)).unwrap();
    History::execute(&mut world, DespawnEntity::new(entity)).unwrap();
    for _ in 0..3 {
        History::undo(&mut world).unwrap();
        assert_eq!(speed(&world, entity), 5.0);
        History::undo(&mut world).unwrap();
        assert_eq!(speed(&world, entity), 1.0);
        History::undo(&mut world).unwrap();
        assert!(world.get_entity(resolve_entity(&world, entity)).is_err());
        History::redo(&mut world).unwrap();
        History::redo(&mut world).unwrap();
        assert_eq!(speed(&world, entity), 5.0);
        History::redo(&mut world).unwrap();
    }
}

#[test]
fn selection_primary_is_unique_and_always_selected() {
    let mut world = world();
    let a = world.spawn_empty().id();
    let b = world.spawn_empty().id();
    let mut selection = Selection::default();
    selection.add(a);
    selection.add(b);
    selection.add(b);
    assert_eq!(selection.entities(), &[a, b]);
    selection.remove(b);
    assert_eq!(selection.primary(), Some(a));
    selection.select_only(b);
    assert_eq!(selection.entities(), &[b]);
    selection.clear();
    assert_eq!(selection.primary(), None);
}

#[test]
fn history_emits_toasts_for_ui_observers() {
    #[derive(Resource, Default)]
    struct Toasts(Vec<String>);
    let mut world = world();
    world.init_resource::<Toasts>();
    world.add_observer(|event: On<EditorEvent>, mut toasts: ResMut<Toasts>| {
        let EditorEvent::Toast(message) = event.event();
        toasts.0.push(message.clone());
    });
    History::execute(&mut world, SpawnEntity::new("Toast cube")).unwrap();
    History::undo(&mut world).unwrap();
    History::redo(&mut world).unwrap();
    assert_eq!(world.resource::<Toasts>().0.len(), 3);
    assert!(world.resource::<Toasts>().0[0].contains("Toast cube"));
}
