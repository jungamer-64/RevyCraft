use super::support::*;

struct WindowZeroSession {
    core: ServerCore,
    player: PlayerId,
    next_action_number: i16,
    now_ms: u64,
}

impl WindowZeroSession {
    fn new(name: &str) -> Self {
        Self::with_action(name, 1)
    }

    fn with_action(name: &str, next_action_number: i16) -> Self {
        let (core, player) = logged_in_creative_core(name);
        Self {
            core,
            player,
            next_action_number,
            now_ms: 0,
        }
    }

    fn seed_hotbar(&mut self, slot: u8, key: &str, count: u8) {
        let _ = creative_inventory_set(
            &mut self.core,
            self.player,
            InventorySlot::Hotbar(slot),
            Some(item(key, count)),
        );
    }

    fn seed_hotbar0(&mut self, key: &str, count: u8) {
        self.seed_hotbar(0, key, count);
    }

    fn pickup_hotbar0(&mut self) -> (i16, Vec<TargetedEvent>) {
        self.click_window_slot(
            0,
            InventorySlot::Hotbar(0),
            InventoryClickButton::Left,
            None,
        )
    }

    fn pickup_hotbar0_in_window(&mut self, window_id: u8) -> (i16, Vec<TargetedEvent>) {
        self.click_window_raw_slot(window_id, 37, InventoryClickButton::Left, None)
    }

    fn click_hotbar0(&mut self, clicked_item: Option<ItemStack>) -> (i16, Vec<TargetedEvent>) {
        self.click_window_slot(
            0,
            InventorySlot::Hotbar(0),
            InventoryClickButton::Left,
            clicked_item,
        )
    }

    fn click_hotbar0_in_window(
        &mut self,
        window_id: u8,
        clicked_item: Option<ItemStack>,
    ) -> (i16, Vec<TargetedEvent>) {
        self.click_window_slot(
            window_id,
            InventorySlot::Hotbar(0),
            InventoryClickButton::Left,
            clicked_item,
        )
    }

    fn right_click_input(
        &mut self,
        index: u8,
        clicked_item: ItemStack,
    ) -> (i16, Vec<TargetedEvent>) {
        self.click_window_slot(
            0,
            craft_input(index),
            InventoryClickButton::Right,
            Some(clicked_item),
        )
    }

    fn left_click_input(
        &mut self,
        index: u8,
        clicked_item: ItemStack,
    ) -> (i16, Vec<TargetedEvent>) {
        self.click_window_slot(
            0,
            craft_input(index),
            InventoryClickButton::Left,
            Some(clicked_item),
        )
    }

    fn take_result(&mut self, clicked_item: Option<ItemStack>) -> (i16, Vec<TargetedEvent>) {
        self.click_window_slot(
            0,
            InventorySlot::crafting_result(),
            InventoryClickButton::Left,
            clicked_item,
        )
    }

    fn move_cursor_to_offhand(&mut self, clicked_item: ItemStack) -> (i16, Vec<TargetedEvent>) {
        self.click_window_slot(
            0,
            InventorySlot::Offhand,
            InventoryClickButton::Left,
            Some(clicked_item),
        )
    }

    fn online(&self) -> OnlinePlayerState {
        online_player(&self.core, self.player)
    }

    fn active_container_mut(&mut self) -> &mut crate::core::OpenInventoryWindow {
        active_container_mut(&mut self.core, self.player)
    }

    fn open_crafting_table(&mut self, window_id: u8) -> Vec<TargetedEvent> {
        self.core
            .open_crafting_table(self.player, window_id, "Crafting")
    }

    fn open_chest(&mut self, window_id: u8) -> Vec<TargetedEvent> {
        self.core.open_chest(self.player, window_id, "Chest")
    }

    fn open_furnace(&mut self, window_id: u8) -> Vec<TargetedEvent> {
        self.core.open_furnace(self.player, window_id, "Furnace")
    }

    fn close_window(&mut self, window_id: u8) -> Vec<TargetedEvent> {
        self.core.apply_command(
            CoreCommand::CloseContainer {
                player_id: self.player,
                window_id,
            },
            0,
        )
    }

    fn click_window_slot(
        &mut self,
        window_id: u8,
        slot: InventorySlot,
        button: InventoryClickButton,
        clicked_item: Option<ItemStack>,
    ) -> (i16, Vec<TargetedEvent>) {
        let action_number = self.next_action_number;
        self.next_action_number += 1;
        let events = click_slot(
            &mut self.core,
            self.player,
            window_id,
            action_number,
            slot,
            button,
            clicked_item,
        );
        (action_number, events)
    }

    fn click_window_raw_slot(
        &mut self,
        window_id: u8,
        raw_slot: i16,
        button: InventoryClickButton,
        clicked_item: Option<ItemStack>,
    ) -> (i16, Vec<TargetedEvent>) {
        fn player_window_slot(raw_slot: u8) -> Option<InventorySlot> {
            match raw_slot {
                0..=8 => Some(InventorySlot::Auxiliary(raw_slot)),
                9..=35 => Some(InventorySlot::MainInventory(raw_slot - 9)),
                36..=44 => Some(InventorySlot::Hotbar(raw_slot - 36)),
                _ => None,
            }
        }

        fn non_player_window_slot(
            container: InventoryContainer,
            raw_slot: u8,
        ) -> Option<InventorySlot> {
            match container {
                InventoryContainer::Player => player_window_slot(raw_slot),
                InventoryContainer::CraftingTable => match raw_slot {
                    0..=9 => Some(InventorySlot::Container(raw_slot)),
                    10..=36 => Some(InventorySlot::MainInventory(raw_slot - 10)),
                    37..=45 => Some(InventorySlot::Hotbar(raw_slot - 37)),
                    _ => None,
                },
                InventoryContainer::Chest => match raw_slot {
                    0..=26 => Some(InventorySlot::Container(raw_slot)),
                    27..=53 => Some(InventorySlot::MainInventory(raw_slot - 27)),
                    54..=62 => Some(InventorySlot::Hotbar(raw_slot - 54)),
                    _ => None,
                },
                InventoryContainer::Furnace => match raw_slot {
                    0..=2 => Some(InventorySlot::Container(raw_slot)),
                    3..=29 => Some(InventorySlot::MainInventory(raw_slot - 3)),
                    30..=38 => Some(InventorySlot::Hotbar(raw_slot - 30)),
                    _ => None,
                },
            }
        }

        let action_number = self.next_action_number;
        self.next_action_number += 1;
        let target = match raw_slot {
            -999 => InventoryClickTarget::Outside,
            raw_slot if raw_slot.is_negative() => InventoryClickTarget::Unsupported,
            raw_slot => {
                let raw_slot =
                    u8::try_from(raw_slot).expect("non-negative test raw slot should fit in u8");
                let slot = if window_id == 0 {
                    player_window_slot(raw_slot)
                } else {
                    self.online()
                        .active_container
                        .as_ref()
                        .filter(|window| window.window_id == window_id)
                        .and_then(|window| non_player_window_slot(window.container, raw_slot))
                };
                slot.map(InventoryClickTarget::Slot)
                    .unwrap_or(InventoryClickTarget::Unsupported)
            }
        };
        let events = self.core.apply_command(
            CoreCommand::InventoryClick {
                player_id: self.player,
                transaction: InventoryTransactionContext {
                    window_id,
                    action_number,
                },
                target,
                button,
                validation: InventoryClickValidation::StrictSlotEcho { clicked_item },
            },
            0,
        );
        (action_number, events)
    }

    fn tick(&mut self) -> Vec<TargetedEvent> {
        self.now_ms = self.now_ms.saturating_add(50);
        self.core.tick(self.now_ms)
    }
}

fn prepare_stick_recipe(session: &mut WindowZeroSession) {
    session.seed_hotbar0("minecraft:oak_planks", 2);
    let _ = session.pickup_hotbar0();
    let _ = session.right_click_input(0, item("minecraft:oak_planks", 1));
    let _ = session.right_click_input(2, item("minecraft:oak_planks", 1));
}

fn fill_crafting_table_from_hotbar0(
    session: &mut WindowZeroSession,
    key: &str,
    count: u8,
    raw_slots: &[i16],
) {
    session.seed_hotbar0(key, count);
    let _ = session.open_crafting_table(2);
    let _ = session.pickup_hotbar0_in_window(2);
    for raw_slot in raw_slots {
        let _ = session.click_window_raw_slot(
            2,
            *raw_slot,
            InventoryClickButton::Right,
            Some(item(key, 1)),
        );
    }
}

#[test]
fn window_zero_clicks_move_items_between_storage_and_crafting_slots() {
    let mut session = WindowZeroSession::new("window-zero-move");
    session.seed_hotbar0("minecraft:oak_log", 4);

    let (pickup_action, pickup_events) = session.pickup_hotbar0();
    assert_transaction_processed(&pickup_events, session.player, 0, pickup_action, true);
    assert_cursor_changed_to(&pickup_events, session.player, "minecraft:oak_log", 4);

    let (place_action, place_events) = session.right_click_input(0, item("minecraft:oak_log", 1));
    assert_transaction_processed(&place_events, session.player, 0, place_action, true);
    assert_inventory_slot_changed_to(
        &place_events,
        session.player,
        craft_input(0),
        Some(("minecraft:oak_log", 1)),
    );

    let online = session.online();
    assert_eq!(
        online
            .snapshot
            .inventory
            .get_slot(craft_input(0))
            .map(stack_summary),
        Some(("minecraft:oak_log", 1))
    );
    assert_eq!(
        online
            .snapshot
            .inventory
            .crafting_result()
            .map(stack_summary),
        Some(("minecraft:oak_planks", 4))
    );
    assert_eq!(
        online.cursor.as_ref().map(stack_summary),
        Some(("minecraft:oak_log", 3))
    );

    let _ = session.move_cursor_to_offhand(item("minecraft:oak_log", 3));
    let online = session.online();
    assert_eq!(
        online
            .snapshot
            .inventory
            .offhand
            .as_ref()
            .map(stack_summary),
        Some(("minecraft:oak_log", 3))
    );
    assert!(online.cursor.is_none());
}

#[test]
fn accepted_inventory_click_emits_transaction_processed_first() {
    let mut session = WindowZeroSession::new("click-order");
    session.seed_hotbar0("minecraft:oak_log", 1);

    let (action_number, events) = session.pickup_hotbar0();

    assert_transaction_processed(&events, session.player, 0, action_number, true);
    assert!(matches!(
        events.first(),
        Some(TargetedEvent {
            target: EventTarget::Player(player_id),
            event:
                CoreEvent::InventoryTransactionProcessed {
                    transaction: InventoryTransactionContext {
                        window_id: 0,
                        action_number: first_action,
                    },
                    accepted: true,
                },
        }) if *player_id == session.player && *first_action == action_number
    ));
}

#[test]
fn window_zero_recipe_preview_updates_with_inputs() {
    let mut session = WindowZeroSession::new("recipe-preview");
    session.seed_hotbar0("minecraft:sand", 4);

    let _ = session.pickup_hotbar0();
    for index in 0_u8..4 {
        let _ = session.right_click_input(index, item("minecraft:sand", 1));
    }

    assert_eq!(
        session
            .online()
            .snapshot
            .inventory
            .crafting_result()
            .map(stack_summary),
        Some(("minecraft:sandstone", 1))
    );
}

#[test]
fn taking_crafting_result_consumes_inputs_and_recomputes_output() {
    let mut session = WindowZeroSession::new("take-result");
    prepare_stick_recipe(&mut session);

    let (action_number, result_events) = session.take_result(None);
    assert_transaction_processed(&result_events, session.player, 0, action_number, true);
    assert_cursor_changed_to(&result_events, session.player, "minecraft:stick", 4);

    let online = session.online();
    let inventory = &online.snapshot.inventory;
    assert!(inventory.crafting_result().is_none());
    assert_crafting_inputs_empty(inventory);
}

#[test]
fn disconnect_folds_window_zero_state_back_into_persistent_inventory() {
    let mut session = WindowZeroSession::new("disconnect-fold");
    session.seed_hotbar0("minecraft:oak_log", 1);

    let _ = session.pickup_hotbar0();
    let _ = session.left_click_input(0, item("minecraft:oak_log", 1));
    let _ = session.take_result(None);
    let _ = session.core.apply_command(
        CoreCommand::Disconnect {
            player_id: session.player,
        },
        0,
    );

    let snapshot = session.core.snapshot();
    let persisted = snapshot
        .players
        .get(&session.player)
        .expect("player should persist after disconnect");
    assert!(persisted.inventory.crafting_result().is_none());
    assert_crafting_inputs_empty(&persisted.inventory);
    assert!(
        persisted
            .inventory
            .slots
            .iter()
            .flatten()
            .any(|stack| stack.key.as_str() == "minecraft:oak_planks"),
        "crafted planks should be merged back into persistent inventory"
    );
}

#[test]
fn matching_clicked_item_accepts_window_zero_click() {
    let mut session = WindowZeroSession::with_action("accept-click", 7);
    session.seed_hotbar0("minecraft:oak_log", 1);

    let (action_number, events) = session.click_hotbar0(None);
    assert_transaction_processed(&events, session.player, 0, action_number, true);
}

#[test]
fn clicked_item_mismatch_rejects_but_keeps_authoritative_mutation() {
    let mut session = WindowZeroSession::with_action("reject-click", 8);
    session.seed_hotbar0("minecraft:oak_log", 1);

    let (action_number, events) = session.click_hotbar0(Some(item("minecraft:oak_log", 1)));
    assert_transaction_processed(&events, session.player, 0, action_number, false);
    assert_player_inventory_contents(&events, session.player);
    assert_cursor_changed_to(&events, session.player, "minecraft:oak_log", 1);

    let online = session.online();
    assert!(
        online
            .snapshot
            .inventory
            .get_slot(InventorySlot::Hotbar(0))
            .is_none()
    );
    assert_eq!(
        online.cursor.as_ref().map(stack_summary),
        Some(("minecraft:oak_log", 1))
    );
}

#[test]
fn bedrock_authoritative_click_accepts_without_clicked_item_echo() {
    let mut session = WindowZeroSession::with_action("be-auth-click", 80);
    session.seed_hotbar0("minecraft:oak_log", 1);
    let _ = session.pickup_hotbar0();

    let action_number = session.next_action_number;
    session.next_action_number += 1;
    let events = session.core.apply_command(
        CoreCommand::InventoryClick {
            player_id: session.player,
            transaction: InventoryTransactionContext {
                window_id: 0,
                action_number,
            },
            target: InventoryClickTarget::Slot(InventorySlot::Hotbar(1)),
            button: InventoryClickButton::Left,
            validation: InventoryClickValidation::Authoritative,
        },
        0,
    );

    assert_transaction_processed(&events, session.player, 0, action_number, true);
    assert_inventory_slot_changed_to(
        &events,
        session.player,
        InventorySlot::Hotbar(1),
        Some(("minecraft:oak_log", 1)),
    );
    assert_eq!(
        count_player_events(&events, session.player, |event| matches!(
            event,
            CoreEvent::InventoryContents { .. }
        ),),
        0,
    );
}

#[test]
fn bedrock_outside_click_drops_cursor_authoritatively() {
    let mut session = WindowZeroSession::with_action("be-out-drop", 81);
    session.seed_hotbar0("minecraft:stone", 1);
    let _ = session.pickup_hotbar0();

    let action_number = session.next_action_number;
    session.next_action_number += 1;
    let events = session.core.apply_command(
        CoreCommand::InventoryClick {
            player_id: session.player,
            transaction: InventoryTransactionContext {
                window_id: 0,
                action_number,
            },
            target: InventoryClickTarget::Outside,
            button: InventoryClickButton::Left,
            validation: InventoryClickValidation::Authoritative,
        },
        0,
    );

    assert_transaction_processed(&events, session.player, 0, action_number, true);
    assert_player_event(
        &events,
        session.player,
        |event| matches!(event, CoreEvent::CursorChanged { stack } if stack.is_none()),
    );
    assert!(session.online().cursor.is_none());
}

#[test]
fn non_zero_window_click_rejects_without_mutation_or_resync() {
    let mut session = WindowZeroSession::with_action("reject-window-id", 9);
    session.seed_hotbar0("minecraft:oak_log", 1);
    let before = session.online().snapshot.clone();

    let (action_number, events) = session.click_hotbar0_in_window(2, None);

    assert_eq!(events.len(), 1);
    assert_transaction_processed(&events, session.player, 2, action_number, false);
    assert_eq!(session.online().snapshot.inventory, before.inventory);
}

#[test]
fn rejected_crafting_result_click_still_consumes_inputs_authoritatively() {
    let mut session = WindowZeroSession::new("reject-result");
    prepare_stick_recipe(&mut session);

    let (action_number, events) = session.take_result(Some(item("minecraft:stick", 4)));
    assert_transaction_processed(&events, session.player, 0, action_number, false);
    assert_player_inventory_contents(&events, session.player);

    let online = session.online();
    assert!(online.snapshot.inventory.crafting_result().is_none());
    assert_crafting_inputs_empty(&online.snapshot.inventory);
    assert_eq!(
        online.cursor.as_ref().map(stack_summary),
        Some(("minecraft:stick", 4))
    );
}

#[test]
fn opening_crafting_table_window_emits_open_and_contents_events() {
    let mut session = WindowZeroSession::new("ct-open");

    let events = session.open_crafting_table(2);

    assert_container_opened(
        &events,
        session.player,
        2,
        InventoryContainer::CraftingTable,
    );
    assert_player_window_contents(
        &events,
        session.player,
        2,
        InventoryContainer::CraftingTable,
    );
    let online = session.online();
    let window = online
        .active_container
        .as_ref()
        .expect("crafting table should stay open");
    assert_eq!(window.window_id, 2);
    assert_eq!(window.container, InventoryContainer::CraftingTable);
}

#[test]
fn crafting_table_window_clicks_use_window_local_slots_and_cursor_sync() {
    let mut session = WindowZeroSession::new("ct-clicks");
    session.seed_hotbar0("minecraft:oak_log", 1);
    let _ = session.open_crafting_table(2);

    let (pickup_action, pickup_events) = session.pickup_hotbar0_in_window(2);
    assert_transaction_processed(&pickup_events, session.player, 2, pickup_action, true);
    assert_inventory_slot_changed_in_window_to(
        &pickup_events,
        session.player,
        2,
        InventorySlot::Hotbar(0),
        None,
    );
    assert_cursor_changed_to(&pickup_events, session.player, "minecraft:oak_log", 1);

    let (place_action, place_events) = session.click_window_raw_slot(
        2,
        1,
        InventoryClickButton::Left,
        Some(item("minecraft:oak_log", 1)),
    );
    assert_transaction_processed(&place_events, session.player, 2, place_action, true);
    assert_inventory_slot_changed_in_window_to(
        &place_events,
        session.player,
        2,
        InventorySlot::Container(1),
        Some(("minecraft:oak_log", 1)),
    );
    assert_inventory_slot_changed_in_window_to(
        &place_events,
        session.player,
        2,
        InventorySlot::Container(0),
        Some(("minecraft:oak_planks", 4)),
    );

    let (result_action, result_events) =
        session.click_window_raw_slot(2, 0, InventoryClickButton::Left, None);
    assert_transaction_processed(&result_events, session.player, 2, result_action, true);
    assert_inventory_slot_changed_in_window(
        &result_events,
        session.player,
        2,
        InventorySlot::Container(0),
    );
    assert_inventory_slot_changed_in_window_to(
        &result_events,
        session.player,
        2,
        InventorySlot::Container(1),
        None,
    );
    assert_cursor_changed_to(&result_events, session.player, "minecraft:oak_planks", 4);

    let online = session.online();
    let window = online
        .active_container
        .as_ref()
        .expect("crafting table should stay open");
    match &window.state {
        crate::core::OpenInventoryWindowState::CraftingTable { slots } => {
            assert!(slots[0].is_none());
        }
        crate::core::OpenInventoryWindowState::Chest(_) => {
            panic!("crafting table test should not open a chest")
        }
        crate::core::OpenInventoryWindowState::Furnace(_) => {
            panic!("crafting table test should not open a furnace")
        }
    }
    assert_eq!(
        online.cursor.as_ref().map(stack_summary),
        Some(("minecraft:oak_planks", 4))
    );
}

#[test]
fn crafting_table_window_supports_three_by_three_chest_recipe() {
    let mut session = WindowZeroSession::new("ct-chest");
    fill_crafting_table_from_hotbar0(
        &mut session,
        "minecraft:oak_planks",
        8,
        &[1, 2, 3, 4, 6, 7, 8, 9],
    );

    let window = session
        .online()
        .active_container
        .clone()
        .expect("crafting table should stay open");
    assert_eq!(
        window
            .contents(&session.online().snapshot.inventory)
            .get_slot(InventorySlot::Container(0))
            .map(stack_summary),
        Some(("minecraft:chest", 1))
    );

    let (action_number, result_events) =
        session.click_window_raw_slot(2, 0, InventoryClickButton::Left, None);
    assert_transaction_processed(&result_events, session.player, 2, action_number, true);
    assert_cursor_changed_to(&result_events, session.player, "minecraft:chest", 1);

    let window = session
        .online()
        .active_container
        .expect("crafting table should stay open");
    if let crate::core::OpenInventoryWindowState::CraftingTable { slots } = &window.state {
        assert!(slots.iter().all(Option::is_none));
    } else {
        panic!("expected crafting table state");
    }
}

#[test]
fn crafting_table_window_supports_shifted_two_by_two_crafting_table_recipe() {
    let mut session = WindowZeroSession::new("ct-table");
    fill_crafting_table_from_hotbar0(&mut session, "minecraft:oak_planks", 4, &[2, 3, 5, 6]);

    let window = session
        .online()
        .active_container
        .expect("crafting table should stay open");
    assert_eq!(
        window
            .contents(&session.online().snapshot.inventory)
            .get_slot(InventorySlot::Container(0))
            .map(stack_summary),
        Some(("minecraft:crafting_table", 1))
    );
}

#[test]
fn crafting_table_window_supports_three_by_three_furnace_recipe() {
    let mut session = WindowZeroSession::new("ct-furnace");
    fill_crafting_table_from_hotbar0(
        &mut session,
        "minecraft:cobblestone",
        8,
        &[1, 2, 3, 4, 6, 7, 8, 9],
    );

    let window = session
        .online()
        .active_container
        .expect("crafting table should stay open");
    assert_eq!(
        window
            .contents(&session.online().snapshot.inventory)
            .get_slot(InventorySlot::Container(0))
            .map(stack_summary),
        Some(("minecraft:furnace", 1))
    );
}

#[test]
fn closing_crafting_table_window_folds_inputs_back_into_player_inventory() {
    let mut session = WindowZeroSession::new("ct-close");
    session.seed_hotbar0("minecraft:oak_log", 1);
    let _ = session.open_crafting_table(2);
    let _ = session.pickup_hotbar0_in_window(2);
    let _ = session.click_window_raw_slot(
        2,
        1,
        InventoryClickButton::Left,
        Some(item("minecraft:oak_log", 1)),
    );

    let events = session.close_window(2);

    assert_container_closed(&events, session.player, 2);
    assert_player_window_contents(&events, session.player, 0, InventoryContainer::Player);
    assert!(session.online().active_container.is_none());
    assert!(
        session
            .online()
            .snapshot
            .inventory
            .slots
            .iter()
            .flatten()
            .any(|stack| stack.key.as_str() == "minecraft:oak_log" && stack.count == 1),
        "closing the crafting table should merge its inputs back into the player inventory"
    );
}

#[test]
fn stale_clicks_are_rejected_after_crafting_table_closes() {
    let mut session = WindowZeroSession::new("ct-stale");
    session.seed_hotbar0("minecraft:oak_log", 1);
    let _ = session.open_crafting_table(2);
    let _ = session.close_window(2);
    let before = session.online().snapshot.clone();

    let (action_number, events) =
        session.click_window_raw_slot(2, 1, InventoryClickButton::Left, None);

    assert_eq!(events.len(), 1);
    assert_transaction_processed(&events, session.player, 2, action_number, false);
    assert_eq!(session.online().snapshot, before);
}

#[test]
fn disconnect_folds_open_crafting_table_back_into_persistent_inventory() {
    let mut session = WindowZeroSession::new("ct-disc");
    session.seed_hotbar0("minecraft:oak_log", 1);
    let _ = session.open_crafting_table(2);
    let _ = session.pickup_hotbar0_in_window(2);
    let _ = session.click_window_raw_slot(
        2,
        1,
        InventoryClickButton::Left,
        Some(item("minecraft:oak_log", 1)),
    );
    let _ = session.core.apply_command(
        CoreCommand::Disconnect {
            player_id: session.player,
        },
        0,
    );

    let snapshot = session.core.snapshot();
    let persisted = snapshot
        .players
        .get(&session.player)
        .expect("player should persist after disconnect");
    assert!(
        persisted
            .inventory
            .slots
            .iter()
            .flatten()
            .any(|stack| stack.key.as_str() == "minecraft:oak_log" && stack.count == 1),
        "disconnect should fold active crafting table inputs back into persisted inventory"
    );
}

#[test]
fn opening_chest_window_emits_open_and_contents_events() {
    let mut session = WindowZeroSession::new("ch-open");

    let events = session.open_chest(4);

    assert_container_opened(&events, session.player, 4, InventoryContainer::Chest);
    assert_player_window_contents(&events, session.player, 4, InventoryContainer::Chest);
    let online = session.online();
    let window = online
        .active_container
        .as_ref()
        .expect("chest should stay open");
    assert_eq!(window.window_id, 4);
    assert_eq!(window.container, InventoryContainer::Chest);
}

#[test]
fn chest_window_clicks_use_window_local_slots_and_cursor_sync() {
    let mut session = WindowZeroSession::new("ch-click");
    session.seed_hotbar0("minecraft:stone", 2);
    let _ = session.open_chest(4);

    let (pickup_action, pickup_events) =
        session.click_window_raw_slot(4, 54, InventoryClickButton::Left, None);
    assert_transaction_processed(&pickup_events, session.player, 4, pickup_action, true);
    assert_inventory_slot_changed_in_window_to(
        &pickup_events,
        session.player,
        4,
        InventorySlot::Hotbar(0),
        None,
    );
    assert_cursor_changed_to(&pickup_events, session.player, "minecraft:stone", 2);

    let (place_action, place_events) = session.click_window_raw_slot(
        4,
        0,
        InventoryClickButton::Left,
        Some(item("minecraft:stone", 2)),
    );
    assert_transaction_processed(&place_events, session.player, 4, place_action, true);
    assert_inventory_slot_changed_in_window_to(
        &place_events,
        session.player,
        4,
        InventorySlot::Container(0),
        Some(("minecraft:stone", 2)),
    );
    assert_player_event(
        &place_events,
        session.player,
        |event| matches!(event, CoreEvent::CursorChanged { stack } if stack.is_none()),
    );

    let online = session.online();
    let window = online
        .active_container
        .as_ref()
        .expect("chest should stay open");
    let crate::core::OpenInventoryWindowState::Chest(chest) = &window.state else {
        panic!("expected chest state");
    };
    assert_eq!(
        chest.slots[0].as_ref().map(stack_summary),
        Some(("minecraft:stone", 2))
    );
    assert!(session.online().cursor.is_none());
}

#[test]
fn closing_chest_window_folds_contents_back_into_player_inventory() {
    let mut session = WindowZeroSession::new("ch-close");
    let _ = session.open_chest(4);
    {
        let window = session.active_container_mut();
        let crate::core::OpenInventoryWindowState::Chest(chest) = &mut window.state else {
            panic!("expected chest state");
        };
        chest.slots[0] = Some(item("minecraft:oak_log", 1));
    }

    let events = session.close_window(4);

    assert_container_closed(&events, session.player, 4);
    assert_player_window_contents(&events, session.player, 0, InventoryContainer::Player);
    assert!(session.online().active_container.is_none());
    assert!(
        session
            .online()
            .snapshot
            .inventory
            .slots
            .iter()
            .flatten()
            .any(|stack| stack.key.as_str() == "minecraft:oak_log" && stack.count == 1),
        "closing the chest should merge its contents back into the player inventory"
    );
}

#[test]
fn stale_clicks_are_rejected_after_chest_closes() {
    let mut session = WindowZeroSession::new("ch-stale");
    let _ = session.open_chest(4);
    let _ = session.close_window(4);
    let before = session.online().snapshot.clone();

    let (action_number, events) =
        session.click_window_raw_slot(4, 0, InventoryClickButton::Left, None);

    assert_eq!(events.len(), 1);
    assert_transaction_processed(&events, session.player, 4, action_number, false);
    assert_eq!(session.online().snapshot, before);
}

#[test]
fn disconnect_folds_open_chest_back_into_persistent_inventory() {
    let mut session = WindowZeroSession::new("ch-disc");
    let _ = session.open_chest(4);
    {
        let window = session.active_container_mut();
        let crate::core::OpenInventoryWindowState::Chest(chest) = &mut window.state else {
            panic!("expected chest state");
        };
        chest.slots[0] = Some(item("minecraft:stone", 8));
    }
    let _ = session.core.apply_command(
        CoreCommand::Disconnect {
            player_id: session.player,
        },
        0,
    );

    let snapshot = session.core.snapshot();
    let persisted = snapshot
        .players
        .get(&session.player)
        .expect("player should persist after disconnect");
    assert!(
        persisted
            .inventory
            .slots
            .iter()
            .flatten()
            .any(|stack| stack.key.as_str() == "minecraft:stone" && stack.count == 8),
        "disconnect should fold active chest contents back into persisted inventory"
    );
}

#[test]
fn opening_furnace_window_emits_open_contents_and_initial_properties() {
    let mut session = WindowZeroSession::new("furnace-open");

    let events = session.open_furnace(3);

    assert_container_opened(&events, session.player, 3, InventoryContainer::Furnace);
    assert_player_window_contents(&events, session.player, 3, InventoryContainer::Furnace);
    assert_container_property_changed(&events, session.player, 3, 0, 0);
    assert_container_property_changed(&events, session.player, 3, 1, 0);
    assert_container_property_changed(&events, session.player, 3, 2, 0);
    assert_container_property_changed(&events, session.player, 3, 3, 200);
}

#[test]
fn opening_furnace_window_orders_open_before_contents_before_properties() {
    let mut session = WindowZeroSession::new("fur-open-ord");
    let events = session.open_furnace(3);

    assert_container_opened(&events, session.player, 3, InventoryContainer::Furnace);
    assert_player_window_contents(&events, session.player, 3, InventoryContainer::Furnace);
    assert_container_property_changed(&events, session.player, 3, 0, 0);

    let open_index = events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(player_id),
                    CoreEvent::ContainerOpened {
                        window_id: 3,
                        container: InventoryContainer::Furnace,
                        ..
                    }
                ) if *player_id == session.player
            )
        })
        .expect("open event should be present");
    let contents_index = events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(player_id),
                    CoreEvent::InventoryContents {
                        window_id: 3,
                        container: InventoryContainer::Furnace,
                        ..
                    }
                ) if *player_id == session.player
            )
        })
        .expect("contents event should be present");
    let property_index = events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(player_id),
                    CoreEvent::ContainerPropertyChanged { window_id: 3, .. }
                ) if *player_id == session.player
            )
        })
        .expect("property event should be present");

    assert!(open_index < contents_index);
    assert!(contents_index < property_index);
}

#[test]
fn furnace_window_smelts_sand_into_glass_and_emits_property_updates() {
    let mut session = WindowZeroSession::new("furnace-smelt");
    session.seed_hotbar(0, "minecraft:sand", 1);
    session.seed_hotbar(1, "minecraft:oak_planks", 1);
    let _ = session.open_furnace(3);

    let (pickup_input_action, pickup_input_events) =
        session.click_window_raw_slot(3, 30, InventoryClickButton::Left, None);
    assert_transaction_processed(
        &pickup_input_events,
        session.player,
        3,
        pickup_input_action,
        true,
    );
    let (place_input_action, place_input_events) = session.click_window_raw_slot(
        3,
        0,
        InventoryClickButton::Left,
        Some(item("minecraft:sand", 1)),
    );
    assert_transaction_processed(
        &place_input_events,
        session.player,
        3,
        place_input_action,
        true,
    );

    let (pickup_fuel_action, pickup_fuel_events) =
        session.click_window_raw_slot(3, 31, InventoryClickButton::Left, None);
    assert_transaction_processed(
        &pickup_fuel_events,
        session.player,
        3,
        pickup_fuel_action,
        true,
    );
    let (place_fuel_action, place_fuel_events) = session.click_window_raw_slot(
        3,
        1,
        InventoryClickButton::Left,
        Some(item("minecraft:oak_planks", 1)),
    );
    assert_transaction_processed(
        &place_fuel_events,
        session.player,
        3,
        place_fuel_action,
        true,
    );

    let first_tick = session.tick();
    assert_container_property_changed(&first_tick, session.player, 3, 0, 300);
    assert_container_property_changed(&first_tick, session.player, 3, 1, 300);
    assert_container_property_changed(&first_tick, session.player, 3, 2, 1);
    assert_inventory_slot_changed_in_window_to(
        &first_tick,
        session.player,
        3,
        InventorySlot::Container(1),
        None,
    );

    let mut final_tick = Vec::new();
    for _ in 1..200 {
        final_tick = session.tick();
    }

    assert_inventory_slot_changed_in_window_to(
        &final_tick,
        session.player,
        3,
        InventorySlot::Container(0),
        None,
    );
    assert_inventory_slot_changed_in_window_to(
        &final_tick,
        session.player,
        3,
        InventorySlot::Container(2),
        Some(("minecraft:glass", 1)),
    );
    assert_container_property_changed(&final_tick, session.player, 3, 2, 0);
}

#[test]
fn invalid_fuel_does_not_start_furnace_progress() {
    let mut session = WindowZeroSession::new("fur-badfuel");
    let _ = session.open_furnace(3);
    {
        let window = session.active_container_mut();
        let crate::core::OpenInventoryWindowState::Furnace(furnace) = &mut window.state else {
            panic!("expected furnace state");
        };
        furnace.input = Some(item("minecraft:sand", 1));
        furnace.fuel = Some(item("minecraft:glass", 1));
    }

    let events = session.tick();
    assert!(events.is_empty());
    let online = session.online();
    let window = online
        .active_container
        .as_ref()
        .expect("furnace should stay open");
    let crate::core::OpenInventoryWindowState::Furnace(furnace) = &window.state else {
        panic!("expected furnace state");
    };
    assert_eq!(furnace.burn_left, 0);
    assert_eq!(furnace.cook_progress, 0);
    assert_eq!(furnace.output.as_ref().map(stack_summary), None);
}

#[test]
fn furnace_output_slot_rejects_placement_attempts() {
    let mut session = WindowZeroSession::new("fur-outrej");
    session.seed_hotbar0("minecraft:stone", 1);
    let _ = session.open_furnace(3);
    let _ = session.click_window_raw_slot(3, 30, InventoryClickButton::Left, None);

    let (action_number, events) = session.click_window_raw_slot(
        3,
        2,
        InventoryClickButton::Left,
        Some(item("minecraft:stone", 1)),
    );

    assert_transaction_processed(&events, session.player, 3, action_number, false);
    assert_player_window_contents(&events, session.player, 3, InventoryContainer::Furnace);
    assert_cursor_changed_to(&events, session.player, "minecraft:stone", 1);
}

#[test]
fn furnace_output_stacks_existing_matching_items() {
    let mut session = WindowZeroSession::new("fur-stack");
    let _ = session.open_furnace(3);
    {
        let window = session.active_container_mut();
        let crate::core::OpenInventoryWindowState::Furnace(furnace) = &mut window.state else {
            panic!("expected furnace state");
        };
        furnace.input = Some(item("minecraft:sand", 1));
        furnace.fuel = Some(item("minecraft:oak_planks", 1));
        furnace.output = Some(item("minecraft:glass", 1));
    }

    for _ in 0..200 {
        let _ = session.tick();
    }

    let online = session.online();
    let window = online
        .active_container
        .as_ref()
        .expect("furnace should stay open");
    let crate::core::OpenInventoryWindowState::Furnace(furnace) = &window.state else {
        panic!("expected furnace state");
    };
    assert_eq!(
        furnace.output.as_ref().map(stack_summary),
        Some(("minecraft:glass", 2))
    );
}

#[test]
fn closing_furnace_window_folds_contents_back_into_player_inventory() {
    let mut session = WindowZeroSession::new("furnace-close");
    let _ = session.open_furnace(3);
    {
        let window = session.active_container_mut();
        let crate::core::OpenInventoryWindowState::Furnace(furnace) = &mut window.state else {
            panic!("expected furnace state");
        };
        furnace.input = Some(item("minecraft:sand", 1));
        furnace.fuel = Some(item("minecraft:oak_planks", 1));
        furnace.output = Some(item("minecraft:glass", 1));
    }

    let events = session.close_window(3);

    assert_container_closed(&events, session.player, 3);
    assert!(session.online().active_container.is_none());
    let online = session.online();
    let inventory = &online.snapshot.inventory;
    assert!(
        inventory
            .slots
            .iter()
            .flatten()
            .any(|stack| stack.key.as_str() == "minecraft:sand")
    );
    assert!(
        inventory
            .slots
            .iter()
            .flatten()
            .any(|stack| stack.key.as_str() == "minecraft:glass")
    );
}

#[test]
fn closing_furnace_window_orders_close_before_player_contents() {
    let mut session = WindowZeroSession::new("fur-close-ord");
    let _ = session.open_furnace(3);

    let events = session.close_window(3);

    assert_container_closed(&events, session.player, 3);
    assert_player_window_contents(&events, session.player, 0, InventoryContainer::Player);

    let close_index = events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(player_id),
                    CoreEvent::ContainerClosed { window_id: 3 }
                ) if *player_id == session.player
            )
        })
        .expect("close event should be present");
    let contents_index = events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(player_id),
                    CoreEvent::InventoryContents {
                        window_id: 0,
                        container: InventoryContainer::Player,
                        ..
                    }
                ) if *player_id == session.player
            )
        })
        .expect("player contents event should be present");

    assert!(close_index < contents_index);
}

#[test]
fn stale_clicks_are_rejected_after_furnace_closes() {
    let mut session = WindowZeroSession::new("furnace-stale");
    let _ = session.open_furnace(3);
    let _ = session.close_window(3);
    let before = session.online().snapshot.clone();

    let (action_number, events) =
        session.click_window_raw_slot(3, 0, InventoryClickButton::Left, None);

    assert_eq!(events.len(), 1);
    assert_transaction_processed(&events, session.player, 3, action_number, false);
    assert_eq!(session.online().snapshot, before);
}

#[test]
fn disconnect_folds_open_furnace_back_into_persistent_inventory() {
    let mut session = WindowZeroSession::new("furnace-disc");
    let _ = session.open_furnace(3);
    {
        let window = session.active_container_mut();
        let crate::core::OpenInventoryWindowState::Furnace(furnace) = &mut window.state else {
            panic!("expected furnace state");
        };
        furnace.input = Some(item("minecraft:sand", 1));
        furnace.output = Some(item("minecraft:glass", 1));
    }
    let _ = session.core.apply_command(
        CoreCommand::Disconnect {
            player_id: session.player,
        },
        0,
    );

    let snapshot = session.core.snapshot();
    let persisted = snapshot
        .players
        .get(&session.player)
        .expect("player should persist after disconnect");
    assert!(
        persisted
            .inventory
            .slots
            .iter()
            .flatten()
            .any(|stack| stack.key.as_str() == "minecraft:glass")
    );
}
