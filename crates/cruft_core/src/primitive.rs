use crate::{CommandError, failure};
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect)]
#[reflect(Default, Debug, PartialEq, Hash)]
pub enum PrimitiveShape {
    #[default]
    Cube,
    Sphere,
    Plane,
    Capsule,
    Cylinder,
}

impl PrimitiveShape {
    pub const ALL: [&'static str; 5] = ["cube", "sphere", "plane", "capsule", "cylinder"];

    pub fn parse(name: &str) -> Result<Self, CommandError> {
        Ok(match name.to_ascii_lowercase().as_str() {
            "cube" => Self::Cube,
            "sphere" => Self::Sphere,
            "plane" => Self::Plane,
            "capsule" => Self::Capsule,
            "cylinder" => Self::Cylinder,
            other => return Err(failure(format!("Primitive inconnue : {other}"))),
        })
    }
}

/// Serializable description of a built-in mesh (unit size; use the Transform
/// scale). The editor derives `Mesh3d` + material from it, so scenes using
/// primitives can be saved without ephemeral asset handles.
#[derive(Component, Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Debug, PartialEq)]
pub struct Primitive {
    pub shape: PrimitiveShape,
    /// sRGB base color, 0..1.
    pub color: Vec3,
}

impl Default for Primitive {
    fn default() -> Self {
        Self {
            shape: PrimitiveShape::Cube,
            color: Vec3::new(0.64, 0.69, 0.71),
        }
    }
}

impl Primitive {
    pub fn new(shape: PrimitiveShape, color: Vec3) -> Self {
        Self { shape, color }
    }
}
