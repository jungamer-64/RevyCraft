use crate::{get_nibble, legacy_block, semantic_block};
use flate2::Compression;
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::GzEncoder;
use mc_core::{
    BlockPos, ChunkColumn, ChunkPos, InventorySlot, PlayerId, PlayerInventory, PlayerSnapshot,
    WorldMeta, WorldSnapshot, expand_block_index,
};
use mc_proto_common::{StorageAdapter, StorageError};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::Path;
use uuid::Uuid;

const LEVEL_DAT: &str = "level.dat";
const PLAYERDATA_DIR: &str = "playerdata";
const REGION_DIR: &str = "region";
const ANVIL_SECTOR_BYTES: usize = 4096;
const ANVIL_HEADER_BYTES: usize = ANVIL_SECTOR_BYTES * 2;
const CHUNK_COMPRESSION_ZLIB: u8 = 2;
const PLAYERDATA_OFFHAND_SLOT: i8 = -106;

#[derive(Default)]
pub struct Je1710StorageAdapter;

impl StorageAdapter for Je1710StorageAdapter {
    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
        let level_path = world_dir.join(LEVEL_DAT);
        if !level_path.exists() {
            return Ok(None);
        }
        let meta = read_level_dat(&level_path)?;
        let chunks = read_regions(&world_dir.join(REGION_DIR))?;
        let players = read_playerdata(&world_dir.join(PLAYERDATA_DIR))?;
        Ok(Some(WorldSnapshot {
            meta,
            chunks,
            players,
        }))
    }

    fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        fs::create_dir_all(world_dir)?;
        write_level_dat(&world_dir.join(LEVEL_DAT), &snapshot.meta)?;
        write_regions(&world_dir.join(REGION_DIR), &snapshot.chunks)?;
        write_playerdata(&world_dir.join(PLAYERDATA_DIR), &snapshot.players)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
enum NbtTag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(u8, Vec<Self>),
    Compound(BTreeMap<String, Self>),
    IntArray(Vec<i32>),
}

fn write_level_dat(path: &Path, meta: &WorldMeta) -> Result<(), StorageError> {
    let mut data = BTreeMap::new();
    data.insert("version".to_string(), NbtTag::Int(19133));
    data.insert(
        "LevelName".to_string(),
        NbtTag::String(meta.level_name.clone()),
    );
    data.insert(
        "generatorName".to_string(),
        NbtTag::String(meta.level_type.to_ascii_lowercase()),
    );
    data.insert("SpawnX".to_string(), NbtTag::Int(meta.spawn.x));
    data.insert("SpawnY".to_string(), NbtTag::Int(meta.spawn.y));
    data.insert("SpawnZ".to_string(), NbtTag::Int(meta.spawn.z));
    data.insert(
        "RandomSeed".to_string(),
        NbtTag::Long(meta.seed.cast_signed()),
    );
    data.insert("Time".to_string(), NbtTag::Long(meta.time));
    data.insert(
        "GameType".to_string(),
        NbtTag::Int(i32::from(meta.game_mode)),
    );
    data.insert(
        "Difficulty".to_string(),
        NbtTag::Byte(i8::from_be_bytes([meta.difficulty])),
    );
    data.insert("MapFeatures".to_string(), NbtTag::Byte(0));
    data.insert("initialized".to_string(), NbtTag::Byte(1));

    let mut root = BTreeMap::new();
    root.insert("Data".to_string(), NbtTag::Compound(data));
    write_gzip_nbt(path, "", &NbtTag::Compound(root))
}

fn read_level_dat(path: &Path) -> Result<WorldMeta, StorageError> {
    let root = read_gzip_nbt(path)?;
    let data = compound_field(as_compound(&root)?, "Data")?;
    Ok(WorldMeta {
        level_name: string_field(data, "LevelName").unwrap_or_else(|_| "world".to_string()),
        seed: long_field(data, "RandomSeed").unwrap_or(0).cast_unsigned(),
        spawn: BlockPos::new(
            int_field(data, "SpawnX").unwrap_or(0),
            int_field(data, "SpawnY").unwrap_or(4),
            int_field(data, "SpawnZ").unwrap_or(0),
        ),
        dimension: mc_core::DimensionId::Overworld,
        age: long_field(data, "Time").unwrap_or(0),
        time: long_field(data, "Time").unwrap_or(0),
        level_type: string_field(data, "generatorName")
            .unwrap_or_else(|_| "flat".to_string())
            .to_ascii_uppercase(),
        game_mode: u8::try_from(int_field(data, "GameType").unwrap_or(0)).unwrap_or(0),
        difficulty: byte_field(data, "Difficulty").unwrap_or(1).to_be_bytes()[0],
        max_players: 20,
    })
}

fn write_playerdata(
    playerdata_dir: &Path,
    players: &BTreeMap<PlayerId, PlayerSnapshot>,
) -> Result<(), StorageError> {
    fs::create_dir_all(playerdata_dir)?;
    for player in players.values() {
        let path = playerdata_dir.join(format!("{}.dat", player.id.0.hyphenated()));
        let root = player_to_nbt(player);
        write_gzip_nbt(&path, "", &root)?;
    }
    Ok(())
}

fn read_playerdata(
    playerdata_dir: &Path,
) -> Result<BTreeMap<PlayerId, PlayerSnapshot>, StorageError> {
    let mut players = BTreeMap::new();
    if !playerdata_dir.exists() {
        return Ok(players);
    }
    for entry in fs::read_dir(playerdata_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("dat") {
            continue;
        }
        let player = player_from_nbt(&read_gzip_nbt(&path)?)?;
        players.insert(player.id, player);
    }
    Ok(players)
}

fn player_to_nbt(player: &PlayerSnapshot) -> NbtTag {
    let mut compound = BTreeMap::new();
    compound.insert(
        "Pos".to_string(),
        NbtTag::List(
            6,
            vec![
                NbtTag::Double(player.position.x),
                NbtTag::Double(player.position.y),
                NbtTag::Double(player.position.z),
            ],
        ),
    );
    compound.insert(
        "Rotation".to_string(),
        NbtTag::List(
            5,
            vec![NbtTag::Float(player.yaw), NbtTag::Float(player.pitch)],
        ),
    );
    let uuid_bytes = player.id.0.as_u128().to_be_bytes();
    let most = i64::from_be_bytes(uuid_bytes[0..8].try_into().expect("uuid most should fit"));
    let least = i64::from_be_bytes(uuid_bytes[8..16].try_into().expect("uuid least should fit"));
    compound.insert("UUIDMost".to_string(), NbtTag::Long(most));
    compound.insert("UUIDLeast".to_string(), NbtTag::Long(least));
    compound.insert("Dimension".to_string(), NbtTag::Int(0));
    compound.insert(
        "OnGround".to_string(),
        NbtTag::Byte(i8::from(player.on_ground)),
    );
    compound.insert("Health".to_string(), NbtTag::Float(player.health));
    compound.insert("foodLevel".to_string(), NbtTag::Int(i32::from(player.food)));
    compound.insert(
        "foodSaturationLevel".to_string(),
        NbtTag::Float(player.food_saturation),
    );
    compound.insert(
        "SelectedItemSlot".to_string(),
        NbtTag::Int(i32::from(player.selected_hotbar_slot)),
    );
    compound.insert(
        "Inventory".to_string(),
        NbtTag::List(10, inventory_to_nbt(&player.inventory)),
    );
    compound.insert("Name".to_string(), NbtTag::String(player.username.clone()));
    NbtTag::Compound(compound)
}

fn player_from_nbt(root: &NbtTag) -> Result<PlayerSnapshot, StorageError> {
    let compound = as_compound(root)?;
    let pos = list_field(compound, "Pos")?;
    let rotation = list_field(compound, "Rotation")?;
    let most = long_field(compound, "UUIDMost")?;
    let least = long_field(compound, "UUIDLeast")?;
    let mut uuid_bytes = [0_u8; 16];
    uuid_bytes[0..8].copy_from_slice(&most.to_be_bytes());
    uuid_bytes[8..16].copy_from_slice(&least.to_be_bytes());
    let inventory = compound
        .get("Inventory")
        .map(inventory_from_tag)
        .transpose()?
        .unwrap_or_else(PlayerInventory::creative_starter);
    Ok(PlayerSnapshot {
        id: PlayerId(Uuid::from_u128(u128::from_be_bytes(uuid_bytes))),
        username: string_field(compound, "Name").unwrap_or_else(|_| "player".to_string()),
        position: mc_core::Vec3::new(
            double_from_tag(&pos[0])?,
            double_from_tag(&pos[1])?,
            double_from_tag(&pos[2])?,
        ),
        yaw: float_from_tag(&rotation[0])?,
        pitch: float_from_tag(&rotation[1])?,
        on_ground: byte_field(compound, "OnGround").unwrap_or(1) != 0,
        dimension: mc_core::DimensionId::Overworld,
        health: float_field(compound, "Health").unwrap_or(20.0),
        food: i16::try_from(int_field(compound, "foodLevel").unwrap_or(20)).unwrap_or(20),
        food_saturation: float_field(compound, "foodSaturationLevel").unwrap_or(5.0),
        inventory,
        selected_hotbar_slot: u8::try_from(int_field(compound, "SelectedItemSlot").unwrap_or(0))
            .unwrap_or(0)
            .min(8),
    })
}

fn inventory_to_nbt(inventory: &PlayerInventory) -> Vec<NbtTag> {
    let mut entries: Vec<_> = inventory
        .slots
        .iter()
        .enumerate()
        .filter_map(|(window_slot, stack)| {
            let stack = stack.as_ref()?;
            let (item_id, damage) = crate::legacy_item(stack)?;
            let nbt_slot = window_slot_to_playerdata_slot(
                u8::try_from(window_slot).expect("window slot should fit into u8"),
            )?;
            let mut compound = BTreeMap::new();
            compound.insert("Slot".to_string(), NbtTag::Byte(nbt_slot));
            compound.insert("id".to_string(), NbtTag::Short(item_id));
            compound.insert(
                "Damage".to_string(),
                NbtTag::Short(i16::from_be_bytes(damage.to_be_bytes())),
            );
            compound.insert(
                "Count".to_string(),
                NbtTag::Byte(i8::try_from(stack.count).expect("count should fit into i8")),
            );
            Some(NbtTag::Compound(compound))
        })
        .collect();
    if let Some(stack) = inventory.offhand.as_ref()
        && let Some((item_id, damage)) = crate::legacy_item(stack)
    {
        let mut compound = BTreeMap::new();
        compound.insert("Slot".to_string(), NbtTag::Byte(PLAYERDATA_OFFHAND_SLOT));
        compound.insert("id".to_string(), NbtTag::Short(item_id));
        compound.insert(
            "Damage".to_string(),
            NbtTag::Short(i16::from_be_bytes(damage.to_be_bytes())),
        );
        compound.insert(
            "Count".to_string(),
            NbtTag::Byte(i8::try_from(stack.count).expect("count should fit into i8")),
        );
        entries.push(NbtTag::Compound(compound));
    }
    entries
}

fn inventory_from_tag(tag: &NbtTag) -> Result<PlayerInventory, StorageError> {
    let mut inventory = PlayerInventory::new_empty();
    let NbtTag::List(_, entries) = tag else {
        return Err(StorageError::InvalidData(
            "expected inventory list".to_string(),
        ));
    };
    for entry in entries {
        let compound = as_compound(entry)?;
        let slot = byte_field(compound, "Slot")?;
        let count = byte_field(compound, "Count").unwrap_or(0);
        if count <= 0 {
            continue;
        }
        let item_id = short_field(compound, "id")?;
        let damage = u16::from_be_bytes(short_field(compound, "Damage").unwrap_or(0).to_be_bytes());
        let stack = crate::semantic_item(item_id, damage, count.cast_unsigned());
        if stack.key.as_str() == "minecraft:unsupported" {
            continue;
        }
        if slot == PLAYERDATA_OFFHAND_SLOT {
            let _ = inventory.set_slot(InventorySlot::Offhand, Some(stack));
            continue;
        }
        let Some(window_slot) = playerdata_slot_to_window_slot(slot) else {
            continue;
        };
        let _ = inventory.set(window_slot, Some(stack));
    }
    Ok(inventory)
}

fn window_slot_to_playerdata_slot(window_slot: u8) -> Option<i8> {
    match window_slot {
        9..=35 => Some(i8::try_from(window_slot).expect("main inventory slot should fit into i8")),
        36..=44 => Some(i8::try_from(window_slot - 36).expect("hotbar slot should fit into i8")),
        _ => None,
    }
}

fn playerdata_slot_to_window_slot(slot: i8) -> Option<u8> {
    match slot {
        0..=8 => Some(36 + u8::try_from(slot).expect("hotbar slot should fit into u8")),
        9..=35 => Some(u8::try_from(slot).expect("main inventory slot should fit into u8")),
        _ => None,
    }
}

fn write_regions(
    region_dir: &Path,
    chunks: &BTreeMap<ChunkPos, ChunkColumn>,
) -> Result<(), StorageError> {
    fs::create_dir_all(region_dir)?;
    let mut grouped = BTreeMap::<(i32, i32), Vec<&ChunkColumn>>::new();
    for chunk in chunks.values() {
        grouped
            .entry((chunk.pos.x.div_euclid(32), chunk.pos.z.div_euclid(32)))
            .or_default()
            .push(chunk);
    }

    for ((region_x, region_z), region_chunks) in grouped {
        let path = region_dir.join(format!("r.{region_x}.{region_z}.mca"));
        write_region_file(&path, &region_chunks)?;
    }
    Ok(())
}

fn read_regions(region_dir: &Path) -> Result<BTreeMap<ChunkPos, ChunkColumn>, StorageError> {
    let mut chunks = BTreeMap::new();
    if !region_dir.exists() {
        return Ok(chunks);
    }
    for entry in fs::read_dir(region_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("mca") {
            continue;
        }
        for chunk in read_region_file(&path)? {
            chunks.insert(chunk.pos, chunk);
        }
    }
    Ok(chunks)
}

fn write_region_file(path: &Path, chunks: &[&ChunkColumn]) -> Result<(), StorageError> {
    let mut locations = vec![0_u8; ANVIL_SECTOR_BYTES];
    let mut timestamps = vec![0_u8; ANVIL_SECTOR_BYTES];
    let mut body = Vec::new();
    let mut sector_offset = 2_u32;

    for chunk in chunks {
        let index = region_chunk_index(chunk.pos);
        let chunk_nbt = chunk_to_nbt(chunk);
        let compressed = zlib_compress_nbt("", &chunk_nbt)?;
        let length = u32::try_from(compressed.len() + 1)
            .map_err(|_| StorageError::InvalidData("compressed chunk too large".to_string()))?;
        let total_bytes = usize::try_from(length + 4).expect("chunk length should fit into usize");
        let sector_count = total_bytes.div_ceil(ANVIL_SECTOR_BYTES);
        let location = (sector_offset << 8)
            | u32::try_from(sector_count).expect("sector count should fit into u32");
        locations[index * 4..index * 4 + 4].copy_from_slice(&location.to_be_bytes());
        timestamps[index * 4..index * 4 + 4].copy_from_slice(&0_u32.to_be_bytes());

        body.extend_from_slice(&length.to_be_bytes());
        body.push(CHUNK_COMPRESSION_ZLIB);
        body.extend_from_slice(&compressed);
        let padding = sector_count * ANVIL_SECTOR_BYTES - total_bytes;
        body.resize(body.len() + padding, 0);
        sector_offset = sector_offset
            .saturating_add(u32::try_from(sector_count).expect("sector count should fit into u32"));
    }

    let mut file = File::create(path)?;
    file.write_all(&locations)?;
    file.write_all(&timestamps)?;
    file.write_all(&body)?;
    Ok(())
}

fn read_region_file(path: &Path) -> Result<Vec<ChunkColumn>, StorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() < ANVIL_HEADER_BYTES {
        return Err(StorageError::InvalidData(
            "region file is too small".to_string(),
        ));
    }
    let mut chunks = Vec::new();
    for index in 0..1024 {
        let location = u32::from_be_bytes(
            bytes[index * 4..index * 4 + 4]
                .try_into()
                .expect("region location should fit"),
        );
        if location == 0 {
            continue;
        }
        let sector_offset =
            usize::try_from(location >> 8).expect("sector offset should fit into usize");
        let sector_count =
            usize::try_from(location & 0xff).expect("sector count should fit into usize");
        let start = sector_offset * ANVIL_SECTOR_BYTES;
        let end = start + sector_count * ANVIL_SECTOR_BYTES;
        if end > bytes.len() || start + 5 > end {
            continue;
        }
        let length = usize::try_from(u32::from_be_bytes(
            bytes[start..start + 4]
                .try_into()
                .expect("chunk length should fit"),
        ))
        .expect("chunk length should fit into usize");
        if length == 0 || start + 4 + length > end {
            continue;
        }
        let compression = bytes[start + 4];
        let payload = &bytes[start + 5..start + 4 + length];
        let decompressed = match compression {
            1 => decompress_gzip(payload)?,
            2 => decompress_zlib(payload)?,
            _ => {
                return Err(StorageError::InvalidData(
                    "unsupported region compression".to_string(),
                ));
            }
        };
        chunks.push(chunk_from_nbt(&read_nbt(&decompressed)?)?);
    }
    Ok(chunks)
}

fn chunk_to_nbt(chunk: &ChunkColumn) -> NbtTag {
    let mut level = BTreeMap::new();
    level.insert("xPos".to_string(), NbtTag::Int(chunk.pos.x));
    level.insert("zPos".to_string(), NbtTag::Int(chunk.pos.z));
    level.insert("TerrainPopulated".to_string(), NbtTag::Byte(1));
    level.insert("LightPopulated".to_string(), NbtTag::Byte(1));
    level.insert("LastUpdate".to_string(), NbtTag::Long(0));
    level.insert("InhabitedTime".to_string(), NbtTag::Long(0));
    level.insert(
        "Biomes".to_string(),
        NbtTag::ByteArray(chunk.biomes.clone()),
    );
    level.insert("HeightMap".to_string(), NbtTag::IntArray(vec![4; 256]));
    level.insert("Entities".to_string(), NbtTag::List(10, Vec::new()));
    level.insert("TileEntities".to_string(), NbtTag::List(10, Vec::new()));

    let sections = chunk
        .sections
        .iter()
        .filter(|(section_y, section)| **section_y >= 0 && **section_y < 16 && !section.is_empty())
        .map(|(section_y, section)| {
            let mut blocks = vec![0_u8; 4096];
            let mut data = vec![0_u8; 2048];
            let block_light = vec![0_u8; 2048];
            let sky_light = vec![0xff_u8; 2048];
            for (index, state) in section.iter_blocks() {
                let (block_id, metadata) = legacy_block(state);
                let index = usize::from(index);
                blocks[index] =
                    u8::try_from(block_id).expect("legacy block id should fit into byte");
                set_nibble(&mut data, index, metadata);
            }
            let mut section_compound = BTreeMap::new();
            section_compound.insert(
                "Y".to_string(),
                NbtTag::Byte(i8::try_from(*section_y).expect("section y should fit into i8")),
            );
            section_compound.insert("Blocks".to_string(), NbtTag::ByteArray(blocks));
            section_compound.insert("Data".to_string(), NbtTag::ByteArray(data));
            section_compound.insert("BlockLight".to_string(), NbtTag::ByteArray(block_light));
            section_compound.insert("SkyLight".to_string(), NbtTag::ByteArray(sky_light));
            NbtTag::Compound(section_compound)
        })
        .collect::<Vec<_>>();
    level.insert("Sections".to_string(), NbtTag::List(10, sections));

    let mut root = BTreeMap::new();
    root.insert("Level".to_string(), NbtTag::Compound(level));
    NbtTag::Compound(root)
}

fn chunk_from_nbt(root: &NbtTag) -> Result<ChunkColumn, StorageError> {
    let root = as_compound(root)?;
    let level = compound_field(root, "Level")?;
    let pos = ChunkPos::new(int_field(level, "xPos")?, int_field(level, "zPos")?);
    let mut chunk = ChunkColumn::new(pos);
    chunk.biomes = byte_array_field(level, "Biomes").unwrap_or_else(|_| vec![1; 256]);

    for section in list_field(level, "Sections")? {
        let section_compound = as_compound(section)?;
        let section_y = i32::from(byte_field(section_compound, "Y")?);
        let blocks = byte_array_field(section_compound, "Blocks")?;
        let metadata = byte_array_field(section_compound, "Data")?;
        for (index, block_id) in blocks.iter().copied().enumerate() {
            let metadata_value = get_nibble(&metadata, index);
            let state = semantic_block(u16::from(block_id), metadata_value);
            if state.is_air() {
                continue;
            }
            let (x, y, z) =
                expand_block_index(u16::try_from(index).expect("block index should fit into u16"));
            chunk.set_block(x, section_y * 16 + i32::from(y), z, state);
        }
    }
    Ok(chunk)
}

fn region_chunk_index(pos: ChunkPos) -> usize {
    let local_x =
        usize::try_from(pos.x.rem_euclid(32)).expect("local region x should fit into usize");
    let local_z =
        usize::try_from(pos.z.rem_euclid(32)).expect("local region z should fit into usize");
    local_x + local_z * 32
}

fn zlib_compress_nbt(name: &str, tag: &NbtTag) -> Result<Vec<u8>, StorageError> {
    let mut raw = Vec::new();
    write_named_tag(&mut raw, 10, name, tag)?;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw)?;
    Ok(encoder.finish()?)
}

fn write_gzip_nbt(path: &Path, name: &str, tag: &NbtTag) -> Result<(), StorageError> {
    let mut raw = Vec::new();
    write_named_tag(&mut raw, 10, name, tag)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw)?;
    fs::write(path, encoder.finish()?)?;
    Ok(())
}

fn read_gzip_nbt(path: &Path) -> Result<NbtTag, StorageError> {
    let bytes = fs::read(path)?;
    read_nbt(&decompress_gzip(&bytes)?)
}

fn decompress_gzip(bytes: &[u8]) -> Result<Vec<u8>, StorageError> {
    let mut decoder = GzDecoder::new(Cursor::new(bytes));
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    Ok(decompressed)
}

fn decompress_zlib(bytes: &[u8]) -> Result<Vec<u8>, StorageError> {
    let mut decoder = ZlibDecoder::new(Cursor::new(bytes));
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    Ok(decompressed)
}

fn read_nbt(bytes: &[u8]) -> Result<NbtTag, StorageError> {
    let mut cursor = Cursor::new(bytes);
    let tag_type = read_u8(&mut cursor)?;
    if tag_type != 10 {
        return Err(StorageError::InvalidData(
            "root nbt tag must be a compound".to_string(),
        ));
    }
    let _name = read_string_u16(&mut cursor)?;
    read_tag_payload(&mut cursor, tag_type)
}

fn write_named_tag(
    writer: &mut impl Write,
    tag_type: u8,
    name: &str,
    tag: &NbtTag,
) -> Result<(), StorageError> {
    write_u8(writer, tag_type)?;
    write_string_u16(writer, name)?;
    write_tag_payload(writer, tag)
}

fn write_tag_payload(writer: &mut impl Write, tag: &NbtTag) -> Result<(), StorageError> {
    match tag {
        NbtTag::Byte(value) => write_i8(writer, *value),
        NbtTag::Short(value) => write_i16(writer, *value),
        NbtTag::Int(value) => write_i32(writer, *value),
        NbtTag::Long(value) => write_i64(writer, *value),
        NbtTag::Float(value) => write_f32(writer, *value),
        NbtTag::Double(value) => write_f64(writer, *value),
        NbtTag::ByteArray(values) => {
            write_i32(
                writer,
                i32::try_from(values.len()).expect("byte array length should fit into i32"),
            )?;
            writer.write_all(values)?;
            Ok(())
        }
        NbtTag::String(value) => write_string_u16(writer, value),
        NbtTag::List(tag_type, values) => {
            write_u8(writer, *tag_type)?;
            write_i32(
                writer,
                i32::try_from(values.len()).expect("list length should fit into i32"),
            )?;
            for value in values {
                write_tag_payload(writer, value)?;
            }
            Ok(())
        }
        NbtTag::Compound(values) => {
            for (name, value) in values {
                let tag_type = tag_type(value);
                write_named_tag(writer, tag_type, name, value)?;
            }
            write_u8(writer, 0)?;
            Ok(())
        }
        NbtTag::IntArray(values) => {
            write_i32(
                writer,
                i32::try_from(values.len()).expect("int array length should fit into i32"),
            )?;
            for value in values {
                write_i32(writer, *value)?;
            }
            Ok(())
        }
    }
}

fn read_tag_payload(reader: &mut impl Read, tag_type: u8) -> Result<NbtTag, StorageError> {
    match tag_type {
        1 => Ok(NbtTag::Byte(read_i8(reader)?)),
        2 => Ok(NbtTag::Short(read_i16(reader)?)),
        3 => Ok(NbtTag::Int(read_i32(reader)?)),
        4 => Ok(NbtTag::Long(read_i64(reader)?)),
        5 => Ok(NbtTag::Float(read_f32(reader)?)),
        6 => Ok(NbtTag::Double(read_f64(reader)?)),
        7 => {
            let len = usize::try_from(read_i32(reader)?)
                .map_err(|_| StorageError::InvalidData("negative byte array length".to_string()))?;
            let mut bytes = vec![0_u8; len];
            reader.read_exact(&mut bytes)?;
            Ok(NbtTag::ByteArray(bytes))
        }
        8 => Ok(NbtTag::String(read_string_u16(reader)?)),
        9 => {
            let child_type = read_u8(reader)?;
            let len = usize::try_from(read_i32(reader)?)
                .map_err(|_| StorageError::InvalidData("negative list length".to_string()))?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(read_tag_payload(reader, child_type)?);
            }
            Ok(NbtTag::List(child_type, values))
        }
        10 => {
            let mut values = BTreeMap::new();
            loop {
                let child_type = read_u8(reader)?;
                if child_type == 0 {
                    break;
                }
                let name = read_string_u16(reader)?;
                values.insert(name, read_tag_payload(reader, child_type)?);
            }
            Ok(NbtTag::Compound(values))
        }
        11 => {
            let len = usize::try_from(read_i32(reader)?)
                .map_err(|_| StorageError::InvalidData("negative int array length".to_string()))?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(read_i32(reader)?);
            }
            Ok(NbtTag::IntArray(values))
        }
        _ => Err(StorageError::InvalidData(format!(
            "unsupported nbt tag type {tag_type}"
        ))),
    }
}

const fn tag_type(tag: &NbtTag) -> u8 {
    match tag {
        NbtTag::Byte(_) => 1,
        NbtTag::Short(_) => 2,
        NbtTag::Int(_) => 3,
        NbtTag::Long(_) => 4,
        NbtTag::Float(_) => 5,
        NbtTag::Double(_) => 6,
        NbtTag::ByteArray(_) => 7,
        NbtTag::String(_) => 8,
        NbtTag::List(_, _) => 9,
        NbtTag::Compound(_) => 10,
        NbtTag::IntArray(_) => 11,
    }
}

fn as_compound(tag: &NbtTag) -> Result<&BTreeMap<String, NbtTag>, StorageError> {
    match tag {
        NbtTag::Compound(values) => Ok(values),
        _ => Err(StorageError::InvalidData(
            "expected compound tag".to_string(),
        )),
    }
}

fn compound_field<'a>(
    compound: &'a BTreeMap<String, NbtTag>,
    key: &str,
) -> Result<&'a BTreeMap<String, NbtTag>, StorageError> {
    as_compound(
        compound
            .get(key)
            .ok_or_else(|| StorageError::InvalidData(format!("missing compound field {key}")))?,
    )
}

fn list_field<'a>(
    compound: &'a BTreeMap<String, NbtTag>,
    key: &str,
) -> Result<&'a [NbtTag], StorageError> {
    match compound.get(key) {
        Some(NbtTag::List(_, values)) => Ok(values),
        _ => Err(StorageError::InvalidData(format!(
            "missing list field {key}"
        ))),
    }
}

fn byte_array_field(
    compound: &BTreeMap<String, NbtTag>,
    key: &str,
) -> Result<Vec<u8>, StorageError> {
    match compound.get(key) {
        Some(NbtTag::ByteArray(values)) => Ok(values.clone()),
        _ => Err(StorageError::InvalidData(format!(
            "missing byte array field {key}"
        ))),
    }
}

fn string_field(compound: &BTreeMap<String, NbtTag>, key: &str) -> Result<String, StorageError> {
    match compound.get(key) {
        Some(NbtTag::String(value)) => Ok(value.clone()),
        _ => Err(StorageError::InvalidData(format!(
            "missing string field {key}"
        ))),
    }
}

fn int_field(compound: &BTreeMap<String, NbtTag>, key: &str) -> Result<i32, StorageError> {
    match compound.get(key) {
        Some(NbtTag::Int(value)) => Ok(*value),
        _ => Err(StorageError::InvalidData(format!(
            "missing int field {key}"
        ))),
    }
}

fn short_field(compound: &BTreeMap<String, NbtTag>, key: &str) -> Result<i16, StorageError> {
    match compound.get(key) {
        Some(NbtTag::Short(value)) => Ok(*value),
        _ => Err(StorageError::InvalidData(format!(
            "missing short field {key}"
        ))),
    }
}

fn long_field(compound: &BTreeMap<String, NbtTag>, key: &str) -> Result<i64, StorageError> {
    match compound.get(key) {
        Some(NbtTag::Long(value)) => Ok(*value),
        _ => Err(StorageError::InvalidData(format!(
            "missing long field {key}"
        ))),
    }
}

fn byte_field(compound: &BTreeMap<String, NbtTag>, key: &str) -> Result<i8, StorageError> {
    match compound.get(key) {
        Some(NbtTag::Byte(value)) => Ok(*value),
        _ => Err(StorageError::InvalidData(format!(
            "missing byte field {key}"
        ))),
    }
}

fn float_field(compound: &BTreeMap<String, NbtTag>, key: &str) -> Result<f32, StorageError> {
    match compound.get(key) {
        Some(NbtTag::Float(value)) => Ok(*value),
        _ => Err(StorageError::InvalidData(format!(
            "missing float field {key}"
        ))),
    }
}

fn double_from_tag(tag: &NbtTag) -> Result<f64, StorageError> {
    match tag {
        NbtTag::Double(value) => Ok(*value),
        _ => Err(StorageError::InvalidData("expected double tag".to_string())),
    }
}

fn float_from_tag(tag: &NbtTag) -> Result<f32, StorageError> {
    match tag {
        NbtTag::Float(value) => Ok(*value),
        _ => Err(StorageError::InvalidData("expected float tag".to_string())),
    }
}

fn read_u8(reader: &mut impl Read) -> Result<u8, StorageError> {
    let mut bytes = [0_u8; 1];
    reader.read_exact(&mut bytes)?;
    Ok(bytes[0])
}

fn read_i8(reader: &mut impl Read) -> Result<i8, StorageError> {
    Ok(i8::from_be_bytes([read_u8(reader)?]))
}

fn read_i16(reader: &mut impl Read) -> Result<i16, StorageError> {
    let mut bytes = [0_u8; 2];
    reader.read_exact(&mut bytes)?;
    Ok(i16::from_be_bytes(bytes))
}

fn read_i32(reader: &mut impl Read) -> Result<i32, StorageError> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(i32::from_be_bytes(bytes))
}

fn read_i64(reader: &mut impl Read) -> Result<i64, StorageError> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(i64::from_be_bytes(bytes))
}

fn read_f32(reader: &mut impl Read) -> Result<f32, StorageError> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(f32::from_be_bytes(bytes))
}

fn read_f64(reader: &mut impl Read) -> Result<f64, StorageError> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(f64::from_be_bytes(bytes))
}

fn read_string_u16(reader: &mut impl Read) -> Result<String, StorageError> {
    let mut len_bytes = [0_u8; 2];
    reader.read_exact(&mut len_bytes)?;
    let len = usize::from(u16::from_be_bytes(len_bytes));
    let mut bytes = vec![0_u8; len];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes)
        .map_err(|_| StorageError::InvalidData("invalid utf-8 string".to_string()))
}

fn write_i16(writer: &mut impl Write, value: i16) -> Result<(), StorageError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_u8(writer: &mut impl Write, value: u8) -> Result<(), StorageError> {
    writer.write_all(&[value])?;
    Ok(())
}

fn write_i8(writer: &mut impl Write, value: i8) -> Result<(), StorageError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_i32(writer: &mut impl Write, value: i32) -> Result<(), StorageError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_i64(writer: &mut impl Write, value: i64) -> Result<(), StorageError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_f32(writer: &mut impl Write, value: f32) -> Result<(), StorageError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_f64(writer: &mut impl Write, value: f64) -> Result<(), StorageError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_string_u16(writer: &mut impl Write, value: &str) -> Result<(), StorageError> {
    let len = u16::try_from(value.len())
        .map_err(|_| StorageError::InvalidData("nbt string too long".to_string()))?;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(value.as_bytes())?;
    Ok(())
}

fn set_nibble(target: &mut [u8], index: usize, value: u8) {
    let byte_index = index / 2;
    if index.is_multiple_of(2) {
        target[byte_index] = (target[byte_index] & 0xf0) | (value & 0x0f);
    } else {
        target[byte_index] = (target[byte_index] & 0x0f) | ((value & 0x0f) << 4);
    }
}

#[cfg(test)]
mod tests {
    use super::Je1710StorageAdapter;
    use mc_core::{
        ChunkColumn, ChunkPos, CoreConfig, InventorySlot, ItemStack, PlayerId, ServerCore,
    };
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
}
