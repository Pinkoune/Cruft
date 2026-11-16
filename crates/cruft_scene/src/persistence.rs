use bevy::{
    asset::{
        AssetPath, EphemeralHandleBehavior, HandleSerializeProcessor, LoadFromPath, UntypedHandle,
    },
    ecs::{component::ComponentCloneBehavior, entity::EntityMapper, resource::IS_RESOURCE},
    prelude::*,
    reflect::{ReflectFromReflect, TypeRegistry, serde::TypedReflectSerializer},
    world_serialization::{DynamicEntity, DynamicWorld, serde::WorldDeserializer},
};
use cruft_core::{CommandError, History, resolve_entity};
use serde::de::DeserializeSeed;
use std::{
    any::TypeId,
    collections::{HashMap, HashSet},
    path::Path,
};

pub(crate) fn error(message: impl std::fmt::Display) -> CommandError {
    CommandError(message.to_string())
}

/// Serialize exactly these entities. Include every entity referenced by a
/// `#[entities]` component, including parents and children. Unregistered and
/// explicitly transient (`clone_behavior = Ignore`) components are omitted.
pub fn save_scene(
    world: &World,
    entities: impl IntoIterator<Item = Entity>,
) -> Result<String, CommandError> {
    let registry = world
        .get_resource::<AppTypeRegistry>()
        .ok_or_else(|| error("TypeRegistry absent"))?
        .read();
    let mut entities: Vec<_> = entities
        .into_iter()
        .map(|entity| resolve_entity(world, entity))
        .collect();
    entities.sort();
    entities.dedup();
    let derived = world.get_resource::<DerivedComponents>();
    let mut scene = DynamicWorld::default();
    for entity in entities {
        let entity_ref = world.get_entity(entity).map_err(error)?;
        let skipped = derived
            .map(|d| d.skipped_for(entity_ref))
            .unwrap_or_default();
        let lenient = derived.is_some_and(|d| d.is_lenient(entity_ref));
        if entity_ref.contains_id(IS_RESOURCE) {
            return Err(error("Une ressource n'est pas une entité de scène"));
        }
        let mut components = Vec::new();
        for id in entity_ref.archetype().components() {
            let info = world.components().get_info(*id).expect("live component");
            if matches!(info.clone_behavior(), ComponentCloneBehavior::Ignore)
                || info.type_id().is_some_and(|id| skipped.contains(&id))
            {
                continue;
            }
            let Some(registration) = info.type_id().and_then(|id| registry.get(id)) else {
                continue;
            };
            let Some(access) = registration.data::<ReflectComponent>() else {
                continue;
            };
            let Some(value) = access.reflect(entity_ref) else {
                continue;
            };
            // Native serialization defaults to replacing ephemeral handles with
            // defaults. Reject them explicitly instead of silently losing assets.
            let serialized = ron::to_string(&TypedReflectSerializer::with_processor(
                value.as_partial_reflect(),
                &registry,
                &HandleSerializeProcessor {
                    ephemeral_handle_behavior: EphemeralHandleBehavior::Error,
                },
            ))
            .map_err(|e| {
                format!(
                    "{} : {e}",
                    registration.type_info().type_path_table().short_path()
                )
            });
            if let Err(message) = serialized {
                if lenient {
                    warn!("Composant non sauvegardé (recréé par Bevy) : {message}");
                    continue;
                }
                return Err(error(message));
            }
            components
                .push(concrete(value.as_partial_reflect(), &registry)?.into_partial_reflect());
        }
        scene.entities.push(DynamicEntity { entity, components });
    }
    prepare(&scene, &registry)?;
    scene.serialize(&registry).map_err(error)
}

pub fn save_scene_file(
    world: &World,
    entities: impl IntoIterator<Item = Entity>,
    path: impl AsRef<Path>,
) -> Result<(), CommandError> {
    let ron = save_scene(world, entities)?;
    std::fs::write(path, ron).map_err(error)
}

/// Add a scene as one history entry. Does not replace the running editor World.
pub fn load_scene(world: &mut World, ron: &str) -> Result<(), CommandError> {
    History::execute(world, crate::LoadScene::new(ron))
}

pub fn load_scene_file(world: &mut World, path: impl AsRef<Path>) -> Result<(), CommandError> {
    let ron = std::fs::read_to_string(path).map_err(error)?;
    load_scene(world, &ron)
}

pub(crate) struct PreparedComponent {
    pub access: ReflectComponent,
    pub value: Box<dyn Reflect>,
}

pub(crate) struct PreparedEntity {
    pub source: Entity,
    pub components: Vec<PreparedComponent>,
    pub parent: Option<Entity>,
    pub children: Option<Vec<Entity>>,
}

pub(crate) fn parse(world: &World, ron: &str) -> Result<Vec<PreparedEntity>, CommandError> {
    let registry = world
        .get_resource::<AppTypeRegistry>()
        .ok_or_else(|| error("TypeRegistry absent"))?
        .read();
    let mut assets = AssetResolver {
        server: world.get_resource::<AssetServer>(),
        missing: false,
    };
    let mut deserializer = ron::Deserializer::from_str(ron).map_err(error)?;
    let scene = WorldDeserializer {
        type_registry: &registry,
        load_from_path: &mut assets,
    }
    .deserialize(&mut deserializer)
    .map_err(error)?;
    deserializer.end().map_err(error)?;
    if assets.missing {
        return Err(error(
            "AssetServer requis pour charger les chemins d'assets",
        ));
    }
    prepare(&scene, &registry)
}

struct AssetResolver<'a> {
    server: Option<&'a AssetServer>,
    missing: bool,
}

impl LoadFromPath for AssetResolver<'_> {
    fn load_from_path_erased(
        &mut self,
        type_id: TypeId,
        path: AssetPath<'static>,
    ) -> UntypedHandle {
        if let Some(mut server) = self.server {
            server.load_from_path_erased(type_id, path)
        } else {
            self.missing = true;
            // Parsing can finish, but parse() rejects the entire scene before mutation.
            UntypedHandle::default_for_type(type_id)
        }
    }
}

fn concrete(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
) -> Result<Box<dyn Reflect>, CommandError> {
    let info = value
        .get_represented_type_info()
        .ok_or_else(|| error("Composant sans type reflect"))?;
    let registration = registry
        .get(info.type_id())
        .ok_or_else(|| error("Type non enregistré"))?;
    registration
        .data::<ReflectFromReflect>()
        .and_then(|from| from.from_reflect(value))
        .ok_or_else(|| {
            error(format!(
                "Impossible de reconstruire {} via FromReflect",
                info.type_path()
            ))
        })
}

/// Validate all types, references and hierarchy before creating any entity.
fn prepare(
    scene: &DynamicWorld,
    registry: &TypeRegistry,
) -> Result<Vec<PreparedEntity>, CommandError> {
    if !scene.resources.is_empty() {
        return Err(error("Les scènes Cruft ne chargent pas de ressources"));
    }
    let ids: HashSet<_> = scene.entities.iter().map(|entity| entity.entity).collect();
    if ids.len() != scene.entities.len() || ids.contains(&Entity::PLACEHOLDER) {
        return Err(error("Identifiant d'entité dupliqué ou réservé"));
    }
    let mut validator = ReferenceValidator {
        ids: &ids,
        missing: None,
    };
    let mut prepared = Vec::new();
    for entity in &scene.entities {
        let mut components = Vec::new();
        let mut parent = None;
        let mut children = None;
        let mut types = HashSet::new();
        for value in &entity.components {
            let mut value = concrete(value.as_ref(), registry)?;
            let type_id = value.as_any().type_id();
            if !types.insert(type_id) {
                return Err(error("Composant dupliqué"));
            }
            let registration = registry
                .get(type_id)
                .ok_or_else(|| error("Type non enregistré"))?;
            // Resources are components in 0.20; never let scene input insert them.
            if registration
                .data::<bevy::ecs::reflect::ReflectResource>()
                .is_some()
            {
                return Err(error("Ressource interdite dans les composants de scène"));
            }
            let access = registration
                .data::<ReflectComponent>()
                .ok_or_else(|| error("Le type n'est pas un composant reflect"))?;
            access.map_entities(value.as_mut(), &mut validator);
            if let Some(value) = value.downcast_ref::<ChildOf>() {
                parent = Some(value.parent());
            } else if let Some(value) = value.downcast_ref::<Children>() {
                children = Some(value.iter().collect());
            } else {
                components.push(PreparedComponent {
                    access: access.clone(),
                    value,
                });
            }
        }
        prepared.push(PreparedEntity {
            source: entity.entity,
            components,
            parent,
            children,
        });
    }
    if let Some(missing) = validator.missing {
        return Err(error(format!("Référence hors de la scène : {missing:?}")));
    }
    let parents: HashMap<_, _> = prepared
        .iter()
        .map(|entity| (entity.source, entity.parent))
        .collect();
    for entity in &prepared {
        let mut visited = HashSet::new();
        let mut ancestor = Some(entity.source);
        while let Some(current) = ancestor {
            if !visited.insert(current) {
                return Err(error("Cycle dans la hiérarchie de scène"));
            }
            ancestor = *parents
                .get(&current)
                .ok_or_else(|| error("Parent hors de la scène"))?;
        }
        if let Some(children) = &entity.children {
            let unique: HashSet<_> = children.iter().copied().collect();
            if unique.len() != children.len()
                || children
                    .iter()
                    .any(|child| parents.get(child) != Some(&Some(entity.source)))
                || prepared.iter().any(|child| {
                    child.parent == Some(entity.source) && !unique.contains(&child.source)
                })
            {
                return Err(error("Liens ChildOf/Children incohérents"));
            }
        }
    }
    Ok(prepared)
}

struct ReferenceValidator<'a> {
    ids: &'a HashSet<Entity>,
    missing: Option<Entity>,
}

impl EntityMapper for ReferenceValidator<'_> {
    fn get_mapped(&mut self, source: Entity) -> Entity {
        if source != Entity::PLACEHOLDER && !self.ids.contains(&source) {
            self.missing = Some(source);
        }
        source
    }
    fn set_mapped(&mut self, _source: Entity, _target: Entity) {}
}

/// Every persistent scene entity (`SceneEntity` roots and all their
/// descendants, minus `EditorOnly`), in a stable order, ready for `save_scene`.
pub fn scene_entities(world: &World) -> Vec<Entity> {
    let mut entities: Vec<Entity> = world
        .iter_entities()
        .filter(|e| {
            e.contains::<cruft_core::SceneEntity>() && !e.contains::<cruft_core::EditorOnly>()
        })
        .map(|e| e.id())
        .collect();
    entities.sort();
    let mut index = 0;
    while index < entities.len() {
        if let Some(children) = world.get::<Children>(entities[index]) {
            for child in children.iter() {
                if !entities.contains(&child)
                    && !world.entity(child).contains::<cruft_core::EditorOnly>()
                {
                    entities.push(child);
                }
            }
        }
        index += 1;
    }
    entities
}

/// Root scene entities (no scene parent): what "Nouvelle scène" removes.
pub fn scene_roots(world: &World) -> Vec<Entity> {
    let mut roots: Vec<Entity> = world
        .iter_entities()
        .filter(|e| {
            e.contains::<cruft_core::SceneEntity>()
                && !e.contains::<cruft_core::EditorOnly>()
                && e.get::<ChildOf>().is_none_or(|parent| {
                    !world
                        .entity(parent.parent())
                        .contains::<cruft_core::SceneEntity>()
                })
        })
        .map(|e| e.id())
        .collect();
    roots.sort();
    roots
}

/// Components rebuilt from a data component (e.g. `Mesh3d` from `Primitive`).
/// They are omitted from saved scenes when the entity has the source component.
#[derive(Resource, Default)]
pub struct DerivedComponents {
    derived: Vec<(std::any::TypeId, Vec<std::any::TypeId>)>,
    lenient: Vec<std::any::TypeId>,
}

impl DerivedComponents {
    pub fn with<Source: Component>(mut self, derived: &[std::any::TypeId]) -> Self {
        self.derived
            .push((std::any::TypeId::of::<Source>(), derived.to_vec()));
        self
    }

    /// On entities with `Source`, components that cannot be serialized are
    /// skipped with a warning instead of failing the save. Use only for engine
    /// components re-inserted as required components (e.g. camera render state).
    pub fn lenient<Source: Component>(mut self) -> Self {
        self.lenient.push(std::any::TypeId::of::<Source>());
        self
    }

    fn is_lenient(&self, entity: EntityRef) -> bool {
        self.lenient.iter().any(|id| entity.contains_type_id(*id))
    }

    fn skipped_for(&self, entity: EntityRef) -> Vec<std::any::TypeId> {
        self.derived
            .iter()
            .filter(|(source, _)| entity.contains_type_id(*source))
            .flat_map(|(_, derived)| derived.iter().copied())
            .collect()
    }
}
