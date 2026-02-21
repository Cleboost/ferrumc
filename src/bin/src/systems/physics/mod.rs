use super::item_entities;
use bevy_ecs::schedule::IntoScheduleConfigs;
pub mod collisions;
pub mod drag;
pub mod gravity;
pub mod unground;
pub mod velocity;

pub fn register_physics(schedule: &mut bevy_ecs::schedule::Schedule) {
    schedule.add_systems(
        (
            unground::handle,
            gravity::handle,
            drag::handle,
            velocity::handle,
            collisions::handle,
            item_entities::pickup_dropped_items,
        )
            .chain(),
    );
}
