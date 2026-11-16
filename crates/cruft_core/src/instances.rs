//! Reconcile reflected instance snapshots with Bevy's asynchronous scene loader.
//! A restored subtree already contains the user's edits. The loader's next
//! generated copy is discarded; a later explicit source change replaces it.
use bevy::{
    prelude::*,
    world_serialization::{
        WorldAssetRoot, WorldInstance, WorldInstanceReady, WorldInstanceSpawner,
    },
};

#[derive(Component)]
#[component(clone_behavior = Ignore)]
pub(crate) struct RestoredInstance {
    awaiting_loader: bool,
    children: Vec<Entity>,
}

pub fn instance_is_loaded(world: &World, entity: Entity) -> bool {
    world.get::<RestoredInstance>(entity).is_some()
        || world.get::<WorldInstance>(entity).is_some_and(|instance| {
            world
                .get_resource::<WorldInstanceSpawner>()
                .is_some_and(|spawner| spawner.instance_is_ready(**instance))
        })
}

/// Call after restoring a root's components and complete hierarchy. This keeps
/// the restored entity IDs (and therefore history references) authoritative.
pub fn preserve_restored_instance(world: &mut World, root: Entity) {
    let Some(source) = world.get::<WorldAssetRoot>(root).cloned() else {
        return;
    };
    let old = world.get::<WorldInstance>(root).map(|instance| **instance);
    if let Some(old) = old
        && let Some(mut spawner) = world.get_resource_mut::<WorldInstanceSpawner>()
    {
        spawner.unregister_instance(old);
    }
    let children = world
        .get::<Children>(root)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    // Run the native root removal hook while WorldInstance is still present,
    // so its asset-to-instance bookkeeping does not grow on every Play/Stop.
    world
        .entity_mut(root)
        .remove::<WorldAssetRoot>()
        .remove::<WorldInstance>()
        .insert((
            source,
            RestoredInstance {
                awaiting_loader: true,
                children,
            },
        ));
}

pub(crate) fn instance_ready(event: On<WorldInstanceReady>, mut commands: Commands) {
    let root = event.entity;
    let instance_id = event.instance_id;
    commands.queue(move |world: &mut World| {
        if world.get::<RestoredInstance>(root).is_none() {
            return;
        }
        let generated = if let Some(mut spawner) = world.get_resource_mut::<WorldInstanceSpawner>()
        {
            let generated: Vec<_> = spawner.iter_instance_entities(instance_id).collect();
            spawner.unregister_instance(instance_id);
            generated
        } else {
            return;
        };
        for entity in generated {
            if world.get_entity(entity).is_ok() {
                world.despawn(entity);
            }
        }
        if let Ok(mut root) = world.get_entity_mut(root) {
            // Retain the native ID until root removal so Bevy can remove its
            // asset-index entry, even though this generated copy is unregistered.
            if let Some(mut instance) = root.get_mut::<RestoredInstance>() {
                instance.awaiting_loader = false;
            }
        }
    });
}

pub(crate) fn reload_restored_instances(
    mut commands: Commands,
    changed: Query<(Entity, &RestoredInstance), Changed<WorldAssetRoot>>,
) {
    for (root, instance) in &changed {
        if instance.awaiting_loader {
            continue;
        }
        for child in &instance.children {
            commands.entity(*child).try_despawn();
        }
        commands.entity(root).remove::<RestoredInstance>();
    }
}
