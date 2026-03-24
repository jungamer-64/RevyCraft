use super::*;

struct WindowZeroSession {
    core: ServerCore,
    player: PlayerId,
    next_action_number: i16,
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
        }
    }

    fn seed_hotbar0(&mut self, key: &str, count: u8) {
        let _ = creative_inventory_set(
            &mut self.core,
            self.player,
            InventorySlot::Hotbar(0),
            Some(item(key, count)),
        );
    }

    fn pickup_hotbar0(&mut self) -> (i16, Vec<TargetedEvent>) {
        self.click_window_slot(
            0,
            InventorySlot::Hotbar(0),
            InventoryClickButton::Left,
            None,
        )
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

    fn online(&self) -> &crate::core::OnlinePlayer {
        online_player(&self.core, self.player)
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
}

fn prepare_stick_recipe(session: &mut WindowZeroSession) {
    session.seed_hotbar0("minecraft:oak_planks", 2);
    let _ = session.pickup_hotbar0();
    let _ = session.right_click_input(0, item("minecraft:oak_planks", 1));
    let _ = session.right_click_input(2, item("minecraft:oak_planks", 1));
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

    let inventory = &session.online().snapshot.inventory;
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
