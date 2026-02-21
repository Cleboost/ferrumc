use crate::errors::BinaryError;
use crate::systems::item_entities::spawn_dropped_item;
use bevy_ecs::prelude::{Commands, Entity, MessageWriter, Query, Res};
use ferrumc_components::player::abilities::PlayerAbilities;
use ferrumc_core::transform::position::Position;
use ferrumc_core::transform::rotation::Rotation;
use ferrumc_core::transform::velocity::Velocity;
use ferrumc_inventories::hotbar::Hotbar;
use ferrumc_inventories::inventory::Inventory;
use ferrumc_inventories::slot::InventorySlot;
use ferrumc_messages::player_digging::*;
use ferrumc_messages::BlockBrokenEvent;

use ferrumc_net::connection::StreamWriter;
use ferrumc_net::packets::outgoing::block_change_ack::BlockChangeAck;
use ferrumc_net::packets::outgoing::block_update::BlockUpdate;
use ferrumc_net::PlayerActionReceiver;
use ferrumc_net_codec::net_types::var_int::VarInt;
use ferrumc_state::GlobalStateResource;
use ferrumc_world::{block_state_id::BlockStateId, pos::BlockPos};
use tracing::{error, warn};

pub fn handle(
    mut commands: Commands,
    receiver: Res<PlayerActionReceiver>,
    state: Res<GlobalStateResource>,
    broadcast_query: Query<(Entity, &StreamWriter)>,
    mut player_query: Query<(
        &PlayerAbilities,
        &mut Inventory,
        &Hotbar,
        &Position,
        &Rotation,
    )>,
    (mut start_dig_events, mut cancel_dig_events, mut finish_dig_events, mut block_break_events): (
        MessageWriter<PlayerStartedDigging>,
        MessageWriter<PlayerCancelledDigging>,
        MessageWriter<PlayerFinishedDigging>,
        MessageWriter<BlockBrokenEvent>,
    ),
) {
    // https://minecraft.wiki/w/Minecraft_Wiki:Projects/wiki.vg_merge/Protocol?oldid=2773393#Player_Action
    for (event, trigger_eid) in receiver.0.try_iter() {
        let Ok((abilities, mut inventory, hotbar, position, rotation)) =
            player_query.get_mut(trigger_eid)
        else {
            warn!(
                "PlayerAction: Player {:?} missing required components",
                trigger_eid
            );
            continue;
        };

        let pos: BlockPos = event.location.clone().into();
        match event.status.0 {
            0 if abilities.creative_mode => {
                let res: Result<(), BinaryError> = try {
                    let mut chunk = ferrumc_utils::world::load_or_generate_mut(
                        &state.0,
                        pos.chunk(),
                        "overworld",
                    )
                    .expect("Failed to load or generate chunk");
                    chunk.set_block(pos.chunk_block_pos(), BlockStateId::default());

                    block_break_events.write(BlockBrokenEvent { position: pos });

                    for (eid, conn) in &broadcast_query {
                        if !state.0.players.is_connected(eid) {
                            continue;
                        }

                        let block_update_packet = BlockUpdate {
                            location: event.location.clone(),
                            block_state_id: VarInt::from(BlockStateId::default()),
                        };
                        conn.send_packet_ref(&block_update_packet)
                            .map_err(BinaryError::Net)?;

                        if eid == trigger_eid {
                            let ack_packet = BlockChangeAck {
                                sequence: event.sequence,
                            };
                            conn.send_packet_ref(&ack_packet)
                                .map_err(BinaryError::Net)?;
                        }
                    }
                };

                if let Err(err) = res {
                    error!("Error handling creative player action: {:?}", err);
                }
            }
            0 => {
                start_dig_events.write(PlayerStartedDigging {
                    player: trigger_eid,
                    position: event.location,
                    sequence: event.sequence,
                });
            }
            1 if !abilities.creative_mode => {
                cancel_dig_events.write(PlayerCancelledDigging {
                    player: trigger_eid,
                    sequence: event.sequence,
                });
            }
            2 if !abilities.creative_mode => {
                finish_dig_events.write(PlayerFinishedDigging {
                    player: trigger_eid,
                    position: event.location,
                    sequence: event.sequence,
                });
            }
            // Drop entire stack
            3 => {
                if let Some(slot) =
                    drop_selected_hotbar_item(&mut inventory, hotbar, trigger_eid, true)
                {
                    let (drop_position, drop_velocity) =
                        build_drop_position_and_velocity(position, rotation);
                    spawn_dropped_item(&mut commands, drop_position, drop_velocity, slot);
                }
            }
            // Drop single item
            4 => {
                if let Some(slot) =
                    drop_selected_hotbar_item(&mut inventory, hotbar, trigger_eid, false)
                {
                    let (drop_position, drop_velocity) =
                        build_drop_position_and_velocity(position, rotation);
                    spawn_dropped_item(&mut commands, drop_position, drop_velocity, slot);
                }
            }
            _ => {}
        }
    }
}

fn drop_selected_hotbar_item(
    inventory: &mut Inventory,
    hotbar: &Hotbar,
    player: Entity,
    drop_entire_stack: bool,
) -> Option<InventorySlot> {
    let selected_index = hotbar.get_selected_inventory_index();
    let selected_slot = match inventory.get_item(selected_index) {
        Ok(Some(slot)) => slot.clone(),
        Ok(None) => return None,
        Err(err) => {
            warn!(
                "Failed to read selected hotbar slot {}: {:?}",
                selected_index, err
            );
            return None;
        }
    };

    if selected_slot.item_id.is_none() || selected_slot.count.0 <= 0 {
        return None;
    }

    if drop_entire_stack || selected_slot.count.0 <= 1 {
        if let Err(err) = inventory.clear_slot_with_update(selected_index, player) {
            warn!(
                "Failed to clear dropped hotbar slot {}: {:?}",
                selected_index, err
            );
            return None;
        }
        return Some(selected_slot);
    }

    let mut dropped_slot = selected_slot.clone();
    dropped_slot.count = VarInt::new(1);

    let mut remaining_slot = selected_slot;
    remaining_slot.count = VarInt::new(remaining_slot.count.0 - 1);

    if let Err(err) = inventory.set_item_with_update(selected_index, remaining_slot, player) {
        warn!(
            "Failed to update hotbar slot {} after dropping item: {:?}",
            selected_index, err
        );
        return None;
    }

    Some(dropped_slot)
}

fn build_drop_position_and_velocity(
    position: &Position,
    rotation: &Rotation,
) -> (Position, Velocity) {
    let yaw = rotation.yaw.to_radians();
    let pitch = rotation.pitch.to_radians();

    let forward_x = -yaw.sin();
    let forward_z = yaw.cos();
    let forward_y = -pitch.sin();

    let drop_position = Position::new(
        position.x + (forward_x as f64 * 0.4),
        position.y + 1.35,
        position.z + (forward_z as f64 * 0.4),
    );

    let drop_velocity = Velocity::new(
        (forward_x * 0.28) as f64,
        (0.15 + forward_y * 0.08) as f64,
        (forward_z * 0.28) as f64,
    );

    (drop_position, drop_velocity)
}
