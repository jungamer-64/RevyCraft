use mc_core::catalog::{
    BEDROCK, BRICKS, COBBLESTONE, DIRT, GLASS, GRASS_BLOCK, OAK_PLANKS, SAND, SANDSTONE, STONE,
};
use mc_core::{BlockPos, BlockState, ChunkColumn, InventorySlot, ItemStack, PlayerInventory};
use mc_proto_common::{PacketReader, PacketWriter, ProtocolError};
use num_traits::ToPrimitive;
use std::collections::BTreeMap;

pub fn legacy_block(state: &BlockState) -> (u16, u8) {
    match state.key.as_str() {
        STONE => (1, 0),
        GRASS_BLOCK => (2, 0),
        DIRT => (3, 0),
        COBBLESTONE => (4, 0),
        OAK_PLANKS => (5, 0),
        BEDROCK => (7, 0),
        SAND => (12, 0),
        GLASS => (20, 0),
        SANDSTONE => (24, 0),
        BRICKS => (45, 0),
        _ => (0, 0),
    }
}

#[must_use]
pub fn legacy_block_state_id(state: &BlockState) -> i32 {
    let (block_id, metadata) = legacy_block(state);
    (i32::from(block_id) << 4) | i32::from(metadata)
}

#[must_use]
pub fn semantic_block(block_id: u16, metadata: u8) -> BlockState {
    match block_id {
        1 => BlockState::stone(),
        2 => BlockState::grass_block(),
        3 => BlockState::dirt(),
        4 => BlockState::cobblestone(),
        5 if metadata == 0 => BlockState::oak_planks(),
        7 => BlockState::bedrock(),
        12 if metadata == 0 => BlockState::sand(),
        20 => BlockState::glass(),
        24 if metadata == 0 => BlockState::sandstone(),
        45 => BlockState::bricks(),
        _ => BlockState::air(),
    }
}

#[must_use]
pub fn legacy_item(stack: &ItemStack) -> Option<(i16, u16)> {
    let damage = stack.damage;
    match stack.key.as_str() {
        STONE => Some((1, damage)),
        GRASS_BLOCK => Some((2, damage)),
        DIRT => Some((3, damage)),
        COBBLESTONE => Some((4, damage)),
        OAK_PLANKS => Some((5, damage)),
        SAND => Some((12, damage)),
        GLASS => Some((20, damage)),
        SANDSTONE => Some((24, damage)),
        BRICKS => Some((45, damage)),
        _ => None,
    }
}

#[must_use]
pub fn semantic_item(item_id: i16, damage: u16, count: u8) -> ItemStack {
    let key = match item_id {
        1 => STONE,
        2 => GRASS_BLOCK,
        3 => DIRT,
        4 => COBBLESTONE,
        5 if damage == 0 => OAK_PLANKS,
        12 if damage == 0 => SAND,
        20 => GLASS,
        24 if damage == 0 => SANDSTONE,
        45 => BRICKS,
        _ => return ItemStack::unsupported(count, damage),
    };
    ItemStack::new(key, count, damage)
}

pub fn read_legacy_slot(reader: &mut PacketReader<'_>) -> Result<Option<ItemStack>, ProtocolError> {
    let item_id = reader.read_i16()?;
    if item_id < 0 {
        return Ok(None);
    }
    let count = reader.read_u8()?;
    let damage = u16::from_be_bytes(reader.read_i16()?.to_be_bytes());
    skip_slot_nbt(reader)?;
    Ok(Some(semantic_item(item_id, damage, count)))
}

pub fn write_legacy_slot(
    writer: &mut PacketWriter,
    stack: Option<&ItemStack>,
) -> Result<(), ProtocolError> {
    let Some(stack) = stack else {
        writer.write_i16(-1);
        return Ok(());
    };
    let Some((item_id, damage)) = legacy_item(stack) else {
        return Err(ProtocolError::InvalidPacket("unsupported inventory item"));
    };
    writer.write_i16(item_id);
    writer.write_u8(stack.count);
    writer.write_i16(i16::from_be_bytes(damage.to_be_bytes()));
    writer.write_i16(-1);
    Ok(())
}

pub fn skip_slot_nbt(reader: &mut PacketReader<'_>) -> Result<(), ProtocolError> {
    let length = reader.read_i16()?;
    if length < 0 {
        return Ok(());
    }
    let length = usize::try_from(length)
        .map_err(|_| ProtocolError::InvalidPacket("negative slot nbt length"))?;
    let _ = reader.read_bytes(length)?;
    Ok(())
}

#[must_use]
pub fn legacy_window_slot(slot: InventorySlot) -> Option<i16> {
    slot.legacy_window_index().map(i16::from)
}

#[must_use]
pub const fn modern_window_slot(slot: InventorySlot) -> Option<i16> {
    match slot {
        InventorySlot::Offhand => Some(45),
        _ => match slot.legacy_window_index() {
            Some(index) => Some(index as i16),
            None => None,
        },
    }
}

#[must_use]
pub fn legacy_inventory_slot(raw_slot: i16) -> Option<InventorySlot> {
    u8::try_from(raw_slot)
        .ok()
        .and_then(InventorySlot::from_legacy_window_index)
}

#[must_use]
pub fn modern_inventory_slot(raw_slot: i16) -> Option<InventorySlot> {
    if raw_slot == 45 {
        Some(InventorySlot::Offhand)
    } else {
        legacy_inventory_slot(raw_slot)
    }
}

#[must_use]
pub fn legacy_window_items(inventory: &PlayerInventory) -> Vec<Option<ItemStack>> {
    inventory.slots.clone()
}

#[must_use]
pub fn modern_window_items(inventory: &PlayerInventory) -> Vec<Option<ItemStack>> {
    let mut items = inventory.slots.clone();
    items.push(inventory.offhand.clone());
    items
}

pub fn write_empty_metadata_1_8(writer: &mut PacketWriter) {
    writer.write_u8(0x7f);
}

pub fn write_empty_metadata_1_12(writer: &mut PacketWriter) {
    writer.write_u8(0xff);
}

#[must_use]
pub fn pack_block_position(position: BlockPos) -> i64 {
    let x = i64::from(position.x) & 0x3ff_ffff;
    let y = i64::from(position.y) & 0xfff;
    let z = i64::from(position.z) & 0x3ff_ffff;
    (x << 38) | (y << 26) | z
}

#[must_use]
pub fn unpack_block_position(packed: i64) -> BlockPos {
    let x = sign_extend((packed >> 38) & 0x3ff_ffff, 26);
    let y = sign_extend((packed >> 26) & 0xfff, 12);
    let z = sign_extend(packed & 0x3ff_ffff, 26);
    BlockPos::new(
        i32::try_from(x).expect("packed x should fit into i32"),
        i32::try_from(y).expect("packed y should fit into i32"),
        i32::try_from(z).expect("packed z should fit into i32"),
    )
}

#[must_use]
pub fn to_fixed_point(value: f64) -> i32 {
    rounded_f64_to_i32(value * 32.0)
}

#[must_use]
pub fn to_angle_byte(value: f32) -> i8 {
    let wrapped = value.rem_euclid(360.0);
    let scaled = rounded_f32_to_i32(wrapped * 256.0 / 360.0);
    let narrowed =
        u8::try_from(scaled.rem_euclid(256)).expect("wrapped angle should fit into byte");
    i8::from_be_bytes([narrowed])
}

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
        for (index, state_id) in block_states.into_iter().enumerate() {
            let palette_index = match palette_lookup.get(&state_id) {
                Some(palette_index) => *palette_index,
                None => {
                    let next_index =
                        u64::try_from(palette.len()).expect("palette length should fit into u64");
                    palette.push(state_id);
                    palette_lookup.insert(state_id, next_index);
                    next_index
                }
            };
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

fn bits_per_block(palette_len: u8) -> u8 {
    let required = (f32::from(palette_len.max(1)))
        .log2()
        .ceil()
        .to_u8()
        .unwrap_or(0);
    required.max(4)
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

fn sign_extend(value: i64, bits: u8) -> i64 {
    let shift = 64_u8.saturating_sub(bits);
    (value << shift) >> shift
}

fn rounded_f64_to_i32(value: f64) -> i32 {
    value
        .round()
        .to_i32()
        .expect("fixed-point value should fit into i32")
}

fn rounded_f32_to_i32(value: f32) -> i32 {
    value
        .round()
        .to_i32()
        .expect("angle byte intermediate should fit into i32")
}
