use super::super::{OnlinePlayer, ServerCore};
use super::state::ContainerDescriptor;
use crate::PlayerId;
use crate::events::{CoreEvent, EventTarget, InventoryClickTarget, TargetedEvent};
use crate::inventory::{InventoryContainer, InventorySlot, InventoryWindowContents, ItemStack};

pub(super) fn active_window_container(
    player: &OnlinePlayer,
    window_id: u8,
) -> Option<InventoryContainer> {
    if window_id == 0 {
        Some(InventoryContainer::Player)
    } else {
        player
            .active_container
            .as_ref()
            .filter(|window| window.window_id == window_id)
            .map(|window| window.container)
    }
}

pub(super) fn window_contents(
    player: &OnlinePlayer,
    container: InventoryContainer,
) -> InventoryWindowContents {
    match container {
        InventoryContainer::Player => {
            InventoryWindowContents::player(player.snapshot.inventory.clone())
        }
        InventoryContainer::CraftingTable
        | InventoryContainer::Chest
        | InventoryContainer::Furnace => player
            .active_container
            .as_ref()
            .map(|window| window.contents(&player.snapshot.inventory))
            .unwrap_or_else(|| {
                InventoryWindowContents::with_container(
                    player.snapshot.inventory.clone(),
                    Vec::new(),
                )
            }),
    }
}

pub(super) fn resolve_inventory_target(target: &InventoryClickTarget) -> Option<InventorySlot> {
    match target {
        InventoryClickTarget::Slot(slot) => Some(*slot),
        InventoryClickTarget::Outside | InventoryClickTarget::Unsupported => None,
    }
}

pub(super) fn container_descriptor(container: InventoryContainer) -> Option<ContainerDescriptor> {
    match container {
        InventoryContainer::Player => None,
        InventoryContainer::CraftingTable => Some(ContainerDescriptor {
            local_slot_count: 10,
            main_inventory_start: 10,
            hotbar_start: 37,
        }),
        InventoryContainer::Chest => Some(ContainerDescriptor {
            local_slot_count: 27,
            main_inventory_start: 27,
            hotbar_start: 54,
        }),
        InventoryContainer::Furnace => Some(ContainerDescriptor {
            local_slot_count: 3,
            main_inventory_start: 3,
            hotbar_start: 30,
        }),
    }
}

pub(super) fn inventory_diff_events(
    window_id: u8,
    container: InventoryContainer,
    player_id: PlayerId,
    before: &InventoryWindowContents,
    after: &InventoryWindowContents,
) -> Vec<TargetedEvent> {
    visible_slots(container)
        .filter_map(|slot| {
            let before_stack = before.get_slot(slot).cloned();
            let after_stack = after.get_slot(slot).cloned();
            (before_stack != after_stack).then_some(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::InventorySlotChanged {
                    window_id,
                    container,
                    slot,
                    stack: after_stack,
                },
            })
        })
        .collect()
}

pub(super) fn property_events(
    window_id: u8,
    player_id: PlayerId,
    entries: &[(u8, i16)],
) -> Vec<TargetedEvent> {
    entries
        .iter()
        .map(|(property_id, value)| TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::ContainerPropertyChanged {
                window_id,
                property_id: *property_id,
                value: *value,
            },
        })
        .collect()
}

pub(super) fn property_diff_events(
    window_id: u8,
    player_id: PlayerId,
    before: &[(u8, i16)],
    after: &[(u8, i16)],
) -> Vec<TargetedEvent> {
    after
        .iter()
        .filter_map(|(property_id, value)| {
            let before_value = before
                .iter()
                .find(|(before_id, _)| before_id == property_id)
                .map(|(_, before_value)| *before_value);
            (before_value != Some(*value)).then_some(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::ContainerPropertyChanged {
                    window_id,
                    property_id: *property_id,
                    value: *value,
                },
            })
        })
        .collect()
}

impl ServerCore {
    pub(super) fn window_resync_events(
        player_id: PlayerId,
        window_id: u8,
        container: InventoryContainer,
        contents: &InventoryWindowContents,
        selected_hotbar_slot: u8,
        cursor: Option<&ItemStack>,
        slot: Option<InventorySlot>,
    ) -> Vec<TargetedEvent> {
        let mut events = vec![TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::InventoryContents {
                window_id,
                container,
                contents: contents.clone(),
            },
        }];

        if let Some(slot) = slot {
            events.push(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::InventorySlotChanged {
                    window_id,
                    container,
                    slot,
                    stack: contents.get_slot(slot).cloned(),
                },
            });
        }

        if container == InventoryContainer::Player {
            events.push(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::SelectedHotbarSlotChanged {
                    slot: selected_hotbar_slot,
                },
            });
        }
        events.push(TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::CursorChanged {
                stack: cursor.cloned(),
            },
        });
        events
    }
}

fn visible_slots(container: InventoryContainer) -> impl Iterator<Item = InventorySlot> {
    let local = match container {
        InventoryContainer::Player => (0_u8..9)
            .map(InventorySlot::Auxiliary)
            .chain((0_u8..27).map(InventorySlot::MainInventory))
            .chain((0_u8..9).map(InventorySlot::Hotbar))
            .chain(std::iter::once(InventorySlot::Offhand))
            .collect::<Vec<_>>(),
        InventoryContainer::CraftingTable
        | InventoryContainer::Chest
        | InventoryContainer::Furnace => {
            let descriptor = container_descriptor(container)
                .expect("non-player container should have a descriptor");
            (0_u8..descriptor.local_slot_count)
                .map(InventorySlot::Container)
                .collect::<Vec<_>>()
        }
    };
    local
        .into_iter()
        .chain((0_u8..27).map(InventorySlot::MainInventory))
        .chain((0_u8..9).map(InventorySlot::Hotbar))
        .chain(
            (container == InventoryContainer::Player)
                .then_some(InventorySlot::Offhand)
                .into_iter(),
        )
}
