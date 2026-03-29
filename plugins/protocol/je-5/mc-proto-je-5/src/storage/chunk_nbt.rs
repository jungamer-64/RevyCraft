use super::nbt::{
    NbtTag, as_compound, byte_array_field, byte_field, compound_field, int_field, list_field,
    short_field, string_field,
};
use mc_proto_common::StorageError;
use mc_proto_je_common::__version_support::{
    blocks::{legacy_block, legacy_item, semantic_block, semantic_item},
    chunks::get_nibble,
};
use revy_voxel_model::{BlockPos, ChunkColumn, ChunkPos, ItemStack, expand_block_index};
use revy_voxel_rules::{
    BlockEntityKindId, BlockEntityState, ContainerBlockEntityState, ContainerPropertyKey,
};
use std::collections::BTreeMap;

const CHEST_BLOCK_ENTITY_KIND: &str = "canonical:chest";
const FURNACE_BLOCK_ENTITY_KIND: &str = "canonical:furnace";
const FURNACE_BURN_LEFT: &str = "canonical:furnace.burn_left";
const FURNACE_BURN_MAX: &str = "canonical:furnace.burn_max";
const FURNACE_COOK_PROGRESS: &str = "canonical:furnace.cook_progress";
const FURNACE_COOK_TOTAL: &str = "canonical:furnace.cook_total";

pub(super) fn chunk_to_nbt(
    chunk: &ChunkColumn,
    block_entities: &BTreeMap<BlockPos, BlockEntityState>,
) -> NbtTag {
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
    level.insert(
        "TileEntities".to_string(),
        NbtTag::List(
            10,
            block_entities
                .iter()
                .filter(|(position, _): &(&BlockPos, &BlockEntityState)| {
                    position.chunk_pos() == chunk.pos
                })
                .filter_map(|(position, block_entity)| encode_tile_entity(*position, block_entity))
                .collect(),
        ),
    );

    let sections = chunk
        .sections
        .iter()
        .filter(
            |(section_y, section): &(&i32, &revy_voxel_model::ChunkSection)| {
                **section_y >= 0 && **section_y < 16 && !section.is_empty()
            },
        )
        .map(
            |(section_y, section): (&i32, &revy_voxel_model::ChunkSection)| {
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
            },
        )
        .collect::<Vec<_>>();
    level.insert("Sections".to_string(), NbtTag::List(10, sections));

    let mut root = BTreeMap::new();
    root.insert("Level".to_string(), NbtTag::Compound(level));
    NbtTag::Compound(root)
}

pub(super) fn chunk_from_nbt(
    root: &NbtTag,
) -> Result<(ChunkColumn, BTreeMap<BlockPos, BlockEntityState>), StorageError> {
    let root = as_compound(root)?;
    let level = compound_field(root, "Level")?;
    let pos = ChunkPos::new(int_field(level, "xPos")?, int_field(level, "zPos")?);
    let mut chunk = ChunkColumn::new(pos);
    let mut block_entities = BTreeMap::new();
    chunk.biomes = byte_array_field(level, "Biomes").unwrap_or_else(|_| vec![1; 256]);

    for section in list_field(level, "Sections")? {
        let section_compound = as_compound(section)?;
        let section_y = i32::from(byte_field(section_compound, "Y")?);
        let blocks = byte_array_field(section_compound, "Blocks")?;
        let metadata = byte_array_field(section_compound, "Data")?;
        for (index, block_id) in blocks.iter().copied().enumerate() {
            let metadata_value = get_nibble(&metadata, index);
            let state = semantic_block(u16::from(block_id), metadata_value);
            if state.key.as_str() == "minecraft:air" {
                continue;
            }
            let (x, y, z) =
                expand_block_index(u16::try_from(index).expect("block index should fit into u16"));
            chunk.set_block(x, section_y * 16 + i32::from(y), z, Some(state));
        }
    }

    if let Ok(tile_entities) = list_field(level, "TileEntities") {
        for tile_entity in tile_entities {
            let Some((position, block_entity)) = decode_tile_entity(tile_entity)? else {
                continue;
            };
            block_entities.insert(position, block_entity);
        }
    }

    Ok((chunk, block_entities))
}

pub(super) fn region_chunk_index(pos: ChunkPos) -> usize {
    let local_x =
        usize::try_from(pos.x.rem_euclid(32)).expect("local region x should fit into usize");
    let local_z =
        usize::try_from(pos.z.rem_euclid(32)).expect("local region z should fit into usize");
    local_x + local_z * 32
}

fn set_nibble(target: &mut [u8], index: usize, value: u8) {
    let byte_index = index / 2;
    if index.is_multiple_of(2) {
        target[byte_index] = (target[byte_index] & 0xf0) | (value & 0x0f);
    } else {
        target[byte_index] = (target[byte_index] & 0x0f) | ((value & 0x0f) << 4);
    }
}

fn encode_tile_entity(position: BlockPos, block_entity: &BlockEntityState) -> Option<NbtTag> {
    match block_entity {
        BlockEntityState::Container(container) => match container.kind.as_str() {
            CHEST_BLOCK_ENTITY_KIND => {
                let mut compound = BTreeMap::new();
                compound.insert("id".to_string(), NbtTag::String("Chest".to_string()));
                compound.insert("x".to_string(), NbtTag::Int(position.x));
                compound.insert("y".to_string(), NbtTag::Int(position.y));
                compound.insert("z".to_string(), NbtTag::Int(position.z));
                compound.insert(
                    "Items".to_string(),
                    NbtTag::List(
                        10,
                        container
                            .slots
                            .iter()
                            .enumerate()
                            .filter_map(|(index, stack): (usize, &Option<ItemStack>)| {
                                encode_item_slot(index, stack.as_ref())
                            })
                            .collect(),
                    ),
                );
                Some(NbtTag::Compound(compound))
            }
            FURNACE_BLOCK_ENTITY_KIND => {
                let mut compound = BTreeMap::new();
                compound.insert("id".to_string(), NbtTag::String("Furnace".to_string()));
                compound.insert("x".to_string(), NbtTag::Int(position.x));
                compound.insert("y".to_string(), NbtTag::Int(position.y));
                compound.insert("z".to_string(), NbtTag::Int(position.z));
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
                compound.insert(
                    "Items".to_string(),
                    NbtTag::List(
                        10,
                        container
                            .slots
                            .iter()
                            .take(3)
                            .enumerate()
                            .filter_map(|(index, stack): (usize, &Option<ItemStack>)| {
                                encode_item_slot(index, stack.as_ref())
                            })
                            .collect(),
                    ),
                );
                Some(NbtTag::Compound(compound))
            }
            _ => None,
        },
    }
}

fn container_property(container: &ContainerBlockEntityState, key: &str) -> i16 {
    container
        .properties
        .get(&ContainerPropertyKey::new(key))
        .copied()
        .unwrap_or_default()
}

fn encode_item_slot(index: usize, stack: Option<&ItemStack>) -> Option<NbtTag> {
    let stack = stack?;
    let (item_id, damage) = legacy_item(stack)?;
    let mut compound = BTreeMap::new();
    compound.insert(
        "Slot".to_string(),
        NbtTag::Byte(i8::try_from(index).expect("chest slot index should fit into i8")),
    );
    compound.insert("id".to_string(), NbtTag::Short(item_id));
    compound.insert(
        "Count".to_string(),
        NbtTag::Byte(i8::try_from(stack.count).expect("stack count should fit into i8")),
    );
    compound.insert(
        "Damage".to_string(),
        NbtTag::Short(i16::try_from(damage).expect("item damage should fit into i16")),
    );
    Some(NbtTag::Compound(compound))
}

fn decode_tile_entity(
    tile_entity: &NbtTag,
) -> Result<Option<(BlockPos, BlockEntityState)>, StorageError> {
    let compound = as_compound(tile_entity)?;
    let Ok(kind) = string_field(compound, "id") else {
        return Ok(None);
    };
    let position = BlockPos::new(
        int_field(compound, "x")?,
        int_field(compound, "y")?,
        int_field(compound, "z")?,
    );
    let mut slots = if kind == "Chest" {
        vec![None; 27]
    } else if kind == "Furnace" {
        vec![None; 3]
    } else {
        return Ok(None);
    };
    if let Ok(items) = list_field(compound, "Items") {
        for item in items {
            let item_compound = as_compound(item)?;
            let slot = usize::try_from(byte_field(item_compound, "Slot")?).map_err(|_| {
                StorageError::InvalidData("negative tile-entity slot index".to_string())
            })?;
            if slot >= slots.len() {
                continue;
            }
            let item_id = short_field(item_compound, "id")?;
            let damage = u16::try_from(short_field(item_compound, "Damage")?).map_err(|_| {
                StorageError::InvalidData("negative item damage not supported".to_string())
            })?;
            let count = u8::try_from(byte_field(item_compound, "Count")?).map_err(|_| {
                StorageError::InvalidData("negative item count not supported".to_string())
            })?;
            slots[slot] = Some(semantic_item(item_id, damage, count));
        }
    }
    let block_entity = if kind == "Chest" {
        BlockEntityState::Container(ContainerBlockEntityState {
            kind: BlockEntityKindId::new(CHEST_BLOCK_ENTITY_KIND),
            slots,
            properties: BTreeMap::new(),
        })
    } else {
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
                    short_field(compound, "BurnTimeMax").unwrap_or(0),
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
        })
    };
    Ok(Some((position, block_entity)))
}
