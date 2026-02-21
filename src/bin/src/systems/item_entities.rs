use bevy_ecs::prelude::{Commands, Component, Entity, Query, With, World};
use ferrumc_core::identity::entity_identity::EntityIdentity;
use ferrumc_core::identity::player_identity::PlayerIdentity;
use ferrumc_core::transform::grounded::OnGround;
use ferrumc_core::transform::position::Position;
use ferrumc_core::transform::rotation::Rotation;
use ferrumc_core::transform::velocity::Velocity;
use ferrumc_data::generated::entities::EntityType as VanillaEntityType;
use ferrumc_entities::components::{
    CombatProperties, EntityMetadata as EntityTypeMetadata, LastSyncedPosition, SpawnProperties,
};
use ferrumc_entities::markers::{HasCollisions, HasGravity};
use ferrumc_inventories::inventory::Inventory;
use ferrumc_inventories::slot::InventorySlot;
use ferrumc_net::connection::StreamWriter;
use ferrumc_net::packets::outgoing::entity_metadata::{
    EntityMetadata as MetadataEntry, EntityMetadataPacket,
};
use ferrumc_net::packets::outgoing::remove_entities::RemoveEntitiesPacket;
use ferrumc_net::packets::outgoing::spawn_entity::SpawnEntityPacket;
use ferrumc_net_codec::net_types::length_prefixed_vec::LengthPrefixedVec;
use ferrumc_net_codec::net_types::var_int::VarInt;
use tracing::{debug, warn};

const DEFAULT_PICKUP_DELAY_TICKS: u16 = 10;
const ITEM_PICKUP_RADIUS_SQUARED: f64 = 2.25;

#[derive(Component, Debug, Clone)]
pub struct DroppedItem {
    pub slot: InventorySlot,
    pub pickup_delay_ticks: u16,
}

pub fn spawn_dropped_item(
    commands: &mut Commands,
    position: Position,
    velocity: Velocity,
    slot: InventorySlot,
) {
    if slot.item_id.is_none() || slot.count.0 <= 0 {
        return;
    }

    let metadata = EntityTypeMetadata::from_vanilla(&VanillaEntityType::ITEM);
    let combat = CombatProperties::from_metadata(&metadata);
    let spawn = SpawnProperties::from_metadata(&metadata);
    let last_synced_position = LastSyncedPosition::from_position(&position);

    let entity = commands
        .spawn((
            EntityIdentity::new(),
            metadata,
            combat,
            spawn,
            position,
            Rotation::default(),
            velocity,
            OnGround(false),
            last_synced_position,
            HasGravity,
            HasCollisions,
            DroppedItem {
                slot,
                pickup_delay_ticks: DEFAULT_PICKUP_DELAY_TICKS,
            },
        ))
        .id();

    commands.queue(move |world: &mut World| {
        broadcast_item_spawn(world, entity);
    });
}

pub fn pickup_dropped_items(
    mut commands: Commands,
    mut item_query: Query<(Entity, &Position, &EntityIdentity, &mut DroppedItem)>,
    mut player_query: Query<(Entity, &Position, &mut Inventory), With<PlayerIdentity>>,
    writer_query: Query<&StreamWriter>,
) {
    let mut entities_to_remove = Vec::new();

    for (item_entity, item_position, identity, mut dropped_item) in &mut item_query {
        if dropped_item.pickup_delay_ticks > 0 {
            dropped_item.pickup_delay_ticks -= 1;
            continue;
        }

        for (player_entity, player_position, mut inventory) in &mut player_query {
            if player_position
                .coords
                .distance_squared(item_position.coords)
                > ITEM_PICKUP_RADIUS_SQUARED
            {
                continue;
            }

            if inventory
                .add_item_player_with_update(dropped_item.slot.clone(), player_entity)
                .is_ok()
            {
                entities_to_remove.push((item_entity, identity.entity_id));
                break;
            }
        }
    }

    for (item_entity, entity_id) in entities_to_remove {
        commands.entity(item_entity).despawn();
        broadcast_item_remove(&writer_query, entity_id);
    }
}

fn broadcast_item_spawn(world: &mut World, entity: Entity) {
    let Some(metadata) = world.get::<EntityTypeMetadata>(entity) else {
        warn!("Cannot spawn dropped item {:?}: missing metadata", entity);
        return;
    };

    let Some(identity) = world.get::<EntityIdentity>(entity) else {
        warn!("Cannot spawn dropped item {:?}: missing identity", entity);
        return;
    };

    let Some(position) = world.get::<Position>(entity) else {
        warn!("Cannot spawn dropped item {:?}: missing position", entity);
        return;
    };

    let Some(rotation) = world.get::<Rotation>(entity) else {
        warn!("Cannot spawn dropped item {:?}: missing rotation", entity);
        return;
    };

    let Some(velocity) = world.get::<Velocity>(entity) else {
        warn!("Cannot spawn dropped item {:?}: missing velocity", entity);
        return;
    };

    let Some(dropped_item) = world.get::<DroppedItem>(entity) else {
        warn!("Cannot spawn dropped item {:?}: missing slot data", entity);
        return;
    };

    let spawn_packet = SpawnEntityPacket::new_with_velocity(
        identity.entity_id,
        identity.uuid.as_u128(),
        metadata.protocol_id() as i32,
        position,
        rotation,
        velocity,
    );

    let metadata_packet = EntityMetadataPacket::new(
        VarInt::new(identity.entity_id),
        [MetadataEntry::item_stack(dropped_item.slot.clone())],
    );

    let mut writer_query = world.query::<&StreamWriter>();
    for writer in writer_query.iter(world) {
        if let Err(err) = writer.send_packet_ref(&spawn_packet) {
            debug!("Failed to send dropped item spawn packet: {:?}", err);
            continue;
        }

        if let Err(err) = writer.send_packet_ref(&metadata_packet) {
            debug!("Failed to send dropped item metadata packet: {:?}", err);
        }
    }
}

fn broadcast_item_remove(writer_query: &Query<&StreamWriter>, entity_id: i32) {
    let packet = RemoveEntitiesPacket {
        entity_ids: LengthPrefixedVec::new(vec![VarInt::new(entity_id)]),
    };

    for writer in writer_query.iter() {
        if let Err(err) = writer.send_packet_ref(&packet) {
            debug!("Failed to send dropped item remove packet: {:?}", err);
        }
    }
}
