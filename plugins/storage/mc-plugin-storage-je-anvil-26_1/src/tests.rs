use super::level;
use super::nbt::{NbtTag, compound_field, read_gzip_nbt};
use super::{JE_26_1_DATA_VERSION, Je261StoragePlugin, item_stack_from_nbt, item_stack_to_nbt};
use mc_plugin_sdk_rust::storage::RustStoragePlugin;
use mc_plugin_sdk_rust::{PlayerId, PlayerSnapshot, WorldSnapshot};
use mc_storage_common::StorageError;
use revy_voxel_semantic::BlockEntityState;
use revy_voxel_semantic::{
    BlockPos, ChunkColumn, ItemDataMap, ItemDataValue, ItemStack, Vec3, WorldMeta,
};
use std::collections::BTreeMap;
use tempfile::TempDir;
use uuid::Uuid;

#[test]
fn level_dat_roundtrip_uses_26_1_metadata() -> Result<(), StorageError> {
    let temp_dir = tempdir()?;
    let level_path = temp_dir.path().join("level.dat");

    level::write_level_dat(&level_path, &sample_meta())?;

    let root = read_gzip_nbt(&level_path)?;
    let data = compound_field(super::nbt::as_compound(&root)?, "Data")?;
    let version = compound_field(data, "Version")?;
    assert_eq!(
        super::nbt::int_field(data, "DataVersion")?,
        JE_26_1_DATA_VERSION
    );
    assert_eq!(super::nbt::int_field(version, "Id")?, JE_26_1_DATA_VERSION);
    assert_eq!(super::nbt::string_field(version, "Name")?, "26.1");
    assert_eq!(level::read_level_dat(&level_path)?, sample_meta());
    Ok(())
}

#[test]
fn modern_item_roundtrip_preserves_components_and_extra_fields() -> Result<(), StorageError> {
    let stack = component_stack();
    let tag = item_stack_to_nbt(&stack, Some(3))?;
    let NbtTag::Compound(compound) = tag else {
        panic!("item nbt should be a compound");
    };

    assert!(compound.contains_key("count"));
    assert!(compound.contains_key("components"));
    assert!(!compound.contains_key("Count"));

    let decoded = item_stack_from_nbt(&compound)?;
    assert_eq!(decoded, stack);
    Ok(())
}

#[test]
fn modern_item_decode_rejects_legacy_fields() {
    let compound = BTreeMap::from([
        (
            "id".to_string(),
            NbtTag::String("minecraft:stone".to_string()),
        ),
        ("Count".to_string(), NbtTag::Byte(1)),
        ("count".to_string(), NbtTag::Byte(1)),
    ]);

    let error = item_stack_from_nbt(&compound).expect_err("legacy fields should fail");
    assert!(
        matches!(error, StorageError::InvalidData(message) if message.contains("legacy-style item fields"))
    );
}

#[test]
fn snapshot_roundtrip_preserves_modern_items() -> Result<(), StorageError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let meta = sample_meta();
    let chest_pos = BlockPos::new(4, 64, 4);
    let mut chunk = ChunkColumn::new(meta.spawn.chunk_pos());
    chunk.set_block(
        4,
        chest_pos.y,
        4,
        Some(revy_voxel_semantic::BlockState::new("minecraft:chest")),
    );
    chunk.biomes = vec![1; 256];

    let mut chest_slots = vec![None; 27];
    chest_slots[0] = Some(component_stack());

    let player_id = PlayerId(Uuid::from_u128(0x26_1));
    let mut inventory = revy_voxel_semantic::PlayerInventory::new_empty();
    let _ = inventory.set(36, Some(component_stack()));

    let snapshot = WorldSnapshot {
        meta,
        chunks: BTreeMap::from([(chunk.pos, chunk)]),
        block_entities: BTreeMap::from([(
            chest_pos,
            BlockEntityState::container(
                mc_content_canonical::ids::CHEST_BLOCK_ENTITY,
                chest_slots,
                BTreeMap::new(),
            ),
        )]),
        players: BTreeMap::from([(
            player_id,
            PlayerSnapshot {
                id: player_id,
                username: "player".to_string(),
                position: Vec3::new(0.5, 65.0, 0.5),
                yaw: 0.0,
                pitch: 0.0,
                on_ground: true,
                dimension: revy_voxel_semantic::DimensionId::Overworld,
                health: 20.0,
                food: 20,
                food_saturation: 5.0,
                inventory,
                selected_hotbar_slot: 0,
            },
        )]),
    };

    Je261StoragePlugin.save_snapshot(&world_dir, &snapshot)?;
    let loaded = Je261StoragePlugin
        .load_snapshot(&world_dir)?
        .expect("saved world should load");

    assert_eq!(loaded, snapshot);
    Ok(())
}

fn component_stack() -> ItemStack {
    let mut stack = ItemStack::new("minecraft:stone", 7, 0);
    let mut components = ItemDataMap::new();
    let _ = components.insert(
        "minecraft:custom_name",
        ItemDataValue::String("{\"text\":\"Stone\"}".to_string()),
    );
    let _ = components.insert("minecraft:repair_cost", ItemDataValue::Int(3));
    let mut extra = ItemDataMap::new();
    let _ = extra.insert("foo", ItemDataValue::Long(42));
    stack.components = components;
    stack.extra = extra;
    stack
}

fn sample_meta() -> WorldMeta {
    WorldMeta {
        level_name: "world".to_string(),
        seed: 123,
        spawn: BlockPos::new(0, 64, 0),
        dimension: revy_voxel_semantic::DimensionId::Overworld,
        age: 10,
        time: 20,
        level_type: "FLAT".to_string(),
        game_mode: 1,
        difficulty: 1,
        max_players: 20,
    }
}

fn tempdir() -> Result<TempDir, StorageError> {
    TempDir::new().map_err(StorageError::from)
}
