mod click;
mod crafting;
mod furnace;
mod lifecycle;
mod state;
mod sync;
mod util;

pub(in crate::core) use self::click::apply_inventory_click_state;
pub(in crate::core) use self::lifecycle::{
    close_inventory_window_state, close_player_active_container_state,
    close_world_container_if_invalid_state, open_non_player_window_state, open_world_chest_state,
    open_world_crafting_table_state, open_world_furnace_state,
    persisted_online_player_snapshot_state, sync_world_chest_viewers_state,
    sync_world_furnace_state, tick_active_container_state, tick_dropped_item_state,
    unregister_world_container_viewer_state, writeback_world_container_state,
};
pub use self::state::{
    ChestWindowBinding, ChestWindowState, ContainerDescriptor, FurnaceWindowBinding,
    FurnaceWindowState, OpenInventoryWindow, OpenInventoryWindowState,
};
pub(in crate::core) use self::sync::{
    inventory_diff_events, property_diff_events, property_events, window_resync_events,
};
