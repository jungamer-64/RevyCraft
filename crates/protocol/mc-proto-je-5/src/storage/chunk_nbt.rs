use super::nbt::{
    NbtTag, as_compound, byte_array_field, byte_field, compound_field, int_field, list_field,
};
use mc_core::{ChunkColumn, ChunkPos, expand_block_index};
use mc_proto_common::StorageError;
use mc_proto_je_common::__version_support::{
    blocks::{legacy_block, semantic_block},
    chunks::get_nibble,
};
use std::collections::BTreeMap;

pub(super) fn chunk_to_nbt(chunk: &ChunkColumn) -> NbtTag {
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

pub(super) fn chunk_from_nbt(root: &NbtTag) -> Result<ChunkColumn, StorageError> {
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
