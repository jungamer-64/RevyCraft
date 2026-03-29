use super::chunk_nbt;
use super::level;
use super::nbt::{NbtTag, read_gzip_nbt, write_gzip_nbt, zlib_compress_nbt};
use super::{JE_1_18_2_DATA_VERSION, Je1182StoragePlugin};
use mc_content_api::{BlockEntityState, ContainerPropertyKey};
use mc_core::{PlayerId, PlayerSnapshot, WorldSnapshot};
use mc_model::{
    BlockPos, BlockState, ChunkColumn, ChunkPos, DimensionId, InventorySlot, ItemStack,
    PlayerInventory, Vec3, WorldMeta,
};
use mc_plugin_sdk_rust::storage::RustStoragePlugin;
use mc_proto_common::StorageError;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use uuid::Uuid;

const ANVIL_SECTOR_BYTES: usize = 4096;

#[test]
fn snapshot_roundtrip_handles_negative_y_palette_block_entities_and_playerdata()
-> Result<(), StorageError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let plugin = Je1182StoragePlugin;
    let snapshot = sample_snapshot();

    plugin.save_snapshot(&world_dir, &snapshot)?;
    let loaded = plugin
        .load_snapshot(&world_dir)?
        .expect("saved world should load back");

    assert_eq!(loaded, snapshot);
    Ok(())
}

#[test]
fn biome_loading_uses_the_topmost_cell_per_column() -> Result<(), StorageError> {
    let root = modern_chunk_root(
        ChunkPos::new(0, 0),
        vec![
            section_with_biomes(
                0,
                vec!["minecraft:plains", "minecraft:desert"],
                top_layer_cells("minecraft:desert", "minecraft:plains"),
            ),
            section_with_biomes(
                1,
                vec!["minecraft:plains", "minecraft:forest"],
                top_layer_cells("minecraft:forest", "minecraft:plains"),
            ),
        ],
        Vec::new(),
    );

    let (chunk, _) = chunk_nbt::chunk_from_nbt(&root)?;

    assert_eq!(chunk.biomes[biome_index(0, 0)], 4);
    assert_eq!(chunk.biomes[biome_index(3, 3)], 4);
    assert_eq!(chunk.biomes[biome_index(4, 0)], 1);
    Ok(())
}

#[test]
fn load_rejects_unexpected_level_data_version() -> Result<(), StorageError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    level::write_level_dat(&world_dir.join("level.dat"), &sample_meta())?;

    let mut root = read_gzip_nbt(&world_dir.join("level.dat"))?;
    let data = compound_mut(&mut root, "Data")?;
    data.insert("DataVersion".to_string(), NbtTag::Int(2974));
    write_gzip_nbt(&world_dir.join("level.dat"), "", &root)?;

    let error = Je1182StoragePlugin
        .load_snapshot(&world_dir)
        .expect_err("unexpected data version should fail");
    assert!(
        matches!(error, StorageError::InvalidData(message) if message.contains("Data.DataVersion"))
    );
    Ok(())
}

#[test]
fn load_rejects_unsupported_block_entities() -> Result<(), StorageError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    level::write_level_dat(&world_dir.join("level.dat"), &sample_meta())?;

    let block_entity = NbtTag::Compound(BTreeMap::from([
        (
            "id".to_string(),
            NbtTag::String("minecraft:beacon".to_string()),
        ),
        ("x".to_string(), NbtTag::Int(2)),
        ("y".to_string(), NbtTag::Int(64)),
        ("z".to_string(), NbtTag::Int(2)),
    ]));
    let root = modern_chunk_root(
        ChunkPos::new(0, 0),
        vec![air_only_section(0)],
        vec![block_entity],
    );
    write_region_roots(
        &world_dir.join("region"),
        &BTreeMap::from([(ChunkPos::new(0, 0), root)]),
    )?;

    let error = Je1182StoragePlugin
        .load_snapshot(&world_dir)
        .expect_err("unsupported block entity should fail");
    assert!(
        matches!(error, StorageError::InvalidData(message) if message.contains("unsupported block entity"))
    );
    Ok(())
}

#[test]
fn load_rejects_player_items_with_tags() -> Result<(), StorageError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    level::write_level_dat(&world_dir.join("level.dat"), &sample_meta())?;

    let player_id = Uuid::from_u128(0x1234);
    write_player_fixture(
        &world_dir.join("playerdata"),
        player_id,
        NbtTag::String("minecraft:overworld".to_string()),
        true,
    )?;

    let error = Je1182StoragePlugin
        .load_snapshot(&world_dir)
        .expect_err("player item tags should fail");
    assert!(
        matches!(error, StorageError::InvalidData(message) if message.contains("player inventory item tag"))
    );
    Ok(())
}

#[test]
fn load_rejects_non_overworld_players() -> Result<(), StorageError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    level::write_level_dat(&world_dir.join("level.dat"), &sample_meta())?;

    let player_id = Uuid::from_u128(0x5678);
    write_player_fixture(
        &world_dir.join("playerdata"),
        player_id,
        NbtTag::String("minecraft:the_nether".to_string()),
        false,
    )?;

    let error = Je1182StoragePlugin
        .load_snapshot(&world_dir)
        .expect_err("non-overworld players should fail");
    assert!(
        matches!(error, StorageError::InvalidData(message) if message.contains("only overworld playerdata"))
    );
    Ok(())
}

#[test]
fn empty_world_save_generates_flat_world_files() -> Result<(), StorageError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let plugin = Je1182StoragePlugin;
    let snapshot = WorldSnapshot {
        meta: sample_meta(),
        chunks: BTreeMap::new(),
        block_entities: BTreeMap::new(),
        players: BTreeMap::new(),
    };

    plugin.save_snapshot(&world_dir, &snapshot)?;
    let loaded = plugin
        .load_snapshot(&world_dir)?
        .expect("generated flat world should load");
    let spawn_chunk = loaded
        .chunks
        .get(&loaded.meta.spawn.chunk_pos())
        .expect("spawn chunk should be synthesized");

    assert!(world_dir.join("level.dat").is_file());
    assert!(world_dir.join("playerdata").is_dir());
    assert_eq!(
        spawn_chunk.get_block(0, 0, 0),
        Some(BlockState::new("minecraft:bedrock"))
    );
    assert_eq!(
        spawn_chunk.get_block(0, 3, 0),
        Some(BlockState::new("minecraft:grass_block"))
    );
    assert!(
        fs::read_dir(world_dir.join("region"))?
            .filter_map(Result::ok)
            .any(|entry| entry.path().extension().and_then(std::ffi::OsStr::to_str) == Some("mca"))
    );
    Ok(())
}

fn sample_snapshot() -> WorldSnapshot {
    let meta = sample_meta();
    let chunk_pos = meta.spawn.chunk_pos();
    let mut chunk = ChunkColumn::new(chunk_pos);
    let mut patterned = BlockState::new("minecraft:oak_log");
    patterned
        .properties
        .insert("axis".to_string(), "y".to_string());
    chunk.set_block(1, -64, 1, Some(BlockState::new("minecraft:bedrock")));
    chunk.set_block(2, -63, 2, Some(BlockState::new("minecraft:stone")));
    chunk.set_block(4, -63, 4, Some(BlockState::new("minecraft:chest")));
    chunk.set_block(6, 20, 6, Some(BlockState::new("minecraft:furnace")));
    chunk.set_block(3, 20, 3, Some(patterned));
    chunk.biomes = vec![4; 256];

    let mut chunks = BTreeMap::new();
    chunks.insert(chunk_pos, chunk);

    let mut chest_slots = vec![None; 27];
    chest_slots[0] = Some(ItemStack::new("minecraft:cobblestone", 32, 0));
    let mut block_entities = BTreeMap::new();
    block_entities.insert(
        BlockPos::new(4, -63, 4),
        BlockEntityState::container(
            mc_content_canonical::ids::CHEST_BLOCK_ENTITY,
            chest_slots,
            BTreeMap::new(),
        ),
    );
    block_entities.insert(
        BlockPos::new(6, 20, 6),
        BlockEntityState::container(
            mc_content_canonical::ids::FURNACE_BLOCK_ENTITY,
            vec![
                Some(ItemStack::new("minecraft:iron_ore", 3, 0)),
                Some(ItemStack::new("minecraft:coal", 5, 0)),
                Some(ItemStack::new("minecraft:iron_ingot", 1, 0)),
            ],
            BTreeMap::from([
                (
                    ContainerPropertyKey::new(mc_content_canonical::ids::FURNACE_BURN_LEFT),
                    40,
                ),
                (
                    ContainerPropertyKey::new(mc_content_canonical::ids::FURNACE_BURN_MAX),
                    200,
                ),
                (
                    ContainerPropertyKey::new(mc_content_canonical::ids::FURNACE_COOK_PROGRESS),
                    80,
                ),
                (
                    ContainerPropertyKey::new(mc_content_canonical::ids::FURNACE_COOK_TOTAL),
                    200,
                ),
            ]),
        ),
    );

    let player = sample_player();
    let mut players = BTreeMap::new();
    players.insert(player.id, player);

    WorldSnapshot {
        meta,
        chunks,
        block_entities,
        players,
    }
}

fn sample_meta() -> WorldMeta {
    WorldMeta {
        level_name: "je-1-18-2".to_string(),
        seed: 987_654_321,
        spawn: BlockPos::new(8, 5, 8),
        dimension: DimensionId::Overworld,
        age: 1_234,
        time: 456,
        level_type: "FLAT".to_string(),
        game_mode: 1,
        difficulty: 2,
        max_players: 20,
    }
}

fn sample_player() -> PlayerSnapshot {
    let mut inventory = PlayerInventory::new_empty();
    let _ = inventory.set(36, Some(ItemStack::new("minecraft:stone", 64, 0)));
    let _ = inventory.set(9, Some(ItemStack::new("minecraft:torch", 16, 0)));
    let _ = inventory.set_slot(
        InventorySlot::Offhand,
        Some(ItemStack::new("minecraft:shield", 1, 0)),
    );
    PlayerSnapshot {
        id: PlayerId(Uuid::from_u128(0xAABBCCDD)),
        username: "builder".to_string(),
        position: Vec3::new(8.5, 5.0, 8.5),
        yaw: 90.0,
        pitch: 15.0,
        on_ground: true,
        dimension: DimensionId::Overworld,
        health: 18.5,
        food: 17,
        food_saturation: 4.5,
        inventory,
        selected_hotbar_slot: 0,
    }
}

fn write_player_fixture(
    playerdata_dir: &Path,
    player_id: Uuid,
    dimension: NbtTag,
    include_tagged_item: bool,
) -> Result<(), StorageError> {
    fs::create_dir_all(playerdata_dir)?;
    let mut item = BTreeMap::from([
        ("Slot".to_string(), NbtTag::Byte(0)),
        (
            "id".to_string(),
            NbtTag::String("minecraft:stone".to_string()),
        ),
        ("Count".to_string(), NbtTag::Byte(1)),
    ]);
    if include_tagged_item {
        item.insert("tag".to_string(), NbtTag::Compound(BTreeMap::new()));
    }
    let root = NbtTag::Compound(BTreeMap::from([
        (
            "Pos".to_string(),
            NbtTag::List(
                6,
                vec![
                    NbtTag::Double(0.0),
                    NbtTag::Double(64.0),
                    NbtTag::Double(0.0),
                ],
            ),
        ),
        (
            "Rotation".to_string(),
            NbtTag::List(5, vec![NbtTag::Float(0.0), NbtTag::Float(0.0)]),
        ),
        (
            "UUID".to_string(),
            NbtTag::IntArray(uuid_to_int_array(player_id)),
        ),
        ("Dimension".to_string(), dimension),
        ("Name".to_string(), NbtTag::String("fixture".to_string())),
        (
            "Inventory".to_string(),
            NbtTag::List(10, vec![NbtTag::Compound(item)]),
        ),
    ]));
    write_gzip_nbt(
        &playerdata_dir.join(format!("{}.dat", player_id.hyphenated())),
        "",
        &root,
    )
}

fn modern_chunk_root(
    chunk_pos: ChunkPos,
    sections: Vec<NbtTag>,
    block_entities: Vec<NbtTag>,
) -> NbtTag {
    NbtTag::Compound(BTreeMap::from([
        (
            "DataVersion".to_string(),
            NbtTag::Int(JE_1_18_2_DATA_VERSION),
        ),
        ("xPos".to_string(), NbtTag::Int(chunk_pos.x)),
        ("yPos".to_string(), NbtTag::Int(0)),
        ("zPos".to_string(), NbtTag::Int(chunk_pos.z)),
        ("Status".to_string(), NbtTag::String("full".to_string())),
        ("LastUpdate".to_string(), NbtTag::Long(0)),
        ("InhabitedTime".to_string(), NbtTag::Long(0)),
        ("entities".to_string(), NbtTag::List(10, Vec::new())),
        ("block_ticks".to_string(), NbtTag::List(10, Vec::new())),
        ("fluid_ticks".to_string(), NbtTag::List(10, Vec::new())),
        ("structures".to_string(), NbtTag::Compound(BTreeMap::new())),
        ("sections".to_string(), NbtTag::List(10, sections)),
        (
            "block_entities".to_string(),
            NbtTag::List(10, block_entities),
        ),
    ]))
}

fn air_only_section(section_y: i8) -> NbtTag {
    section_with_biomes(
        section_y,
        vec!["minecraft:plains"],
        top_layer_cells("minecraft:plains", "minecraft:plains"),
    )
}

fn section_with_biomes(section_y: i8, palette: Vec<&str>, top_layer: Vec<&str>) -> NbtTag {
    let mut biome_palette = Vec::new();
    let mut palette_indices = BTreeMap::new();
    for biome_name in &palette {
        let index = biome_palette.len();
        biome_palette.push((*biome_name).to_string());
        palette_indices.insert((*biome_name).to_string(), index);
    }

    let mut biome_indices = vec![0_usize; 64];
    for (cell_index, biome_name) in top_layer.into_iter().enumerate() {
        biome_indices[biome_cell_index(cell_index % 4, 3, cell_index / 4)] = *palette_indices
            .get(biome_name)
            .expect("biome name should exist in the palette");
    }

    NbtTag::Compound(BTreeMap::from([
        ("Y".to_string(), NbtTag::Byte(section_y)),
        ("block_states".to_string(), air_only_block_states()),
        (
            "biomes".to_string(),
            NbtTag::Compound(BTreeMap::from([
                (
                    "palette".to_string(),
                    NbtTag::List(
                        8,
                        biome_palette
                            .into_iter()
                            .map(NbtTag::String)
                            .collect::<Vec<_>>(),
                    ),
                ),
                (
                    "data".to_string(),
                    NbtTag::LongArray(pack_indices(&biome_indices, 1)),
                ),
            ])),
        ),
    ]))
}

fn top_layer_cells<'a>(top_left: &'a str, fill: &'a str) -> Vec<&'a str> {
    let mut cells = vec![fill; 16];
    cells[0] = top_left;
    cells
}

fn air_only_block_states() -> NbtTag {
    NbtTag::Compound(BTreeMap::from([(
        "palette".to_string(),
        NbtTag::List(
            10,
            vec![NbtTag::Compound(BTreeMap::from([(
                "Name".to_string(),
                NbtTag::String("minecraft:air".to_string()),
            )]))],
        ),
    )]))
}

fn write_region_roots(
    region_dir: &Path,
    roots: &BTreeMap<ChunkPos, NbtTag>,
) -> Result<(), StorageError> {
    fs::create_dir_all(region_dir)?;
    let path = region_dir.join("r.0.0.mca");
    let mut locations = vec![0_u8; ANVIL_SECTOR_BYTES];
    let timestamps = vec![0_u8; ANVIL_SECTOR_BYTES];
    let mut sectors = Vec::new();
    let mut next_sector = 2_usize;

    for (chunk_pos, root) in roots {
        let payload = zlib_compress_nbt("", root)?;
        let total_bytes = 4 + 1 + payload.len();
        let sector_count = total_bytes.div_ceil(ANVIL_SECTOR_BYTES);
        let offset = next_sector;
        next_sector += sector_count;
        let location = ((u32::try_from(offset).expect("offset should fit")) << 8)
            | u32::try_from(sector_count).expect("sector count should fit");
        let index = region_chunk_index(*chunk_pos);
        locations[index * 4..index * 4 + 4].copy_from_slice(&location.to_be_bytes());

        let mut chunk_bytes = Vec::with_capacity(sector_count * ANVIL_SECTOR_BYTES);
        chunk_bytes.extend_from_slice(
            &u32::try_from(payload.len() + 1)
                .expect("payload length should fit")
                .to_be_bytes(),
        );
        chunk_bytes.push(2);
        chunk_bytes.extend_from_slice(&payload);
        chunk_bytes.extend(std::iter::repeat_n(
            0_u8,
            sector_count * ANVIL_SECTOR_BYTES - total_bytes,
        ));
        sectors.push(chunk_bytes);
    }

    let mut bytes = Vec::with_capacity(next_sector * ANVIL_SECTOR_BYTES);
    bytes.extend_from_slice(&locations);
    bytes.extend_from_slice(&timestamps);
    for sector in sectors {
        bytes.extend_from_slice(&sector);
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn compound_mut<'a>(
    root: &'a mut NbtTag,
    key: &str,
) -> Result<&'a mut BTreeMap<String, NbtTag>, StorageError> {
    let root = match root {
        NbtTag::Compound(root) => root,
        _ => {
            return Err(StorageError::InvalidData(
                "root nbt tag must be a compound".to_string(),
            ));
        }
    };
    match root.get_mut(key) {
        Some(NbtTag::Compound(compound)) => Ok(compound),
        _ => Err(StorageError::InvalidData(format!(
            "missing compound field {key}"
        ))),
    }
}

fn uuid_to_int_array(uuid: Uuid) -> Vec<i32> {
    uuid.as_bytes()
        .chunks_exact(4)
        .map(|chunk| i32::from_be_bytes(chunk.try_into().expect("uuid chunk should fit")))
        .collect()
}

fn biome_index(x: usize, z: usize) -> usize {
    z * 16 + x
}

fn biome_cell_index(x: usize, y: usize, z: usize) -> usize {
    x + z * 4 + y * 16
}

fn region_chunk_index(pos: ChunkPos) -> usize {
    let local_x = usize::try_from(pos.x.rem_euclid(32)).expect("x should fit");
    let local_z = usize::try_from(pos.z.rem_euclid(32)).expect("z should fit");
    local_x + local_z * 32
}

fn pack_indices(indices: &[usize], min_bits: usize) -> Vec<i64> {
    let palette_len = indices.iter().max().copied().unwrap_or(0) + 1;
    let bits = bits_per_value(palette_len, min_bits);
    let values_per_long = 64 / bits;
    let mask = (1_u64 << bits) - 1;
    let mut packed = vec![0_u64; indices.len().div_ceil(values_per_long)];
    for (index, palette_index) in indices.iter().copied().enumerate() {
        let long_index = index / values_per_long;
        let bit_offset = (index % values_per_long) * bits;
        packed[long_index] |=
            (u64::try_from(palette_index).expect("palette index should fit") & mask) << bit_offset;
    }
    packed
        .into_iter()
        .map(|value| i64::from_be_bytes(value.to_be_bytes()))
        .collect()
}

fn bits_per_value(palette_len: usize, min_bits: usize) -> usize {
    let required = if palette_len <= 1 {
        1
    } else {
        usize::try_from((palette_len - 1).ilog2() + 1).expect("bit width should fit")
    };
    required.max(min_bits)
}

fn tempdir() -> Result<TempDir, StorageError> {
    tempfile::Builder::new()
        .prefix("mc-plugin-storage-je-anvil-1_18_2-")
        .tempdir()
        .map_err(StorageError::from)
}
