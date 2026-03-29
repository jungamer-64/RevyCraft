use super::super::PlayerSessionState;
use crate::PlayerId;
use crate::events::{CoreEvent, EventTarget, InventoryClickTarget, TargetedEvent};
use crate::inventory::{InventorySlot, InventoryWindowContents, ItemStack, PlayerInventory};
use revy_voxel_rules::{ContainerKindId, ContainerPropertyKey};

pub(super) fn active_window_container(
    content_behavior: &dyn revy_voxel_rules::ContentBehavior,
    session: &PlayerSessionState,
    window_id: u8,
) -> Option<ContainerKindId> {
    if window_id == 0 {
        Some(content_behavior.player_container_kind())
    } else {
        session
            .active_container
            .as_ref()
            .filter(|window| window.window_id == window_id)
            .map(|window| window.container.kind.clone())
    }
}

pub(super) fn window_contents(
    content_behavior: &dyn revy_voxel_rules::ContentBehavior,
    session: &PlayerSessionState,
    inventory: &PlayerInventory,
    container: &ContainerKindId,
) -> InventoryWindowContents {
    if *container == content_behavior.player_container_kind() {
        InventoryWindowContents::player(inventory.clone())
    } else {
        session
            .active_container
            .as_ref()
            .filter(|window| window.container.kind == *container)
            .map(|window| window.contents(inventory))
            .unwrap_or_else(|| {
                InventoryWindowContents::with_local_slots(inventory.clone(), Vec::new())
            })
    }
}

pub(super) fn resolve_inventory_target(target: &InventoryClickTarget) -> Option<InventorySlot> {
    match target {
        InventoryClickTarget::Slot(slot) => Some(*slot),
        InventoryClickTarget::Outside | InventoryClickTarget::Unsupported => None,
    }
}

pub(in crate::core) fn inventory_diff_events(
    content_behavior: &dyn revy_voxel_rules::ContentBehavior,
    window_id: u8,
    container: &ContainerKindId,
    player_id: PlayerId,
    before: &InventoryWindowContents,
    after: &InventoryWindowContents,
) -> Vec<TargetedEvent> {
    visible_slots(content_behavior, container)
        .into_iter()
        .filter_map(|slot| {
            let before_stack = before.get_slot(slot).cloned();
            let after_stack = after.get_slot(slot).cloned();
            (before_stack != after_stack).then_some(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::InventorySlotChanged {
                    window_id,
                    container: container.clone(),
                    slot,
                    stack: after_stack,
                },
            })
        })
        .collect()
}

pub(in crate::core) fn property_events(
    window_id: u8,
    player_id: PlayerId,
    entries: &[(ContainerPropertyKey, i16)],
) -> Vec<TargetedEvent> {
    entries
        .iter()
        .map(|(property, value)| TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::ContainerPropertyChanged {
                window_id,
                property: property.clone(),
                value: *value,
            },
        })
        .collect()
}

pub(in crate::core) fn property_diff_events(
    window_id: u8,
    player_id: PlayerId,
    before: &[(ContainerPropertyKey, i16)],
    after: &[(ContainerPropertyKey, i16)],
) -> Vec<TargetedEvent> {
    after
        .iter()
        .filter_map(|(property, value)| {
            let before_value = before
                .iter()
                .find(|(before_property, _)| before_property == property)
                .map(|(_, before_value)| *before_value);
            (before_value != Some(*value)).then_some(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::ContainerPropertyChanged {
                    window_id,
                    property: property.clone(),
                    value: *value,
                },
            })
        })
        .collect()
}

pub(in crate::core) fn window_resync_events(
    content_behavior: &dyn revy_voxel_rules::ContentBehavior,
    player_id: PlayerId,
    window_id: u8,
    container: &ContainerKindId,
    contents: &InventoryWindowContents,
    selected_hotbar_slot: u8,
    cursor: Option<&ItemStack>,
    slot: Option<InventorySlot>,
) -> Vec<TargetedEvent> {
    let mut events = vec![TargetedEvent {
        target: EventTarget::Player(player_id),
        event: CoreEvent::InventoryContents {
            window_id,
            container: container.clone(),
            contents: contents.clone(),
        },
    }];

    if let Some(slot) = slot {
        events.push(TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::InventorySlotChanged {
                window_id,
                container: container.clone(),
                slot,
                stack: contents.get_slot(slot).cloned(),
            },
        });
    }

    if *container == content_behavior.player_container_kind() {
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

fn visible_slots(
    content_behavior: &dyn revy_voxel_rules::ContentBehavior,
    container: &ContainerKindId,
) -> Vec<InventorySlot> {
    let mut slots = Vec::new();
    let is_player = *container == content_behavior.player_container_kind();
    if let Some(spec) = content_behavior.container_spec(container) {
        slots.extend((0_u16..spec.local_slot_count).map(InventorySlot::WindowLocal));
    }
    slots.extend((0_u8..27).map(InventorySlot::MainInventory));
    slots.extend((0_u8..9).map(InventorySlot::Hotbar));
    if is_player {
        slots.push(InventorySlot::Offhand);
    }
    slots
}
