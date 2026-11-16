//! JSON <-> reflect helpers shared by the MCP server and agent proposals.
use crate::{CommandError, failure};
use bevy::{
    prelude::*,
    reflect::{
        ReflectFromReflect, TypeRegistration, TypeRegistry,
        serde::{TypedReflectDeserializer, TypedReflectSerializer},
        std_traits::ReflectDefault,
    },
};
use serde::de::DeserializeSeed;
use serde_json::Value;

/// Parses an entity ID as produced by `Entity::to_bits` (decimal string or number).
pub fn parse_entity(value: &Value) -> Result<Entity, CommandError> {
    let bits = match value {
        Value::String(text) => text
            .parse::<u64>()
            .map_err(|_| failure(format!("ID d'entité invalide : {text}")))?,
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| failure("ID d'entité invalide"))?,
        _ => return Err(failure("ID d'entité attendu (chaîne décimale)")),
    };
    Entity::try_from_bits(bits).ok_or_else(|| failure(format!("ID d'entité invalide : {bits}")))
}

pub fn entity_id(entity: Entity) -> String {
    entity.to_bits().to_string()
}

/// Finds a registered component by full type path, then by unambiguous short path.
pub fn resolve_component<'a>(
    registry: &'a TypeRegistry,
    name: &str,
) -> Result<&'a TypeRegistration, CommandError> {
    let registration = registry
        .get_with_type_path(name)
        .or_else(|| registry.get_with_short_type_path(name))
        .ok_or_else(|| failure(format!("Composant inconnu ou ambigu : {name}")))?;
    if registration.data::<ReflectComponent>().is_none() {
        return Err(failure(format!("{name} n'est pas un composant reflect")));
    }
    Ok(registration)
}

/// Serializes any reflected value with its registered serde representation.
pub fn reflect_to_json(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
) -> Result<Value, CommandError> {
    serde_json::to_value(TypedReflectSerializer::new(value, registry))
        .map_err(|error| failure(error.to_string()))
}

/// Builds a concrete component from JSON. When the type has `ReflectDefault`,
/// the JSON is merged over the default value so partial objects are accepted
/// (e.g. `{"translation": [0, 1, 0]}` for a Transform).
pub fn component_from_json(
    registry: &TypeRegistry,
    registration: &TypeRegistration,
    json: &Value,
) -> Result<Box<dyn Reflect>, CommandError> {
    let mut merged = json.clone();
    if let Some(default) = registration.data::<ReflectDefault>() {
        let base = default.default();
        let mut base = reflect_to_json(base.as_partial_reflect(), registry)?;
        merge_json(&mut base, json);
        merged = base;
    }
    let value = TypedReflectDeserializer::new(registration, registry)
        .deserialize(merged)
        .map_err(|error| {
            failure(format!(
                "{} : {error}",
                registration.type_info().type_path_table().short_path()
            ))
        })?;
    registration
        .data::<ReflectFromReflect>()
        .and_then(|from| from.from_reflect(value.as_ref()))
        .ok_or_else(|| failure("Impossible de reconstruire le composant"))
}

pub fn reflect_to_ron(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
) -> Result<String, CommandError> {
    ron::to_string(&TypedReflectSerializer::new(value, registry))
        .map_err(|error| failure(error.to_string()))
}

fn merge_json(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            for (key, value) in patch {
                match base.get_mut(key) {
                    Some(existing) => merge_json(existing, value),
                    None => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn partial_transform_json_is_merged_over_default() {
        let mut registry = TypeRegistry::default();
        registry.register::<Transform>();
        let registration = resolve_component(&registry, "Transform").unwrap();
        let value = component_from_json(
            &registry,
            registration,
            &json!({"translation": [1.0, 2.0, 3.0]}),
        )
        .unwrap();
        let transform = value.downcast_ref::<Transform>().unwrap();
        assert_eq!(transform.translation, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(transform.scale, Vec3::ONE);
        assert!(
            component_from_json(&registry, registration, &json!({"translation": "x"})).is_err()
        );
        assert!(resolve_component(&registry, "Nope").is_err());
    }

    #[test]
    fn entity_ids_roundtrip() {
        let entity = Entity::from_raw_u32(7).unwrap();
        assert_eq!(parse_entity(&json!(entity_id(entity))).unwrap(), entity);
        assert!(parse_entity(&json!("abc")).is_err());
    }
}
