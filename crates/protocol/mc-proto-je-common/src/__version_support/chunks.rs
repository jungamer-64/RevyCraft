use crate::__version_support::blocks::{
    flattened_block_state_id_1_13_2, legacy_block, legacy_block_state_id,
};
use flate2::Compression;
use flate2::write::ZlibEncoder;
use mc_proto_common::ProtocolError;
use num_traits::ToPrimitive;
use revy_voxel_model::ChunkColumn;
use std::collections::BTreeMap;
use std::io::Write;

#[must_use]
pub fn build_chunk_data_1_8(chunk: &ChunkColumn, include_biomes: bool) -> (u16, Vec<u8>) {
    let mut bit_map = 0_u16;
    let mut bytes = Vec::new();
    for (section_y, section) in &chunk.sections {
        if !(0..16).contains(section_y) || section.is_empty() {
            continue;
        }
        bit_map |= 1_u16 << u16::try_from(*section_y).expect("section index should fit into u16");

        let mut states = vec![0_u16; 4096];
        for (index, state) in section.iter_blocks() {
            states[usize::from(index)] =
                u16::try_from(legacy_block_state_id(state)).expect("block state id should fit");
        }
        for state in states {
            bytes.extend_from_slice(&state.to_le_bytes());
        }
        bytes.extend_from_slice(&[0_u8; 2048]);
        bytes.extend_from_slice(&[0xff_u8; 2048]);
    }
    if include_biomes {
        bytes.extend_from_slice(&chunk.biomes);
    }
    (bit_map, bytes)
}

#[must_use]
pub fn build_chunk_data_1_12(chunk: &ChunkColumn, include_biomes: bool) -> (u16, Vec<u8>) {
    let mut bit_map = 0_u16;
    let mut bytes = Vec::new();
    for (section_y, section) in &chunk.sections {
        if !(0..16).contains(section_y) || section.is_empty() {
            continue;
        }
        bit_map |= 1_u16 << u16::try_from(*section_y).expect("section index should fit into u16");

        let mut block_states = vec![0_i32; 4096];
        for (index, state) in section.iter_blocks() {
            block_states[usize::from(index)] = legacy_block_state_id(state);
        }

        let mut palette = vec![0_i32];
        let mut palette_lookup = BTreeMap::from([(0_i32, 0_u64)]);
        let mut packed_indices = vec![0_u64; 4096];
        for (index, state_id) in block_states.iter().copied().enumerate() {
            let palette_index = palette_lookup.get(&state_id).copied().unwrap_or_else(|| {
                let next_index =
                    u64::try_from(palette.len()).expect("palette length should fit into u64");
                palette.push(state_id);
                palette_lookup.insert(state_id, next_index);
                next_index
            });
            packed_indices[index] = palette_index;
        }

        let bits_per_block =
            bits_per_block(u8::try_from(palette.len()).expect("palette length should fit into u8"));
        let data_array_len = (4096 * usize::from(bits_per_block)).div_ceil(64);
        let mut data_array = vec![0_u64; data_array_len];
        for (index, palette_index) in packed_indices.into_iter().enumerate() {
            let start_bit = index * usize::from(bits_per_block);
            let long_index = start_bit / 64;
            let bit_offset = start_bit % 64;
            data_array[long_index] |= palette_index << bit_offset;
            if bit_offset + usize::from(bits_per_block) > 64 {
                let spill = bit_offset + usize::from(bits_per_block) - 64;
                data_array[long_index + 1] |=
                    palette_index >> (usize::from(bits_per_block) - spill);
            }
        }

        bytes.push(bits_per_block);
        write_varint_to_vec(
            &mut bytes,
            i32::try_from(palette.len()).expect("palette length should fit into i32"),
        );
        for state_id in palette {
            write_varint_to_vec(&mut bytes, state_id);
        }
        write_varint_to_vec(
            &mut bytes,
            i32::try_from(data_array.len()).expect("data array length should fit into i32"),
        );
        for value in data_array {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        bytes.extend_from_slice(&[0_u8; 2048]);
        bytes.extend_from_slice(&[0xff_u8; 2048]);
    }

    if include_biomes {
        bytes.extend_from_slice(&chunk.biomes);
    }

    (bit_map, bytes)
}

#[must_use]
pub fn build_chunk_data_1_13_2(chunk: &ChunkColumn, include_biomes: bool) -> (u16, Vec<u8>) {
    const DIRECT_PALETTE_BITS_PER_BLOCK_1_13: u8 = 14;

    let mut bit_map = 0_u16;
    let mut bytes = Vec::new();
    for (section_y, section) in &chunk.sections {
        if !(0..16).contains(section_y) || section.is_empty() {
            continue;
        }
        bit_map |= 1_u16 << u16::try_from(*section_y).expect("section index should fit into u16");

        let mut block_states = vec![0_i32; 4096];
        for (index, state) in section.iter_blocks() {
            block_states[usize::from(index)] = flattened_block_state_id_1_13_2(state);
        }

        let mut palette = vec![0_i32];
        let mut palette_lookup = BTreeMap::from([(0_i32, 0_u64)]);
        let mut packed_states = vec![0_u64; 4096];
        for (index, state_id) in block_states.iter().copied().enumerate() {
            let palette_index = palette_lookup.get(&state_id).copied().unwrap_or_else(|| {
                let next_index =
                    u64::try_from(palette.len()).expect("palette length should fit into u64");
                palette.push(state_id);
                palette_lookup.insert(state_id, next_index);
                next_index
            });
            packed_states[index] = palette_index;
        }

        let local_bits_per_block =
            bits_per_block(u8::try_from(palette.len()).expect("palette length should fit into u8"));
        let use_direct_palette = local_bits_per_block > 8;
        let bits_per_block = if use_direct_palette {
            DIRECT_PALETTE_BITS_PER_BLOCK_1_13
        } else {
            local_bits_per_block
        };
        let data_array = if use_direct_palette {
            let global_states = block_states
                .iter()
                .copied()
                .map(|state_id| {
                    u64::try_from(state_id).expect("block state id should fit into u64")
                })
                .collect::<Vec<_>>();
            pack_compacted_u64s(&global_states, bits_per_block)
        } else {
            pack_compacted_u64s(&packed_states, bits_per_block)
        };

        bytes.push(bits_per_block);
        if !use_direct_palette {
            write_varint_to_vec(
                &mut bytes,
                i32::try_from(palette.len()).expect("palette length should fit into i32"),
            );
            for state_id in palette {
                write_varint_to_vec(&mut bytes, state_id);
            }
        }
        write_varint_to_vec(
            &mut bytes,
            i32::try_from(data_array.len()).expect("data array length should fit into i32"),
        );
        for value in data_array {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        bytes.extend_from_slice(&[0_u8; 2048]);
        bytes.extend_from_slice(&[0xff_u8; 2048]);
    }

    if include_biomes {
        bytes.extend_from_slice(&chunk.biomes);
    }

    (bit_map, bytes)
}

#[must_use]
pub fn build_chunk_data_1_7(chunk: &ChunkColumn, include_biomes: bool) -> (u16, Vec<u8>) {
    let mut bit_map = 0_u16;
    let mut sections = Vec::new();
    for (section_y, section) in &chunk.sections {
        if !(0..16).contains(section_y) || section.is_empty() {
            continue;
        }
        bit_map |= 1_u16 << u16::try_from(*section_y).expect("section index should fit into u16");
        let mut blocks = vec![0_u8; 4096];
        let mut metadata = vec![0_u8; 2048];
        let block_light = vec![0_u8; 2048];
        let sky_light = vec![0xff_u8; 2048];
        for (index, state) in section.iter_blocks() {
            let (block_id, block_meta) = legacy_block(state);
            let index_usize = usize::from(index);
            blocks[index_usize] =
                u8::try_from(block_id).expect("legacy block id should fit into byte");
            set_nibble(&mut metadata, index_usize, block_meta);
        }
        sections.extend_from_slice(&blocks);
        sections.extend_from_slice(&metadata);
        sections.extend_from_slice(&block_light);
        sections.extend_from_slice(&sky_light);
    }
    if include_biomes {
        sections.extend_from_slice(&chunk.biomes);
    }
    (bit_map, sections)
}

#[must_use]
pub fn get_nibble(source: &[u8], index: usize) -> u8 {
    let byte = source[index / 2];
    if index.is_multiple_of(2) {
        byte & 0x0f
    } else {
        (byte >> 4) & 0x0f
    }
}

pub fn zlib_compress(data: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .map_err(|_| ProtocolError::InvalidPacket("failed to compress payload"))?;
    encoder
        .finish()
        .map_err(|_| ProtocolError::InvalidPacket("failed to finalize compressed payload"))
}

fn set_nibble(target: &mut [u8], index: usize, value: u8) {
    let byte_index = index / 2;
    if index.is_multiple_of(2) {
        target[byte_index] = (target[byte_index] & 0xf0) | (value & 0x0f);
    } else {
        target[byte_index] = (target[byte_index] & 0x0f) | ((value & 0x0f) << 4);
    }
}

fn bits_per_block(palette_len: u8) -> u8 {
    let required = (f32::from(palette_len.max(1)))
        .log2()
        .ceil()
        .to_u8()
        .unwrap_or(0);
    required.max(4)
}

fn pack_compacted_u64s(values: &[u64], bits_per_block: u8) -> Vec<u64> {
    let data_array_len = (values.len() * usize::from(bits_per_block)).div_ceil(64);
    let mut data_array = vec![0_u64; data_array_len];
    for (index, value) in values.iter().copied().enumerate() {
        let start_bit = index * usize::from(bits_per_block);
        let long_index = start_bit / 64;
        let bit_offset = start_bit % 64;
        data_array[long_index] |= value << bit_offset;
        if bit_offset + usize::from(bits_per_block) > 64 {
            let spill = bit_offset + usize::from(bits_per_block) - 64;
            data_array[long_index + 1] |= value >> (usize::from(bits_per_block) - spill);
        }
    }
    data_array
}

fn write_varint_to_vec(target: &mut Vec<u8>, mut value: i32) {
    loop {
        let mut byte = u8::try_from(value & 0x7f).expect("varint chunk should fit into u8");
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        target.push(byte);
        if value == 0 {
            break;
        }
    }
}
