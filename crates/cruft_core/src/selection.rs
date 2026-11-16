use bevy::prelude::*;

/// Ordered, duplicate-free multi-selection; the primary entity always belongs to it.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    primary: Option<Entity>,
    entities: Vec<Entity>,
}

impl Selection {
    pub fn primary(&self) -> Option<Entity> {
        self.primary
    }
    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub fn select_only(&mut self, entity: Entity) {
        self.entities.clear();
        self.entities.push(entity);
        self.primary = Some(entity);
    }

    pub fn add(&mut self, entity: Entity) {
        if !self.entities.contains(&entity) {
            self.entities.push(entity);
        }
        self.primary = Some(entity);
    }

    pub fn remove(&mut self, entity: Entity) {
        self.entities.retain(|selected| *selected != entity);
        if self.primary == Some(entity) {
            self.primary = self.entities.last().copied();
        }
    }

    pub fn clear(&mut self) {
        self.entities.clear();
        self.primary = None;
    }

    pub(crate) fn remap(&mut self, mut map: impl FnMut(Entity) -> Entity) {
        for entity in &mut self.entities {
            *entity = map(*entity);
        }
        self.primary = self.primary.map(map);
    }
}
