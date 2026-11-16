//! Owned, read-only descriptions; edits remain `SetComponentField` commands.
use crate::{
    CommandError, failure,
    snapshot::{clone_reflected, existing, is_hierarchy},
};
use bevy::{
    prelude::*,
    reflect::{ReflectFromReflect, ReflectRef, TypeRegistry, serde::TypedReflectSerializer},
};
use std::any::TypeId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentView {
    /// Full reflection type path, accepted by `SetComponentField::new`.
    pub name: String,
    pub fields: Vec<FieldView>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldView {
    /// Relative reflect path. Empty means the whole component.
    pub path: String,
    pub kind: FieldKind,
    /// Typed RON, including quotes for strings. Feed it unchanged to a command.
    /// If `error` is set, this is a debug representation for display only.
    pub value: String,
    pub editable: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Bool,
    SignedInteger,
    UnsignedInteger,
    Float,
    String,
    Char,
    Entity,
    Struct,
    Tuple,
    List,
    Array,
    Map,
    Set,
    Enum { variants: Vec<String> },
    Opaque,
}

/// Lists only registered reflected components. No access to concrete component
/// types is needed by the UI. Components are sorted by name; fields follow
/// declaration/index order. Containers have a RON row as well as child rows.
pub fn component_views(world: &World, entity: Entity) -> Result<Vec<ComponentView>, CommandError> {
    let entity = existing(world, entity)?;
    let registry = world
        .get_resource::<AppTypeRegistry>()
        .ok_or_else(|| failure("TypeRegistry absent"))?
        .read();
    let entity_ref = world.entity(entity);
    let mut components = Vec::new();
    for id in entity_ref.archetype().components() {
        let Some(type_id) = world
            .components()
            .get_info(*id)
            .and_then(|info| info.type_id())
        else {
            continue;
        };
        let Some(registration) = registry.get(type_id) else {
            continue;
        };
        let Some(access) = registration.data::<ReflectComponent>() else {
            continue;
        };
        let Some(value) = access.reflect(entity_ref) else {
            continue;
        };
        let editing = world
            .get_resource::<State<crate::EditorState>>()
            .is_none_or(|state| *state.get() == crate::EditorState::Editing);
        let editable =
            editing && !is_hierarchy(type_id) && clone_reflected(value, &registry).is_ok();
        let mut fields = Vec::new();
        visit(
            value.as_partial_reflect(),
            "".into(),
            &registry,
            editable,
            &mut fields,
            0,
        );
        components.push(ComponentView {
            name: registration.type_info().type_path().into(),
            fields,
        });
    }
    components.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(components)
}

fn kind(value: &dyn PartialReflect) -> FieldKind {
    let type_id = value.get_represented_type_info().map(|info| info.type_id());
    macro_rules! is_type {
        ($($ty:ty),+) => { $(type_id == Some(TypeId::of::<$ty>()))||+ };
    }
    if is_type!(bool) {
        return FieldKind::Bool;
    }
    if is_type!(i8, i16, i32, i64, i128, isize) {
        return FieldKind::SignedInteger;
    }
    if is_type!(u8, u16, u32, u64, u128, usize) {
        return FieldKind::UnsignedInteger;
    }
    if is_type!(f32, f64) {
        return FieldKind::Float;
    }
    if is_type!(String) {
        return FieldKind::String;
    }
    if is_type!(char) {
        return FieldKind::Char;
    }
    if is_type!(Entity) {
        return FieldKind::Entity;
    }
    match value.reflect_ref() {
        ReflectRef::Struct(_) => FieldKind::Struct,
        ReflectRef::Tuple(_) | ReflectRef::TupleStruct(_) => FieldKind::Tuple,
        ReflectRef::List(_) => FieldKind::List,
        ReflectRef::Array(_) => FieldKind::Array,
        ReflectRef::Map(_) => FieldKind::Map,
        ReflectRef::Set(_) => FieldKind::Set,
        ReflectRef::Enum(value) => FieldKind::Enum {
            variants: value
                .get_represented_enum_info()
                .map(|info| info.iter().map(|variant| variant.name().into()).collect())
                .unwrap_or_default(),
        },
        _ => FieldKind::Opaque,
    }
}

fn child_path(parent: &str, child: impl std::fmt::Display) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}.{child}")
    }
}

fn visit(
    value: &dyn PartialReflect,
    path: String,
    registry: &TypeRegistry,
    editable: bool,
    fields: &mut Vec<FieldView>,
    depth: usize,
) {
    let kind = kind(value);
    let editable = editable && kind != FieldKind::Entity;
    // The root struct is represented by its component heading; other containers
    // get a row so empty collections and enum variants remain editable.
    if !path.is_empty() || !matches!(value.reflect_ref(), ReflectRef::Struct(_)) || depth == 32 {
        let serialized = ron::to_string(&TypedReflectSerializer::new(value, registry));
        let reconstructible = value
            .get_represented_type_info()
            .and_then(|info| registry.get(info.type_id()))
            .is_some_and(|registration| registration.data::<ReflectFromReflect>().is_some());
        let error = serialized.as_ref().err().map(ToString::to_string);
        fields.push(FieldView {
            path: path.clone(),
            kind,
            editable: editable && reconstructible && error.is_none(),
            value: serialized.unwrap_or_else(|_| format!("{value:?}")),
            error,
        });
    }
    if depth == 32 {
        return;
    }
    let mut descend = |value, path| visit(value, path, registry, editable, fields, depth + 1);
    match value.reflect_ref() {
        ReflectRef::Struct(value) => {
            for index in 0..value.field_len() {
                if let (Some(name), Some(field)) = (value.name_at(index), value.field_at(index)) {
                    descend(field, child_path(&path, name));
                }
            }
        }
        ReflectRef::TupleStruct(value) => {
            for (index, field) in value.iter_fields().enumerate() {
                descend(field, child_path(&path, index));
            }
        }
        ReflectRef::Tuple(value) => {
            for (index, field) in value.iter_fields().enumerate() {
                descend(field, child_path(&path, index));
            }
        }
        ReflectRef::List(value) => {
            for (index, field) in value.iter().enumerate() {
                descend(field, format!("{path}[{index}]"));
            }
        }
        ReflectRef::Array(value) => {
            for (index, field) in value.iter().enumerate() {
                descend(field, format!("{path}[{index}]"));
            }
        }
        ReflectRef::Enum(value) => {
            for index in 0..value.field_len() {
                if let Some(field) = value.field_at(index) {
                    let part = value
                        .name_at(index)
                        .map(str::to_owned)
                        .unwrap_or_else(|| index.to_string());
                    descend(field, child_path(&path, part));
                }
            }
        }
        // Reflect paths cannot address map keys or set entries; edit their RON row.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CruftCorePlugin, DespawnEntity, EditorCommand, History, SetComponentField};
    use std::collections::HashMap;

    #[derive(Reflect, Debug, PartialEq)]
    enum Mode {
        Idle,
        Moving { speed: f32 },
        Target(i32),
    }

    #[derive(Component, Reflect, Debug, PartialEq)]
    #[reflect(Component)]
    struct Settings {
        enabled: bool,
        title: String,
        count: i64,
        large: u64,
        mode: Mode,
        points: Vec<(f32, f32)>,
        array: [u8; 2],
        weights: HashMap<String, f32>,
    }

    #[derive(Component, Reflect)]
    #[reflect(Component)]
    struct Hidden(f32);

    fn fixture() -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(AppTypeRegistry::default());
        app.add_plugins(CruftCorePlugin).register_type::<Settings>();
        let entity = app
            .world_mut()
            .spawn(Settings {
                enabled: true,
                title: "Cube \"A\"\nÉtage".into(),
                count: -2,
                large: u64::MAX,
                mode: Mode::Moving { speed: 3.5 },
                points: vec![(1.0, 2.0)],
                array: [3, 4],
                weights: HashMap::from([("strength".into(), 2.0)]),
            })
            .id();
        (app, entity)
    }

    #[test]
    fn views_are_neutral_complete_and_roundtrip_through_field_commands() {
        let (mut app, entity) = fixture();
        app.world_mut().entity_mut(entity).insert(Hidden(9.0));
        let views = component_views(app.world(), entity).unwrap();
        assert_eq!(views.len(), 1); // reflected but unregistered Hidden is omitted
        let view = &views[0];
        assert_eq!(view.name, Settings::type_path());
        let field = |path: &str| view.fields.iter().find(|field| field.path == path).unwrap();
        assert_eq!(field("enabled").kind, FieldKind::Bool);
        assert_eq!(field("large").value, u64::MAX.to_string());
        assert_eq!(field("count").kind, FieldKind::SignedInteger);
        assert_eq!(field("points[0].1").kind, FieldKind::Float);
        assert_eq!(field("array[1]").value, "4");
        assert_eq!(field("weights").kind, FieldKind::Map);
        assert!(
            matches!(field("mode").kind, FieldKind::Enum { ref variants } if variants == &["Idle", "Moving", "Target"])
        );
        for field in &view.fields {
            // Bevy does not register ReflectFromReflect for tuples/arrays by default.
            // Their scalar children remain editable independently.
            if matches!(field.kind, FieldKind::Tuple | FieldKind::Array) {
                assert!(!field.editable);
                continue;
            }
            assert!(field.editable, "{} {:?}", field.path, field.error);
            let mut command = SetComponentField::new(entity, &view.name, &field.path, &field.value);
            command.apply(app.world_mut());
            assert_eq!(command.error(), None, "{}", field.path);
            command.undo(app.world_mut());
            assert_eq!(command.error(), None);
        }
        assert_eq!(component_views(app.world(), entity).unwrap(), views);
        History::execute(
            app.world_mut(),
            SetComponentField::new(entity, &view.name, "mode", "Target(17)"),
        )
        .unwrap();
        assert_eq!(
            app.world().get::<Settings>(entity).unwrap().mode,
            Mode::Target(17)
        );
        History::undo(app.world_mut()).unwrap();
        assert_eq!(
            app.world().get::<Settings>(entity).unwrap().mode,
            Mode::Moving { speed: 3.5 }
        );
    }

    #[test]
    fn hierarchy_is_read_only_and_views_resolve_restored_ids() {
        let (mut app, entity) = fixture();
        let parent = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(parent).add_child(entity);
        let before = component_views(app.world(), entity).unwrap();
        let hierarchy = before
            .iter()
            .find(|view| view.name == ChildOf::type_path())
            .unwrap();
        assert!(!hierarchy.fields.is_empty());
        assert!(hierarchy.fields.iter().all(|field| !field.editable));
        let mut delete = DespawnEntity::new(entity);
        delete.apply(app.world_mut());
        assert!(component_views(app.world(), entity).is_err());
        delete.undo(app.world_mut());
        assert_eq!(delete.error(), None);
        assert_eq!(component_views(app.world(), entity).unwrap(), before);
        assert!(component_views(&World::new(), entity).is_err());
    }

    #[test]
    fn opaque_unserializable_fields_remain_visible_but_read_only() {
        #[derive(Reflect, Clone, Debug)]
        #[reflect(opaque, Clone)]
        struct Opaque;
        #[derive(Component, Reflect)]
        #[reflect(Component)]
        struct WithOpaque {
            opaque: Opaque,
            normal: bool,
        }
        let mut app = App::new();
        app.add_plugins(CruftCorePlugin)
            .register_type::<WithOpaque>();
        let entity = app
            .world_mut()
            .spawn(WithOpaque {
                opaque: Opaque,
                normal: true,
            })
            .id();
        let views = component_views(app.world(), entity).unwrap();
        let opaque = &views[0].fields[0];
        assert_eq!(opaque.kind, FieldKind::Opaque);
        assert!(!opaque.editable);
        assert!(opaque.error.is_some());
        assert!(views[0].fields[1].editable);
    }
}
