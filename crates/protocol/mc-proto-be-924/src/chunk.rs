use bedrockrs_proto::V924;
use bedrockrs_proto::v662::packets::LevelChunkPacket;
use bedrockrs_proto::v662::types::ChunkPos as BedrockChunkPos;
use mc_core::{BlockState, ChunkColumn, ChunkSection};
use mc_proto_common::ProtocolError;
use nbtx::Value;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

const SUBCHUNK_VERSION_LIMITLESS: u8 = 9;
const SUBCHUNK_LAYER_COUNT: u8 = 1;
const BEDROCK_BIOME_SECTION_COUNT: usize = 24;
const VALID_BITS: [u8; 8] = [1, 2, 3, 4, 5, 6, 8, 16];
const DEFAULT_BLOCK_VERSION: i32 = 0;

#[derive(Serialize)]
#[serde(rename = "")]
struct BedrockBlockPaletteEntry {
    name: String,
    #[serde(default)]
    states: BTreeMap<String, Value>,
    version: i32,
}

pub(crate) fn level_chunk_packet(chunk: &ChunkColumn) -> Result<V924, ProtocolError> {
    let serialized_chunk_data = encode_chunk_payload(chunk)?;
    Ok(V924::LevelChunkPacket(LevelChunkPacket {
        chunk_position: BedrockChunkPos {
            x: chunk.pos.x,
            z: chunk.pos.z,
        },
        dimension_id: 0,
        sub_chunk_count: u32::try_from(encoded_subchunk_count(chunk))
            .expect("subchunk count should fit into u32"),
        sub_chunk_limit: 0,
        cache_enabled: false,
        cache_blobs: Vec::new(),
        serialized_chunk_data,
    }))
}

fn encode_chunk_payload(chunk: &ChunkColumn) -> Result<Vec<u8>, ProtocolError> {
    let subchunk_count = encoded_subchunk_count(chunk);
    let mut bytes = Vec::new();
    for section_y in 0..subchunk_count {
        write_subchunk(
            &mut bytes,
            i8::try_from(section_y).expect("section index should fit into i8"),
            chunk
                .sections
                .get(&i32::try_from(section_y).expect("section index should fit into i32")),
        )?;
    }
    write_biomes(&mut bytes, chunk);
    // Border blocks are only used in Education Edition.
    bytes.push(0);
    Ok(bytes)
}

fn encoded_subchunk_count(chunk: &ChunkColumn) -> usize {
    chunk
        .sections
        .keys()
        .next_back()
        .and_then(|highest| usize::try_from(*highest + 1).ok())
        .unwrap_or(0)
}

fn write_subchunk(
    bytes: &mut Vec<u8>,
    section_index: i8,
    section: Option<&ChunkSection>,
) -> Result<(), ProtocolError> {
    bytes.push(SUBCHUNK_VERSION_LIMITLESS);
    bytes.push(SUBCHUNK_LAYER_COUNT);
    bytes.push(section_index.to_le_bytes()[0]);

    let (palette, indices) = encode_palette_indices(section);
    let bits = palette_bits(palette.len());
    bytes.push(bits << 1);
    write_packed_indices(bytes, &indices, bits);
    bytes.extend_from_slice(
        &u32::try_from(palette.len())
            .expect("palette length should fit into u32")
            .to_le_bytes(),
    );
    for entry in palette {
        nbtx::to_le_bytes_in(bytes, &entry)
            .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
    }
    Ok(())
}

fn encode_palette_indices(
    section: Option<&ChunkSection>,
) -> (Vec<BedrockBlockPaletteEntry>, Vec<u16>) {
    let air = bedrock_palette_entry(&BlockState::air());
    let mut palette = vec![air];
    let mut lookup = HashMap::from([(block_palette_key(&BlockState::air()), 0_u16)]);
    let mut indices = vec![0_u16; 16 * 16 * 16];

    if let Some(section) = section {
        for x in 0..16_u8 {
            for z in 0..16_u8 {
                for y in 0..16_u8 {
                    let block = section
                        .get_block(x, y, z)
                        .cloned()
                        .unwrap_or_else(BlockState::air);
                    let key = block_palette_key(&block);
                    let palette_index = if let Some(existing) = lookup.get(&key) {
                        *existing
                    } else {
                        let index = u16::try_from(palette.len())
                            .expect("bedrock palette length should fit into u16");
                        palette.push(bedrock_palette_entry(&block));
                        lookup.insert(key, index);
                        index
                    };
                    indices[subchunk_offset(x, y, z)] = palette_index;
                }
            }
        }
    }

    (palette, indices)
}

fn palette_bits(palette_len: usize) -> u8 {
    VALID_BITS
        .into_iter()
        .find(|bits| 2usize.pow(u32::from(*bits)) >= palette_len.max(1))
        .unwrap_or(16)
}

fn write_packed_indices(bytes: &mut Vec<u8>, indices: &[u16], bits: u8) {
    let per_word = 32usize / usize::from(bits);
    let total_words = indices.len().div_ceil(per_word);
    let mut offset = 0usize;
    for _ in 0..total_words {
        let mut word = 0_u32;
        for shift_index in 0..per_word {
            let Some(index) = indices.get(offset) else {
                break;
            };
            word |= u32::from(*index) << (shift_index * usize::from(bits));
            offset += 1;
        }
        bytes.extend_from_slice(&word.to_le_bytes());
    }
}

fn write_biomes(bytes: &mut Vec<u8>, chunk: &ChunkColumn) {
    let heightmap = chunk_heightmap(chunk);
    for entry in heightmap {
        bytes.extend_from_slice(&entry.to_le_bytes());
    }
    let biome_id = u32::from(chunk.biomes.first().copied().unwrap_or(1));
    for _ in 0..BEDROCK_BIOME_SECTION_COUNT {
        bytes.push(0);
        bytes.extend_from_slice(&biome_id.to_le_bytes());
    }
}

fn chunk_heightmap(chunk: &ChunkColumn) -> [u16; 256] {
    let mut heightmap = [0_u16; 256];
    for x in 0..16_u8 {
        for z in 0..16_u8 {
            let mut highest = 0_i32;
            for section_y in chunk.sections.keys().rev() {
                if let Some(section) = chunk.sections.get(section_y) {
                    for local_y in (0..16_u8).rev() {
                        if section.get_block(x, local_y, z).is_some() {
                            highest = section_y * 16 + i32::from(local_y) + 1;
                            break;
                        }
                    }
                }
                if highest > 0 {
                    break;
                }
            }
            heightmap[usize::from(x) * 16 + usize::from(z)] =
                u16::try_from(highest.max(0)).unwrap_or_default();
        }
    }
    heightmap
}

fn subchunk_offset(x: u8, y: u8, z: u8) -> usize {
    16 * 16 * usize::from(x) + 16 * usize::from(z) + usize::from(y)
}

fn block_palette_key(block: &BlockState) -> String {
    let mut key = block.key.as_str().to_string();
    if !block.properties.is_empty() {
        key.push('[');
        let properties = block
            .properties
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(",");
        key.push_str(&properties);
        key.push(']');
    }
    key
}

fn bedrock_palette_entry(block: &BlockState) -> BedrockBlockPaletteEntry {
    let states = block
        .properties
        .iter()
        .map(|(name, value)| (name.clone(), Value::String(value.clone())))
        .collect::<BTreeMap<_, _>>();
    BedrockBlockPaletteEntry {
        name: block.key.as_str().to_string(),
        states,
        version: DEFAULT_BLOCK_VERSION,
    }
}
