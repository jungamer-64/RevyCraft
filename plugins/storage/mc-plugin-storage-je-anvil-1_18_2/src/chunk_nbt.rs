use super::JE_1_18_2_DATA_VERSION;
use super::JE_1_18_2_MAX_SECTION_Y;
use super::JE_1_18_2_MIN_SECTION_Y;
use super::nbt::{
    NbtTag, as_compound, byte_field, int_field, list_field, short_field, string_field,
};
use mc_content_canonical::catalog;
use mc_core::{
    BlockEntityKindId, BlockEntityState, BlockPos, BlockState, ChunkColumn, ChunkPos, ChunkSection,
    ContainerBlockEntityState, ContainerPropertyKey, ItemStack, expand_block_index,
};
use mc_proto_common::StorageError;
use std::collections::{BTreeMap, BTreeSet};

const CHEST_BLOCK_ENTITY_KIND: &str = "canonical:chest";
const FURNACE_BLOCK_ENTITY_KIND: &str = "canonical:furnace";
const FURNACE_BURN_LEFT: &str = "canonical:furnace.burn_left";
const FURNACE_BURN_MAX: &str = "canonical:furnace.burn_max";
const FURNACE_COOK_PROGRESS: &str = "canonical:furnace.cook_progress";
const FURNACE_COOK_TOTAL: &str = "canonical:furnace.cook_total";

pub(super) fn chunk_to_nbt(
    chunk: &ChunkColumn,
    block_entities: &BTreeMap<BlockPos, BlockEntityState>,
    existing_root: Option<&NbtTag>,
) -> Result<NbtTag, StorageError> {
    let existing_root = existing_root
        .map(as_compound)
        .transpose()?
        .cloned()
        .unwrap_or_default();
    let existing_sections = existing_section_map(&existing_root)?;
    let existing_block_entities = existing_block_entity_map(&existing_root)?;
    let existing_biomes = existing_chunk_biomes(&existing_sections);
    let section_ys = section_y_values(chunk, &existing_sections);
    let mut root = existing_root;
    root.insert(
        "DataVersion".to_string(),
        NbtTag::Int(JE_1_18_2_DATA_VERSION),
    );
    root.insert("xPos".to_string(), NbtTag::Int(chunk.pos.x));
    root.insert(
        "yPos".to_string(),
        NbtTag::Int(*section_ys.first().unwrap_or(&JE_1_18_2_MIN_SECTION_Y)),
    );
    root.insert("zPos".to_string(), NbtTag::Int(chunk.pos.z));
    root.entry("Status".to_string())
        .or_insert_with(|| NbtTag::String("full".to_string()));
    root.entry("LastUpdate".to_string())
        .or_insert_with(|| NbtTag::Long(0));
    root.entry("InhabitedTime".to_string())
        .or_insert_with(|| NbtTag::Long(0));
    root.entry("entities".to_string())
        .or_insert_with(|| NbtTag::List(10, Vec::new()));
    root.entry("block_ticks".to_string())
        .or_insert_with(|| NbtTag::List(10, Vec::new()));
    root.entry("fluid_ticks".to_string())
        .or_insert_with(|| NbtTag::List(10, Vec::new()));
    root.entry("structures".to_string())
        .or_insert_with(|| NbtTag::Compound(BTreeMap::new()));

    let mut sections = Vec::with_capacity(section_ys.len());
    for section_y in section_ys {
        sections.push(NbtTag::Compound(encode_section(
            section_y,
            chunk.sections.get(&section_y),
            existing_sections.get(&section_y),
            existing_biomes
                .get(&section_y)
                .cloned()
                .unwrap_or_else(|| uniform_biome_name(chunk)),
        )?));
    }
    root.insert("sections".to_string(), NbtTag::List(10, sections));
    root.insert(
        "block_entities".to_string(),
        NbtTag::List(
            10,
            encode_block_entities(chunk, block_entities, &existing_block_entities)?,
        ),
    );
    Ok(NbtTag::Compound(root))
}

pub(super) fn chunk_from_nbt(
    root: &NbtTag,
) -> Result<(ChunkColumn, BTreeMap<BlockPos, BlockEntityState>), StorageError> {
    let root = as_compound(root)?;
    validate_chunk_data_version(root)?;
    let pos = ChunkPos::new(int_field(root, "xPos")?, int_field(root, "zPos")?);
    let mut chunk = ChunkColumn::new(pos);
    let mut section_top_biomes = BTreeMap::new();

    for section_tag in list_field(root, "sections")? {
        let section = as_compound(section_tag)?;
        let section_y = i32::from(byte_field(section, "Y")?);
        decode_section_blocks(&mut chunk, section_y, section)?;
        if let Some(top_biome_layer) = top_biome_layer(section)? {
            section_top_biomes.insert(section_y, top_biome_layer);
        }
    }
    chunk.biomes = decode_column_biomes(&section_top_biomes);

    let mut block_entities = BTreeMap::new();
    if let Ok(entries) = list_field(root, "block_entities") {
        for entry in entries {
            let (position, block_entity) = decode_block_entity(entry)?;
            block_entities.insert(position, block_entity);
        }
    }
    Ok((chunk, block_entities))
}

fn validate_chunk_data_version(root: &BTreeMap<String, NbtTag>) -> Result<(), StorageError> {
    let data_version = int_field(root, "DataVersion")?;
    if data_version != JE_1_18_2_DATA_VERSION {
        return Err(StorageError::InvalidData(format!(
            "expected chunk DataVersion={}, got {data_version}",
            JE_1_18_2_DATA_VERSION
        )));
    }
    Ok(())
}

fn decode_section_blocks(
    chunk: &mut ChunkColumn,
    section_y: i32,
    section: &BTreeMap<String, NbtTag>,
) -> Result<(), StorageError> {
    let Some(block_states_tag) = section.get("block_states") else {
        return Ok(());
    };
    let block_states = as_compound(block_states_tag)?;
    let palette = block_state_palette(block_states)?;
    let indices = unpack_indices(palette.len(), 4096, 4, block_states.get("data"))?;
    for (index, palette_index) in indices.into_iter().enumerate() {
        let state = palette.get(palette_index).ok_or_else(|| {
            StorageError::InvalidData("block state palette index was out of bounds".to_string())
        })?;
        if state.key.as_str() == catalog::AIR {
            continue;
        }
        let (x, y, z) =
            expand_block_index(u16::try_from(index).expect("block index should fit into u16"));
        chunk.set_block(x, section_y * 16 + i32::from(y), z, Some(state.clone()));
    }
    Ok(())
}

fn top_biome_layer(
    section: &BTreeMap<String, NbtTag>,
) -> Result<Option<Vec<String>>, StorageError> {
    let Some(biomes_tag) = section.get("biomes") else {
        return Ok(None);
    };
    let biomes = as_compound(biomes_tag)?;
    let palette = biome_palette(biomes)?;
    let indices = unpack_indices(palette.len(), 64, 1, biomes.get("data"))?;
    let mut top_layer = Vec::with_capacity(16);
    for z_cell in 0..4 {
        for x_cell in 0..4 {
            let palette_index = indices
                .get(biome_cell_index(x_cell, 3, z_cell))
                .copied()
                .ok_or_else(|| {
                    StorageError::InvalidData("biome data was missing the top cell".to_string())
                })?;
            let biome_name = palette.get(palette_index).cloned().ok_or_else(|| {
                StorageError::InvalidData("biome palette index was out of bounds".to_string())
            })?;
            top_layer.push(biome_name);
        }
    }
    Ok(Some(top_layer))
}

fn block_state_palette(
    block_states: &BTreeMap<String, NbtTag>,
) -> Result<Vec<BlockState>, StorageError> {
    let entries = list_field(block_states, "palette")?;
    let mut palette = Vec::with_capacity(entries.len());
    for entry in entries {
        let entry = as_compound(entry)?;
        let mut state = BlockState::new(string_field(entry, "Name")?);
        if let Some(properties_tag) = entry.get("Properties") {
            let properties = as_compound(properties_tag)?;
            for (key, value) in properties {
                let NbtTag::String(value) = value else {
                    return Err(StorageError::InvalidData(format!(
                        "block property `{key}` was not a string"
                    )));
                };
                state.properties.insert(key.clone(), value.clone());
            }
        }
        palette.push(state);
    }
    if palette.is_empty() {
        return Err(StorageError::InvalidData(
            "block state palette was empty".to_string(),
        ));
    }
    Ok(palette)
}

fn biome_palette(biomes: &BTreeMap<String, NbtTag>) -> Result<Vec<String>, StorageError> {
    let entries = list_field(biomes, "palette")?;
    let mut palette = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            NbtTag::String(value) => palette.push(value.clone()),
            _ => {
                return Err(StorageError::InvalidData(
                    "biome palette entry was not a string".to_string(),
                ));
            }
        }
    }
    if palette.is_empty() {
        return Err(StorageError::InvalidData(
            "biome palette was empty".to_string(),
        ));
    }
    Ok(palette)
}

fn unpack_indices(
    palette_len: usize,
    value_count: usize,
    min_bits: usize,
    data_tag: Option<&NbtTag>,
) -> Result<Vec<usize>, StorageError> {
    if palette_len == 1 {
        return Ok(vec![0; value_count]);
    }
    let Some(NbtTag::LongArray(values)) = data_tag else {
        return Err(StorageError::InvalidData(
            "palette data was missing".to_string(),
        ));
    };
    let bits = bits_per_value(palette_len, min_bits);
    let values_per_long = 64 / bits;
    let expected_len = value_count.div_ceil(values_per_long);
    if values.len() != expected_len {
        return Err(StorageError::InvalidData(format!(
            "expected {expected_len} packed longs, got {}",
            values.len()
        )));
    }
    let mask = (1_u64 << bits) - 1;
    let mut unpacked = Vec::with_capacity(value_count);
    for index in 0..value_count {
        let long_index = index / values_per_long;
        let bit_offset = (index % values_per_long) * bits;
        let raw = u64::from_be_bytes(values[long_index].to_be_bytes());
        let value = ((raw >> bit_offset) & mask) as usize;
        if value >= palette_len {
            return Err(StorageError::InvalidData(
                "palette index was out of bounds".to_string(),
            ));
        }
        unpacked.push(value);
    }
    Ok(unpacked)
}

fn existing_section_map(
    root: &BTreeMap<String, NbtTag>,
) -> Result<BTreeMap<i32, BTreeMap<String, NbtTag>>, StorageError> {
    let mut sections = BTreeMap::new();
    let Some(sections_tag) = root.get("sections") else {
        return Ok(sections);
    };
    let NbtTag::List(_, section_list) = sections_tag else {
        return Err(StorageError::InvalidData(
            "chunk sections was not a list".to_string(),
        ));
    };
    for section in section_list {
        let section = as_compound(section)?;
        sections.insert(i32::from(byte_field(section, "Y")?), section.clone());
    }
    Ok(sections)
}

fn existing_block_entity_map(
    root: &BTreeMap<String, NbtTag>,
) -> Result<BTreeMap<BlockPos, BTreeMap<String, NbtTag>>, StorageError> {
    let mut block_entities = BTreeMap::new();
    let Some(block_entities_tag) = root.get("block_entities") else {
        return Ok(block_entities);
    };
    let NbtTag::List(_, entries) = block_entities_tag else {
        return Err(StorageError::InvalidData(
            "chunk block_entities was not a list".to_string(),
        ));
    };
    for entry in entries {
        let compound = as_compound(entry)?;
        let position = BlockPos::new(
            int_field(compound, "x")?,
            int_field(compound, "y")?,
            int_field(compound, "z")?,
        );
        block_entities.insert(position, compound.clone());
    }
    Ok(block_entities)
}

fn existing_chunk_biomes(
    sections: &BTreeMap<i32, BTreeMap<String, NbtTag>>,
) -> BTreeMap<i32, String> {
    sections
        .iter()
        .filter_map(|(section_y, section)| {
            let biome_name = section
                .get("biomes")
                .and_then(|biomes| as_compound(biomes).ok())
                .and_then(|biomes| biome_palette(biomes).ok())
                .and_then(|palette| palette.first().cloned());
            biome_name.map(|biome_name| (*section_y, biome_name))
        })
        .collect()
}

fn section_y_values(
    chunk: &ChunkColumn,
    existing_sections: &BTreeMap<i32, BTreeMap<String, NbtTag>>,
) -> Vec<i32> {
    let mut section_ys = BTreeSet::new();
    if existing_sections.is_empty() {
        for section_y in JE_1_18_2_MIN_SECTION_Y..=JE_1_18_2_MAX_SECTION_Y {
            section_ys.insert(section_y);
        }
    } else {
        section_ys.extend(existing_sections.keys().copied());
    }
    section_ys.extend(chunk.sections.keys().copied());
    section_ys.into_iter().collect()
}

fn encode_section(
    section_y: i32,
    snapshot_section: Option<&ChunkSection>,
    existing_section: Option<&BTreeMap<String, NbtTag>>,
    biome_name: String,
) -> Result<BTreeMap<String, NbtTag>, StorageError> {
    let mut section = existing_section.cloned().unwrap_or_default();
    section.insert(
        "Y".to_string(),
        NbtTag::Byte(i8::try_from(section_y).expect("section y should fit into i8")),
    );
    section.insert(
        "block_states".to_string(),
        encode_block_states(snapshot_section),
    );
    if !section.contains_key("biomes") {
        section.insert("biomes".to_string(), uniform_biomes(&biome_name));
    }
    Ok(section)
}

fn encode_block_states(snapshot_section: Option<&ChunkSection>) -> NbtTag {
    let mut palette = vec![BlockState::new(catalog::AIR)];
    let mut indices = vec![0_usize; 4096];

    if let Some(snapshot_section) = snapshot_section {
        for (index, state) in snapshot_section.iter_blocks() {
            let palette_index = palette
                .iter()
                .position(|palette_state| palette_state == state)
                .unwrap_or_else(|| {
                    palette.push(state.clone());
                    palette.len() - 1
                });
            indices[usize::from(index)] = palette_index;
        }
    }

    let mut block_states = BTreeMap::new();
    block_states.insert(
        "palette".to_string(),
        NbtTag::List(10, palette.iter().map(encode_palette_entry).collect()),
    );
    if palette.len() > 1 {
        block_states.insert(
            "data".to_string(),
            NbtTag::LongArray(pack_indices(&indices, 4)),
        );
    }
    NbtTag::Compound(block_states)
}

fn encode_palette_entry(state: &BlockState) -> NbtTag {
    let mut entry = BTreeMap::new();
    entry.insert(
        "Name".to_string(),
        NbtTag::String(state.key.as_str().to_string()),
    );
    if !state.properties.is_empty() {
        entry.insert(
            "Properties".to_string(),
            NbtTag::Compound(
                state
                    .properties
                    .iter()
                    .map(|(key, value)| (key.clone(), NbtTag::String(value.clone())))
                    .collect(),
            ),
        );
    }
    NbtTag::Compound(entry)
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
        usize::try_from((palette_len - 1).ilog2() + 1).expect("bit width should fit into usize")
    };
    required.max(min_bits)
}

fn uniform_biomes(biome_name: &str) -> NbtTag {
    let mut biomes = BTreeMap::new();
    biomes.insert(
        "palette".to_string(),
        NbtTag::List(8, vec![NbtTag::String(biome_name.to_string())]),
    );
    NbtTag::Compound(biomes)
}

fn uniform_biome_name(chunk: &ChunkColumn) -> String {
    let biome_id = chunk.biomes.first().copied().unwrap_or(1);
    modern_biome_name(biome_id).to_string()
}

fn modern_biome_name(biome_id: u8) -> &'static str {
    match biome_id {
        1 => "minecraft:plains",
        2 => "minecraft:desert",
        4 => "minecraft:forest",
        5 => "minecraft:taiga",
        6 => "minecraft:swamp",
        _ => "minecraft:plains",
    }
}

fn legacy_biome_id(biome_name: &str) -> u8 {
    match biome_name {
        "minecraft:desert" => 2,
        "minecraft:forest" => 4,
        "minecraft:taiga" => 5,
        "minecraft:swamp" => 6,
        _ => 1,
    }
}

fn decode_column_biomes(section_top_biomes: &BTreeMap<i32, Vec<String>>) -> Vec<u8> {
    let mut biomes = vec![1; 256];
    for z in 0..16 {
        for x in 0..16 {
            let biome_name = top_column_biome_name(section_top_biomes, x, z);
            biomes[z * 16 + x] = legacy_biome_id(biome_name);
        }
    }
    biomes
}

fn encode_block_entities(
    chunk: &ChunkColumn,
    block_entities: &BTreeMap<BlockPos, BlockEntityState>,
    existing_block_entities: &BTreeMap<BlockPos, BTreeMap<String, NbtTag>>,
) -> Result<Vec<NbtTag>, StorageError> {
    let mut encoded = Vec::new();
    for (position, block_entity) in block_entities {
        if position.chunk_pos() != chunk.pos {
            continue;
        }
        encoded.push(encode_block_entity(
            *position,
            block_entity,
            existing_block_entities.get(position),
        )?);
    }
    Ok(encoded)
}

fn encode_block_entity(
    position: BlockPos,
    block_entity: &BlockEntityState,
    existing_compound: Option<&BTreeMap<String, NbtTag>>,
) -> Result<NbtTag, StorageError> {
    let mut compound = existing_compound
        .filter(|existing| block_entity_kind_matches(existing, block_entity))
        .cloned()
        .unwrap_or_default();
    compound.insert("x".to_string(), NbtTag::Int(position.x));
    compound.insert("y".to_string(), NbtTag::Int(position.y));
    compound.insert("z".to_string(), NbtTag::Int(position.z));
    match block_entity {
        BlockEntityState::Container(container) => match container.kind.as_str() {
            CHEST_BLOCK_ENTITY_KIND => {
                compound.insert(
                    "id".to_string(),
                    NbtTag::String("minecraft:chest".to_string()),
                );
                compound.insert(
                    "Items".to_string(),
                    NbtTag::List(
                        10,
                        container
                            .slots
                            .iter()
                            .enumerate()
                            .filter_map(|(index, stack)| {
                                stack.as_ref().map(|stack| encode_item_slot(index, stack))
                            })
                            .collect(),
                    ),
                );
            }
            "canonical:furnace" => {
                compound.insert(
                    "id".to_string(),
                    NbtTag::String("minecraft:furnace".to_string()),
                );
                compound.insert(
                    "Items".to_string(),
                    NbtTag::List(
                        10,
                        container
                            .slots
                            .iter()
                            .take(3)
                            .enumerate()
                            .filter_map(|(index, stack)| {
                                stack.as_ref().map(|stack| encode_item_slot(index, stack))
                            })
                            .collect(),
                    ),
                );
                compound.insert(
                    "BurnTime".to_string(),
                    NbtTag::Short(container_property(container, "canonical:furnace.burn_left")),
                );
                compound.insert(
                    "BurnTime".to_string(),
                    NbtTag::Short(container_property(container, FURNACE_BURN_LEFT)),
                );
                compound.insert(
                    "BurnTimeMax".to_string(),
                    NbtTag::Short(container_property(container, FURNACE_BURN_MAX)),
                );
                compound.insert(
                    "CookTime".to_string(),
                    NbtTag::Short(container_property(container, FURNACE_COOK_PROGRESS)),
                );
                compound.insert(
                    "CookTimeTotal".to_string(),
                    NbtTag::Short(container_property(container, FURNACE_COOK_TOTAL)),
                );
            }
            kind => {
                return Err(StorageError::InvalidData(format!(
                    "unsupported container block entity kind `{kind}`"
                )));
            }
        },
    }
    Ok(NbtTag::Compound(compound))
}

fn block_entity_kind_matches(
    existing_compound: &BTreeMap<String, NbtTag>,
    block_entity: &BlockEntityState,
) -> bool {
    match string_field(existing_compound, "id").ok().as_deref() {
        Some("minecraft:chest") | Some("Chest") => {
            matches!(
                block_entity,
                BlockEntityState::Container(container)
                    if container.kind.as_str() == CHEST_BLOCK_ENTITY_KIND
            )
        }
        Some("minecraft:furnace") | Some("Furnace") => {
            matches!(
                block_entity,
                BlockEntityState::Container(container)
                    if container.kind.as_str() == FURNACE_BLOCK_ENTITY_KIND
            )
        }
        _ => false,
    }
}

fn container_property(container: &mc_core::ContainerBlockEntityState, key: &str) -> i16 {
    container
        .properties
        .get(&mc_core::ContainerPropertyKey::new(key))
        .copied()
        .unwrap_or_default()
}

fn encode_item_slot(index: usize, stack: &ItemStack) -> NbtTag {
    let mut compound = BTreeMap::new();
    compound.insert(
        "Slot".to_string(),
        NbtTag::Byte(i8::try_from(index).expect("slot index should fit into i8")),
    );
    compound.insert(
        "id".to_string(),
        NbtTag::String(stack.key.as_str().to_string()),
    );
    compound.insert(
        "Count".to_string(),
        NbtTag::Byte(i8::try_from(stack.count).expect("count should fit into i8")),
    );
    if stack.damage != 0 {
        compound.insert("Damage".to_string(), NbtTag::Int(i32::from(stack.damage)));
    }
    NbtTag::Compound(compound)
}

fn decode_block_entity(entry: &NbtTag) -> Result<(BlockPos, BlockEntityState), StorageError> {
    let compound = as_compound(entry)?;
    let id = string_field(compound, "id")?;
    let position = BlockPos::new(
        int_field(compound, "x")?,
        int_field(compound, "y")?,
        int_field(compound, "z")?,
    );
    let items = list_field(compound, "Items").unwrap_or(&[]);
    match id.as_str() {
        "minecraft:chest" | "Chest" => Ok((
            position,
            BlockEntityState::Container(ContainerBlockEntityState {
                kind: BlockEntityKindId::new(CHEST_BLOCK_ENTITY_KIND),
                slots: decode_container_items(items, 27)?,
                properties: BTreeMap::new(),
            }),
        )),
        "minecraft:furnace" | "Furnace" => {
            let slots = decode_container_items(items, 3)?;
            Ok((
                position,
                BlockEntityState::Container(ContainerBlockEntityState {
                    kind: BlockEntityKindId::new(FURNACE_BLOCK_ENTITY_KIND),
                    slots,
                    properties: BTreeMap::from([
                        (
                            ContainerPropertyKey::new(FURNACE_BURN_LEFT),
                            short_field(compound, "BurnTime").unwrap_or(0),
                        ),
                        (
                            ContainerPropertyKey::new(FURNACE_BURN_MAX),
                            short_field(compound, "BurnTimeMax")
                                .unwrap_or_else(|_| short_field(compound, "BurnTime").unwrap_or(0)),
                        ),
                        (
                            ContainerPropertyKey::new(FURNACE_COOK_PROGRESS),
                            short_field(compound, "CookTime").unwrap_or(0),
                        ),
                        (
                            ContainerPropertyKey::new(FURNACE_COOK_TOTAL),
                            short_field(compound, "CookTimeTotal").unwrap_or(200),
                        ),
                    ]),
                }),
            ))
        }
        _ => Err(StorageError::InvalidData(format!(
            "unsupported block entity `{id}`"
        ))),
    }
}

fn decode_container_items(
    items: &[NbtTag],
    slot_count: usize,
) -> Result<Vec<Option<ItemStack>>, StorageError> {
    let mut slots = vec![None; slot_count];
    for item in items {
        let item = as_compound(item)?;
        validate_item_keys(item)?;
        if item.contains_key("tag") {
            return Err(StorageError::InvalidData(
                "item tag is not supported".to_string(),
            ));
        }
        let slot = usize::try_from(byte_field(item, "Slot")?)
            .map_err(|_| StorageError::InvalidData("negative item slot index".to_string()))?;
        if slot >= slots.len() {
            return Err(StorageError::InvalidData(format!(
                "item slot {slot} was out of bounds"
            )));
        }
        slots[slot] = Some(item_stack_from_nbt(item)?);
    }
    Ok(slots)
}

fn validate_item_keys(compound: &BTreeMap<String, NbtTag>) -> Result<(), StorageError> {
    for key in compound.keys() {
        if !matches!(key.as_str(), "Slot" | "id" | "Count" | "Damage" | "tag") {
            return Err(StorageError::InvalidData(format!(
                "unsupported item field `{key}`"
            )));
        }
    }
    Ok(())
}

fn item_stack_from_nbt(compound: &BTreeMap<String, NbtTag>) -> Result<ItemStack, StorageError> {
    let key = string_field(compound, "id")?;
    let count = u8::try_from(byte_field(compound, "Count")?)
        .map_err(|_| StorageError::InvalidData("negative item count not supported".to_string()))?;
    let damage = match compound.get("Damage") {
        Some(NbtTag::Short(value)) => u16::try_from(*value).map_err(|_| {
            StorageError::InvalidData("negative item damage not supported".to_string())
        })?,
        Some(NbtTag::Int(value)) => u16::try_from(*value).map_err(|_| {
            StorageError::InvalidData("item damage did not fit into u16".to_string())
        })?,
        Some(_) => {
            return Err(StorageError::InvalidData(
                "item Damage field had an unsupported type".to_string(),
            ));
        }
        None => 0,
    };
    Ok(ItemStack::new(key, count, damage))
}

fn biome_cell_index(x: usize, y: usize, z: usize) -> usize {
    x + z * 4 + y * 16
}

fn top_column_biome_name<'a>(
    section_top_biomes: &'a BTreeMap<i32, Vec<String>>,
    x: usize,
    z: usize,
) -> &'a str {
    let cell_index = (x / 4) + (z / 4) * 4;
    section_top_biomes
        .iter()
        .rev()
        .find_map(|(_, top_layer)| top_layer.get(cell_index))
        .map_or("minecraft:plains", String::as_str)
}
