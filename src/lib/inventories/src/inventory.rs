use crate::defined_slots;
use crate::errors::InventoryError;
use crate::item::ItemID;
use crate::slot::InventorySlot;
use crate::{INVENTORY_UPDATES_QUEUE, InventoryUpdate};
use bevy_ecs::prelude::{Component, Entity};
use bitcode_derive::{Decode, Encode};

#[derive(Component, Clone, Debug, Decode, Encode)]
pub struct Inventory {
    pub slots: Box<[Option<InventorySlot>]>,
}

impl Default for Inventory {
    /// Make default inventory, sized for a PLAYER.
    /// 46 = (5 * 9) + 1 =
    /// NOT divisible by 9.
    fn default() -> Self {
        Self::new(Self::DEFAULT_PLAYER_SIZE)
    }
}

impl Inventory {
    pub const DEFAULT_PLAYER_SIZE: usize = 46;
    const PLAYER_MAIN_START: usize = 9;
    const PLAYER_HOTBAR_END_EXCLUSIVE: usize = defined_slots::player::HOTBAR_SLOT_9 as usize + 1;
    const DEFAULT_STACK_LIMIT: i32 = 64;

    pub fn new(size: usize) -> Self {
        Self {
            slots: vec![None; size].into_boxed_slice(),
        }
    }

    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            *slot = None;
        }
    }

    pub fn contains_item(&self, item_id: i32) -> bool {
        self.slots.iter().any(|slot| {
            if let Some(slot) = slot {
                if let Some(item) = &slot.item_id {
                    item.0.0 == item_id
                } else {
                    false
                }
            } else {
                false
            }
        })
    }

    pub fn add_item(&mut self, item: InventorySlot) -> Result<(), InventoryError> {
        for slot in self.slots.iter_mut() {
            if slot.is_none() {
                *slot = Some(item);
                return Ok(());
            }
        }
        Err(InventoryError::InventoryFull)
    }

    pub fn add_item_with_update(
        &mut self,
        item: InventorySlot,
        entity: Entity,
    ) -> Result<(), InventoryError> {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(item.clone());
                INVENTORY_UPDATES_QUEUE.push(InventoryUpdate {
                    slot_index: index as u8,
                    slot: item,
                    entity,
                });
                return Ok(());
            }
        }
        Err(InventoryError::InventoryFull)
    }

    /// Adds an item using player inventory rules:
    /// - Never uses crafting/armor/offhand slots.
    /// - Fills an existing compatible stack first.
    /// - Falls back to first empty slot in main inventory/hotbar.
    pub fn add_item_player_with_update(
        &mut self,
        item: InventorySlot,
        entity: Entity,
    ) -> Result<(), InventoryError> {
        if item.item_id.is_none() || item.count.0 <= 0 {
            return Ok(());
        }

        let mut remaining = item.count.0;

        for index in Self::PLAYER_MAIN_START..Self::PLAYER_HOTBAR_END_EXCLUSIVE {
            let Some(existing) = self.slots[index].as_mut() else {
                continue;
            };

            if !slots_can_stack(existing, &item) {
                continue;
            }

            let free_space = Self::DEFAULT_STACK_LIMIT - existing.count.0;
            if free_space <= 0 {
                continue;
            }

            let to_add = remaining.min(free_space);
            existing.count.0 += to_add;
            remaining -= to_add;

            INVENTORY_UPDATES_QUEUE.push(InventoryUpdate {
                slot_index: index as u8,
                slot: existing.clone(),
                entity,
            });

            if remaining <= 0 {
                return Ok(());
            }
        }

        while remaining > 0 {
            let to_place = remaining.min(Self::DEFAULT_STACK_LIMIT);
            let mut new_stack = item.clone();
            new_stack.count.0 = to_place;

            let Some(index) = (Self::PLAYER_MAIN_START..Self::PLAYER_HOTBAR_END_EXCLUSIVE)
                .find(|i| self.slots[*i].is_none())
            else {
                return Err(InventoryError::InventoryFull);
            };

            self.slots[index] = Some(new_stack.clone());
            INVENTORY_UPDATES_QUEUE.push(InventoryUpdate {
                slot_index: index as u8,
                slot: new_stack,
                entity,
            });

            remaining -= to_place;
        }

        Ok(())
    }

    pub fn set_item(&mut self, index: usize, item: InventorySlot) -> Result<(), InventoryError> {
        if index >= self.slots.len() {
            return Err(InventoryError::InvalidSlotIndex(index));
        }
        self.slots[index] = Some(item);
        Ok(())
    }

    pub fn set_item_with_update(
        &mut self,
        index: usize,
        item: InventorySlot,
        entity: Entity,
    ) -> Result<(), InventoryError> {
        if index >= self.slots.len() {
            return Err(InventoryError::InvalidSlotIndex(index));
        }
        self.slots[index] = Some(item.clone());
        INVENTORY_UPDATES_QUEUE.push(InventoryUpdate {
            slot_index: index as u8,
            slot: item,
            entity,
        });
        Ok(())
    }

    pub fn get_item(&self, index: usize) -> Result<Option<&InventorySlot>, InventoryError> {
        if index >= self.slots.len() {
            return Err(InventoryError::InvalidSlotIndex(index));
        }
        Ok(self.slots[index].as_ref())
    }

    pub fn remove_item(&mut self, index: usize) -> Result<(), InventoryError> {
        if index >= self.slots.len() {
            return Err(InventoryError::InvalidSlotIndex(index));
        }
        if self.slots[index].is_none() {
            return Err(InventoryError::ItemNotFound);
        }
        self.slots[index] = None;
        Ok(())
    }

    pub fn remove_item_with_update(
        &mut self,
        index: usize,
        entity: Entity,
    ) -> Result<(), InventoryError> {
        if index >= self.slots.len() {
            return Err(InventoryError::InvalidSlotIndex(index));
        }
        if self.slots[index].is_none() {
            return Err(InventoryError::ItemNotFound);
        }
        self.slots[index] = None;
        INVENTORY_UPDATES_QUEUE.push(InventoryUpdate {
            slot_index: index as u8,
            slot: InventorySlot::default(),
            entity,
        });
        Ok(())
    }

    /// Clears an inventory slot, regardless of its current state, and sends an update.
    /// This is idempotent and will not error if the slot is already empty.
    pub fn clear_slot_with_update(
        &mut self,
        index: usize,
        entity: Entity,
    ) -> Result<(), InventoryError> {
        if index >= self.slots.len() {
            return Err(InventoryError::InvalidSlotIndex(index));
        }

        // If the slot is already empty, we don't need to do anything
        // except send the update (which is good practice).
        if self.slots[index].is_none() {
            // Fall through to send the update
        }

        // Set the server's state to empty
        self.slots[index] = None;

        // Queue the update to tell the client the slot is now empty
        INVENTORY_UPDATES_QUEUE.push(InventoryUpdate {
            slot_index: index as u8,
            slot: InventorySlot::default(), // An empty slot (count: 0)
            entity,
        });
        Ok(())
    }

    /// Searches the inventory for the first slot containing the given ItemID.
    ///
    /// Returns `Some(index)` if found, `None` otherwise.
    pub fn find_item(&self, item_id: ItemID) -> Option<usize> {
        self.slots.iter().position(|slot| match slot {
            Some(inventory_slot) => inventory_slot.item_id == Some(item_id),
            None => false,
        })
    }

    /// Swaps the contents of two slots and sends updates to the client.
    pub fn swap_slots_with_update(
        &mut self,
        index_a: usize,
        index_b: usize,
        entity: Entity,
    ) -> Result<(), InventoryError> {
        if index_a >= self.slots.len() {
            return Err(InventoryError::InvalidSlotIndex(index_a));
        }
        if index_b >= self.slots.len() {
            return Err(InventoryError::InvalidSlotIndex(index_b));
        }
        if index_a == index_b {
            return Ok(()); // Nothing to do
        }

        // Swap the slots in the server's memory
        self.slots.swap(index_a, index_b);

        // Send an update for the first slot
        INVENTORY_UPDATES_QUEUE.push(InventoryUpdate {
            slot_index: index_a as u8,
            // Clone the data that is now in slot A
            slot: self.slots[index_a].clone().unwrap_or_default(),
            entity,
        });

        // Send an update for the second slot
        INVENTORY_UPDATES_QUEUE.push(InventoryUpdate {
            slot_index: index_b as u8,
            // Clone the data that is now in slot B
            slot: self.slots[index_b].clone().unwrap_or_default(),
            entity,
        });

        Ok(())
    }
}

fn slots_can_stack(existing: &InventorySlot, incoming: &InventorySlot) -> bool {
    existing.item_id == incoming.item_id
        && existing.components_to_add_count == incoming.components_to_add_count
        && existing.components_to_remove_count == incoming.components_to_remove_count
        && existing.components_to_add == incoming.components_to_add
        && existing.components_to_remove == incoming.components_to_remove
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ItemID;
    use ferrumc_net_codec::net_types::var_int::VarInt;

    fn make_slot_with_id(id: i32) -> InventorySlot {
        InventorySlot {
            item_id: Some(ItemID(VarInt::new(id))),
            ..Default::default()
        }
    }

    #[test]
    fn test_new_inventory() {
        let inv = Inventory::new(5);
        assert_eq!(inv.slots.len(), 5);
        assert!(inv.slots.iter().all(|s| s.is_none()));
    }

    #[test]
    fn test_add_and_get_item() {
        let mut inv = Inventory::new(2);
        let slot = make_slot_with_id(1);
        assert!(inv.add_item(slot.clone()).is_ok());
        assert!(inv.get_item(0).unwrap().is_some());
        assert!(inv.get_item(1).unwrap().is_none());
    }

    #[test]
    fn test_add_item_full() {
        let mut inv = Inventory::new(1);
        let slot = make_slot_with_id(1);
        inv.add_item(slot).unwrap();
        let slot2 = make_slot_with_id(2);
        assert!(matches!(
            inv.add_item(slot2),
            Err(InventoryError::InventoryFull)
        ));
    }

    #[test]
    fn test_set_and_remove_item() {
        let mut inv = Inventory::new(1);
        let slot = make_slot_with_id(1);
        inv.set_item(0, slot).unwrap();
        assert!(inv.get_item(0).unwrap().is_some());
        inv.remove_item(0).unwrap();
        assert!(inv.get_item(0).unwrap().is_none());
    }

    #[test]
    fn test_contains_item() {
        let mut inv = Inventory::new(2);
        let slot = make_slot_with_id(42);
        inv.add_item(slot).unwrap();
        assert!(inv.contains_item(42));
        assert!(!inv.contains_item(99));
    }

    #[test]
    fn test_clear() {
        let mut inv = Inventory::new(2);
        inv.set_item(0, make_slot_with_id(1)).unwrap();
        inv.set_item(1, make_slot_with_id(2)).unwrap();
        inv.clear();
        assert!(inv.slots.iter().all(|s| s.is_none()));
    }

    #[test]
    fn test_invalid_index() {
        let mut inv = Inventory::new(1);
        assert!(matches!(
            inv.get_item(2),
            Err(InventoryError::InvalidSlotIndex(2))
        ));
        assert!(matches!(
            inv.set_item(2, make_slot_with_id(1)),
            Err(InventoryError::InvalidSlotIndex(2))
        ));
        assert!(matches!(
            inv.remove_item(2),
            Err(InventoryError::InvalidSlotIndex(2))
        ));
    }
}
