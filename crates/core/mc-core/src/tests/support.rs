pub(super) use crate::*;

use uuid::Uuid;

pub(super) fn player_id(name: &str) -> PlayerId {
    PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, name.as_bytes()))
}

pub(super) fn item(key: &str, count: u8) -> ItemStack {
    ItemStack::new(key, count, 0)
}

pub(super) fn login_player(
    core: &mut ServerCore,
    connection_id: u64,
    name: &str,
) -> (PlayerId, Vec<TargetedEvent>) {
    let player_id = player_id(name);
    let events = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(connection_id),
            username: name.to_string(),
            player_id,
        },
        0,
    );
    (player_id, events)
}

pub(super) fn logged_in_core(
    config: CoreConfig,
    connection_id: u64,
    name: &str,
) -> (ServerCore, PlayerId) {
    let mut core = ServerCore::new(config);
    let (player_id, _) = login_player(&mut core, connection_id, name);
    (core, player_id)
}

pub(super) fn logged_in_creative_core(name: &str) -> (ServerCore, PlayerId) {
    logged_in_core(
        CoreConfig {
            game_mode: 1,
            ..CoreConfig::default()
        },
        1,
        name,
    )
}

pub(super) fn creative_inventory_set(
    core: &mut ServerCore,
    player_id: PlayerId,
    slot: InventorySlot,
    stack: Option<ItemStack>,
) -> Vec<TargetedEvent> {
    core.apply_command(
        CoreCommand::CreativeInventorySet {
            player_id,
            slot,
            stack,
        },
        0,
    )
}

pub(super) fn set_held_slot(
    core: &mut ServerCore,
    player_id: PlayerId,
    slot: i16,
) -> Vec<TargetedEvent> {
    core.apply_command(CoreCommand::SetHeldSlot { player_id, slot }, 0)
}

pub(super) fn click_slot(
    core: &mut ServerCore,
    player_id: PlayerId,
    window_id: u8,
    action_number: i16,
    slot: InventorySlot,
    button: InventoryClickButton,
    clicked_item: Option<ItemStack>,
) -> Vec<TargetedEvent> {
    core.apply_command(
        CoreCommand::InventoryClick {
            player_id,
            transaction: InventoryTransactionContext {
                window_id,
                action_number,
            },
            target: InventoryClickTarget::Slot(slot),
            button,
            validation: InventoryClickValidation::StrictSlotEcho { clicked_item },
        },
        0,
    )
}

pub(super) fn bedrock_session_capabilities() -> SessionCapabilitySet {
    let mut session = canonical_session_capabilities();
    session.protocol.insert(ProtocolCapability::Bedrock);
    session.protocol.insert(ProtocolCapability::Bedrock924);
    session
}

pub(super) fn apply_with_session(
    core: &mut ServerCore,
    command: CoreCommand,
    session: &SessionCapabilitySet,
) -> Vec<TargetedEvent> {
    core.apply_command_with_policy(command, 0, Some(session), &CanonicalGameplayPolicy)
        .expect("test command should apply with the provided session")
}

pub(super) fn craft_input(index: u8) -> InventorySlot {
    InventorySlot::crafting_input(index).expect("craft input should exist")
}

pub(super) fn online_player(core: &ServerCore, player_id: PlayerId) -> &crate::core::OnlinePlayer {
    core.online_players
        .get(&player_id)
        .expect("player should still be online")
}

pub(super) fn stack_summary(stack: &ItemStack) -> (&str, u8) {
    (stack.key.as_str(), stack.count)
}

#[track_caller]
pub(super) fn assert_connection_event<F>(
    events: &[TargetedEvent],
    connection_id: ConnectionId,
    predicate: F,
) where
    F: Fn(&CoreEvent) -> bool,
{
    assert!(events.iter().any(|event| {
        matches!(event.target, EventTarget::Connection(id) if id == connection_id)
            && predicate(&event.event)
    }));
}

#[track_caller]
pub(super) fn assert_player_event<F>(events: &[TargetedEvent], player_id: PlayerId, predicate: F)
where
    F: Fn(&CoreEvent) -> bool,
{
    assert!(events.iter().any(|event| {
        matches!(event.target, EventTarget::Player(id) if id == player_id)
            && predicate(&event.event)
    }));
}

#[track_caller]
pub(super) fn assert_everyone_except_event<F>(
    events: &[TargetedEvent],
    player_id: PlayerId,
    predicate: F,
) where
    F: Fn(&CoreEvent) -> bool,
{
    assert!(events.iter().any(|event| {
        matches!(event.target, EventTarget::EveryoneExcept(id) if id == player_id)
            && predicate(&event.event)
    }));
}

pub(super) fn count_player_events<F>(
    events: &[TargetedEvent],
    player_id: PlayerId,
    predicate: F,
) -> usize
where
    F: Fn(&CoreEvent) -> bool,
{
    events
        .iter()
        .filter(|event| {
            matches!(event.target, EventTarget::Player(id) if id == player_id)
                && predicate(&event.event)
        })
        .count()
}

#[track_caller]
pub(super) fn assert_transaction_processed(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
    action_number: i16,
    accepted: bool,
) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::InventoryTransactionProcessed {
                transaction,
                accepted: event_accepted,
            } if *event_accepted == accepted
                && *transaction == InventoryTransactionContext {
                    window_id,
                    action_number,
                }
        )
    });
}

#[track_caller]
pub(super) fn assert_inventory_slot_changed_in_window(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
    slot: InventorySlot,
) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::InventorySlotChanged {
                window_id: event_window_id,
                slot: event_slot,
                ..
            } if *event_window_id == window_id && *event_slot == slot
        )
    });
}

#[track_caller]
pub(super) fn assert_inventory_slot_changed_to(
    events: &[TargetedEvent],
    player_id: PlayerId,
    slot: InventorySlot,
    expected: Option<(&str, u8)>,
) {
    assert_player_event(events, player_id, |event| match event {
        CoreEvent::InventorySlotChanged {
            slot: event_slot,
            stack,
            ..
        } if *event_slot == slot => stack.as_ref().map(stack_summary) == expected,
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_inventory_slot_changed_in_window_to(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
    slot: InventorySlot,
    expected: Option<(&str, u8)>,
) {
    assert_player_event(events, player_id, |event| match event {
        CoreEvent::InventorySlotChanged {
            window_id: event_window_id,
            slot: event_slot,
            stack,
            ..
        } if *event_window_id == window_id && *event_slot == slot => {
            stack.as_ref().map(stack_summary) == expected
        }
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_cursor_changed_to(
    events: &[TargetedEvent],
    player_id: PlayerId,
    key: &str,
    count: u8,
) {
    assert_player_event(events, player_id, |event| match event {
        CoreEvent::CursorChanged { stack } => {
            stack.as_ref().map(stack_summary) == Some((key, count))
        }
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_player_window_contents(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
    container: InventoryContainer,
) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::InventoryContents {
                window_id: event_window_id,
                container: event_container,
                ..
            } if *event_window_id == window_id && *event_container == container
        )
    });
}

#[track_caller]
pub(super) fn assert_container_opened(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
    container: InventoryContainer,
) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::ContainerOpened {
                window_id: event_window_id,
                container: event_container,
                ..
            } if *event_window_id == window_id && *event_container == container
        )
    });
}

#[track_caller]
pub(super) fn assert_container_closed(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::ContainerClosed {
                window_id: event_window_id,
            } if *event_window_id == window_id
        )
    });
}

#[track_caller]
pub(super) fn assert_container_property_changed(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
    property_id: u8,
    value: i16,
) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::ContainerPropertyChanged {
                window_id: event_window_id,
                property_id: event_property_id,
                value: event_value,
            } if *event_window_id == window_id
                && *event_property_id == property_id
                && *event_value == value
        )
    });
}

#[track_caller]
pub(super) fn assert_connection_inventory_contents(
    events: &[TargetedEvent],
    connection_id: ConnectionId,
) {
    assert_connection_event(events, connection_id, |event| {
        matches!(
            event,
            CoreEvent::InventoryContents {
                container: InventoryContainer::Player,
                ..
            }
        )
    });
}

#[track_caller]
pub(super) fn assert_connection_dropped_item_spawned(
    events: &[TargetedEvent],
    connection_id: ConnectionId,
    expected: Option<(&str, u8)>,
) {
    assert_connection_event(events, connection_id, |event| match event {
        CoreEvent::DroppedItemSpawned { item, .. } => Some(stack_summary(&item.item)) == expected,
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_player_dropped_item_spawned(
    events: &[TargetedEvent],
    player_id: PlayerId,
    expected: Option<(&str, u8)>,
) {
    assert_player_event(events, player_id, |event| match event {
        CoreEvent::DroppedItemSpawned { item, .. } => Some(stack_summary(&item.item)) == expected,
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_player_dropped_item_spawned_at(
    events: &[TargetedEvent],
    player_id: PlayerId,
    key: &str,
    count: u8,
    position: Vec3,
) {
    assert_player_event(events, player_id, |event| match event {
        CoreEvent::DroppedItemSpawned { item, .. } => {
            stack_summary(&item.item) == (key, count) && item.position == position
        }
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_player_entity_despawned(
    events: &[TargetedEvent],
    player_id: PlayerId,
    entity_id: EntityId,
) {
    assert_player_event(events, player_id, |event| match event {
        CoreEvent::EntityDespawned { entity_ids } => entity_ids.contains(&entity_id),
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_player_inventory_contents(events: &[TargetedEvent], player_id: PlayerId) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::InventoryContents {
                container: InventoryContainer::Player,
                ..
            }
        )
    });
}

#[track_caller]
pub(super) fn assert_connection_selected_hotbar_slot(
    events: &[TargetedEvent],
    connection_id: ConnectionId,
    slot: u8,
) {
    assert_connection_event(
        events,
        connection_id,
        |event| matches!(event, CoreEvent::SelectedHotbarSlotChanged { slot: event_slot } if *event_slot == slot),
    );
}

#[track_caller]
pub(super) fn assert_player_selected_hotbar_slot(
    events: &[TargetedEvent],
    player_id: PlayerId,
    slot: u8,
) {
    assert_player_event(
        events,
        player_id,
        |event| matches!(event, CoreEvent::SelectedHotbarSlotChanged { slot: event_slot } if *event_slot == slot),
    );
}

#[track_caller]
pub(super) fn assert_crafting_inputs_empty(inventory: &PlayerInventory) {
    for index in 0_u8..4 {
        assert!(
            inventory.crafting_input(index).is_none(),
            "craft input {index} should be consumed"
        );
    }
}
