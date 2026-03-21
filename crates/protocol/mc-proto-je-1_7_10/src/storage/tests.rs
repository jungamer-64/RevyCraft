use super::Je1710StorageAdapter;
use mc_core::{ChunkColumn, ChunkPos, CoreConfig, InventorySlot, ItemStack, PlayerId, ServerCore};
use mc_proto_common::StorageAdapter;
use tempfile::tempdir;
use uuid::Uuid;

#[test]
fn snapshot_round_trip_through_anvil_and_nbt() {
    let temp_dir = tempdir().expect("temp dir should exist");
    let mut core = ServerCore::new(CoreConfig::default());
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"storage-roundtrip"));
    let _ = core.apply_command(
        mc_core::CoreCommand::LoginStart {
            connection_id: mc_core::ConnectionId(1),
            username: "alpha".to_string(),
            player_id,
        },
        0,
    );
    let mut snapshot = core.snapshot();
    let mut custom_chunk = ChunkColumn::new(ChunkPos::new(4, 5));
    custom_chunk.set_block(0, 0, 0, mc_core::BlockState::bedrock());
    snapshot.chunks.insert(custom_chunk.pos, custom_chunk);
    snapshot
        .players
        .get_mut(&player_id)
        .expect("player should exist")
        .inventory
        .set_slot(
            InventorySlot::Offhand,
            Some(ItemStack::new("minecraft:glass", 16, 0)),
        );

    let storage = Je1710StorageAdapter;
    storage
        .save_snapshot(temp_dir.path(), &snapshot)
        .expect("snapshot should save");
    let loaded = storage
        .load_snapshot(temp_dir.path())
        .expect("snapshot should load")
        .expect("snapshot should exist");

    assert_eq!(loaded.meta.level_name, snapshot.meta.level_name);
    assert!(loaded.players.contains_key(&player_id));
    assert_eq!(
        loaded
            .players
            .get(&player_id)
            .expect("player should load")
            .inventory
            .offhand
            .as_ref()
            .map(|stack| (stack.key.as_str(), stack.count, stack.damage)),
        Some(("minecraft:glass", 16, 0))
    );
    assert_eq!(
        loaded
            .chunks
            .get(&ChunkPos::new(4, 5))
            .expect("custom chunk should exist")
            .get_block(0, 0, 0)
            .key
            .as_str(),
        "minecraft:bedrock"
    );
}
