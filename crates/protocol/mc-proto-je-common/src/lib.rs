#![allow(clippy::multiple_crate_versions)]
use flate2::Compression;
use flate2::write::ZlibEncoder;
use mc_core::catalog::{
    BEDROCK, BRICKS, COBBLESTONE, DIRT, GLASS, GRASS_BLOCK, OAK_PLANKS, SAND, SANDSTONE, STONE,
};
use mc_core::{
    BlockPos, BlockState, ChunkColumn, CoreCommand, CoreEvent, EntityId, InventoryContainer,
    InventorySlot, ItemStack, PlayerId, PlayerInventory, PlayerSnapshot, WorldMeta,
};
use mc_proto_common::{
    ConnectionPhase, Edition, HandshakeIntent, HandshakeNextState, HandshakeProbe, LoginRequest,
    MinecraftWireCodec, PacketReader, PacketWriter, PlayEncodingContext, PlaySyncAdapter,
    ProtocolAdapter, ProtocolDescriptor, ProtocolError, ServerListStatus, SessionAdapter,
    StatusRequest, TransportKind, WireCodec,
};
use num_traits::ToPrimitive;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Write;

const PACKET_HANDSHAKE: i32 = 0x00;

/// Decodes the shared Java Edition handshake packet used by the supported legacy versions.
///
/// # Errors
///
/// Returns an error when the payload identifies itself as a handshake packet but contains an
/// unsupported next-state value or is otherwise truncated.
pub fn decode_handshake_frame(frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
    let mut reader = PacketReader::new(frame);
    let packet_id = reader.read_varint()?;
    if packet_id != PACKET_HANDSHAKE {
        return Ok(None);
    }
    let protocol_number = reader.read_varint()?;
    let server_host = reader.read_string(255)?;
    let server_port = reader.read_u16()?;
    let next_state = match reader.read_varint()? {
        1 => HandshakeNextState::Status,
        2 => HandshakeNextState::Login,
        _ => {
            return Err(ProtocolError::InvalidPacket(
                "unsupported handshake next state",
            ));
        }
    };
    Ok(Some(HandshakeIntent {
        edition: Edition::Je,
        protocol_number,
        server_host,
        server_port,
        next_state,
    }))
}

#[must_use]
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

/// Reads a legacy item slot from a Java Edition packet stream.
///
/// # Errors
///
/// Returns an error when the slot payload is truncated or contains invalid NBT framing.
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

/// Writes a legacy item slot to a Java Edition packet stream.
///
/// # Errors
///
/// Returns an error when the provided item stack cannot be represented in the legacy item table.
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

/// Skips the optional legacy slot NBT payload.
///
/// # Errors
///
/// Returns an error when the NBT length prefix is invalid or the payload is truncated.
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
/// Unpacks the packed 64-bit block position used by legacy Java Edition protocols.
///
/// # Panics
///
/// Panics if the unpacked coordinates fall outside the `i32` range expected by `BlockPos`.
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
/// Converts a degree angle into the signed byte representation used on the wire.
///
/// # Panics
///
/// Panics if the rounded angle value cannot be narrowed into a single byte.
pub fn to_angle_byte(value: f32) -> i8 {
    let wrapped = value.rem_euclid(360.0);
    let scaled = rounded_f32_to_i32(wrapped * 256.0 / 360.0);
    let narrowed =
        u8::try_from(scaled.rem_euclid(256)).expect("wrapped angle should fit into byte");
    i8::from_be_bytes([narrowed])
}

#[must_use]
/// Builds legacy 1.8 chunk section payload bytes and the corresponding section bitmask.
///
/// # Panics
///
/// Panics if a retained section index or legacy block state ID falls outside the encoded range.
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
/// Builds legacy 1.12 chunk section payload bytes and the corresponding section bitmask.
///
/// # Panics
///
/// Panics if section indices, palette sizes, or packed array lengths exceed the legacy codec
/// limits assumed by this helper.
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
/// Builds legacy 1.7 chunk section payload bytes and the corresponding section bitmask.
///
/// # Panics
///
/// Panics if a retained section index or legacy block ID falls outside the encoded range.
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

fn set_nibble(target: &mut [u8], index: usize, value: u8) {
    let byte_index = index / 2;
    if index.is_multiple_of(2) {
        target[byte_index] = (target[byte_index] & 0xf0) | (value & 0x0f);
    } else {
        target[byte_index] = (target[byte_index] & 0x0f) | ((value & 0x0f) << 4);
    }
}

/// Compresses a legacy chunk payload with zlib.
///
/// # Errors
///
/// Returns an error when compression fails.
pub fn zlib_compress(data: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .map_err(|_| ProtocolError::InvalidPacket("failed to compress payload"))?;
    encoder
        .finish()
        .map_err(|_| ProtocolError::InvalidPacket("failed to finalize compressed payload"))
}

/// Writes the legacy login byte-array structure shared by the supported JE adapters.
///
/// # Errors
///
/// Returns an error when the byte array length does not fit into a VarInt.
pub fn write_login_byte_array(
    writer: &mut PacketWriter,
    bytes: &[u8],
) -> Result<(), ProtocolError> {
    writer.write_varint(
        i32::try_from(bytes.len())
            .map_err(|_| ProtocolError::InvalidPacket("login byte array too large"))?,
    );
    writer.write_bytes(bytes);
    Ok(())
}

/// Reads the legacy login byte-array structure shared by the supported JE adapters.
///
/// # Errors
///
/// Returns an error when the length prefix is invalid or the payload is truncated.
pub fn read_login_byte_array(reader: &mut PacketReader<'_>) -> Result<Vec<u8>, ProtocolError> {
    let len = usize::try_from(reader.read_varint()?)
        .map_err(|_| ProtocolError::InvalidPacket("negative login byte array length"))?;
    Ok(reader.read_bytes(len)?.to_vec())
}

/// Encodes the shared JSON-based JE status response packet.
///
/// # Errors
///
/// Returns an error when the generated JSON string cannot be encoded as a protocol string.
pub fn encode_status_response_packet(status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
    let payload = json!({
        "version": {
            "name": status.version.version_name,
            "protocol": status.version.protocol_number,
        },
        "players": {
            "max": status.max_players,
            "online": status.players_online,
            "sample": [],
        },
        "description": {
            "text": status.description,
        }
    });
    let mut writer = PacketWriter::default();
    writer.write_varint(0x00);
    writer.write_string(&payload.to_string())?;
    Ok(writer.into_inner())
}

/// Encodes the shared JE status pong packet.
pub fn encode_status_pong_packet(payload: i64) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x01);
    writer.write_i64(payload);
    writer.into_inner()
}

/// Encodes the shared JE login success packet used by the supported adapters.
///
/// # Errors
///
/// Returns an error when the UUID or username cannot be encoded as protocol strings.
pub fn encode_login_success_packet(player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x02);
    writer.write_string(&player.id.0.hyphenated().to_string())?;
    writer.write_string(&player.username)?;
    Ok(writer.into_inner())
}

#[must_use]
pub const fn player_window_id(container: InventoryContainer) -> u8 {
    match container {
        InventoryContainer::Player => 0,
    }
}

pub trait JavaEditionProfile: Default + Send + Sync {
    fn adapter_id(&self) -> &'static str;
    fn descriptor(&self) -> ProtocolDescriptor;
    fn play_disconnect_packet_id(&self) -> i32;
    fn format_disconnect_reason(&self, reason: &str) -> String;
    fn encode_play_bootstrap(
        &self,
        entity_id: EntityId,
        world_meta: &WorldMeta,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_chunk_batch(&self, chunks: &[ChunkColumn]) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_entity_spawn(
        &self,
        entity_id: EntityId,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_entity_moved(
        &self,
        entity_id: EntityId,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_entity_despawn(&self, entity_ids: &[EntityId]) -> Result<Vec<u8>, ProtocolError>;
    fn encode_inventory_contents(
        &self,
        container: InventoryContainer,
        inventory: &PlayerInventory,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn encode_inventory_slot_changed(
        &self,
        container: InventoryContainer,
        slot: InventorySlot,
        stack: Option<&ItemStack>,
    ) -> Result<Option<Vec<u8>>, ProtocolError>;
    fn encode_selected_hotbar_slot_changed(&self, slot: u8) -> Result<Vec<u8>, ProtocolError>;
    fn encode_block_changed(
        &self,
        position: BlockPos,
        block: &BlockState,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn encode_keep_alive_requested(&self, keep_alive_id: i32) -> Result<Vec<u8>, ProtocolError>;
    fn decode_play(
        &self,
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError>;
}

#[derive(Default)]
pub struct JavaEditionAdapter<P> {
    codec: MinecraftWireCodec,
    profile: P,
}

impl<P: Default> JavaEditionAdapter<P> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl<P: JavaEditionProfile> HandshakeProbe for JavaEditionAdapter<P> {
    fn transport_kind(&self) -> TransportKind {
        TransportKind::Tcp
    }

    fn adapter_id(&self) -> Option<&'static str> {
        Some(self.profile.adapter_id())
    }

    fn try_route(&self, frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
        decode_handshake_frame(frame)
    }
}

impl<P: JavaEditionProfile> SessionAdapter for JavaEditionAdapter<P> {
    fn wire_codec(&self) -> &dyn WireCodec {
        &self.codec
    }

    fn decode_status(&self, frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        let mut reader = PacketReader::new(frame);
        match reader.read_varint()? {
            0x00 => Ok(StatusRequest::Query),
            0x01 => Ok(StatusRequest::Ping {
                payload: reader.read_i64()?,
            }),
            packet_id => Err(ProtocolError::UnsupportedPacket(packet_id)),
        }
    }

    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        let mut reader = PacketReader::new(frame);
        match reader.read_varint()? {
            0x00 => Ok(LoginRequest::LoginStart {
                username: reader.read_string(16)?,
            }),
            0x01 => Ok(LoginRequest::EncryptionResponse {
                shared_secret_encrypted: read_login_byte_array(&mut reader)?,
                verify_token_encrypted: read_login_byte_array(&mut reader)?,
            }),
            packet_id => Err(ProtocolError::UnsupportedPacket(packet_id)),
        }
    }

    fn encode_status_response(&self, status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        encode_status_response_packet(status)
    }

    fn encode_status_pong(&self, payload: i64) -> Result<Vec<u8>, ProtocolError> {
        Ok(encode_status_pong_packet(payload))
    }

    fn encode_disconnect(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        let packet_id = match phase {
            ConnectionPhase::Login => 0x00,
            ConnectionPhase::Play => self.profile.play_disconnect_packet_id(),
            _ => {
                return Err(ProtocolError::InvalidPacket(
                    "disconnect only valid in login/play",
                ));
            }
        };
        let mut writer = PacketWriter::default();
        writer.write_varint(packet_id);
        writer.write_string(&self.profile.format_disconnect_reason(reason))?;
        Ok(writer.into_inner())
    }

    fn encode_encryption_request(
        &self,
        server_id: &str,
        public_key_der: &[u8],
        verify_token: &[u8],
    ) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = PacketWriter::default();
        writer.write_varint(0x01);
        writer.write_string(server_id)?;
        write_login_byte_array(&mut writer, public_key_der)?;
        write_login_byte_array(&mut writer, verify_token)?;
        Ok(writer.into_inner())
    }

    fn encode_network_settings(
        &self,
        _compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError> {
        Err(ProtocolError::InvalidPacket(
            "java edition adapters do not support bedrock network settings",
        ))
    }

    fn encode_login_success(&self, player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
        encode_login_success_packet(player)
    }
}

impl<P: JavaEditionProfile> PlaySyncAdapter for JavaEditionAdapter<P> {
    fn decode_play(
        &self,
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError> {
        self.profile.decode_play(player_id, frame)
    }

    fn encode_play_event(
        &self,
        event: &CoreEvent,
        _context: &PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        match event {
            CoreEvent::PlayBootstrap {
                player,
                entity_id,
                world_meta,
                ..
            } => self
                .profile
                .encode_play_bootstrap(*entity_id, world_meta, player),
            CoreEvent::ChunkBatch { chunks } => self.profile.encode_chunk_batch(chunks),
            CoreEvent::EntitySpawned { entity_id, player } => {
                self.profile.encode_entity_spawn(*entity_id, player)
            }
            CoreEvent::EntityMoved { entity_id, player } => {
                self.profile.encode_entity_moved(*entity_id, player)
            }
            CoreEvent::EntityDespawned { entity_ids } => {
                Ok(vec![self.profile.encode_entity_despawn(entity_ids)?])
            }
            CoreEvent::InventoryContents {
                container,
                inventory,
            } => Ok(vec![
                self.profile
                    .encode_inventory_contents(*container, inventory)?,
            ]),
            CoreEvent::InventorySlotChanged {
                container,
                slot,
                stack,
            } => Ok(self
                .profile
                .encode_inventory_slot_changed(*container, *slot, stack.as_ref())?
                .into_iter()
                .collect()),
            CoreEvent::SelectedHotbarSlotChanged { slot } => Ok(vec![
                self.profile.encode_selected_hotbar_slot_changed(*slot)?,
            ]),
            CoreEvent::BlockChanged { position, block } => {
                Ok(vec![self.profile.encode_block_changed(*position, block)?])
            }
            CoreEvent::KeepAliveRequested { keep_alive_id } => Ok(vec![
                self.profile.encode_keep_alive_requested(*keep_alive_id)?,
            ]),
            CoreEvent::LoginAccepted { .. } | CoreEvent::Disconnect { .. } => Err(
                ProtocolError::InvalidPacket("session event cannot be encoded as play sync"),
            ),
        }
    }
}

impl<P: JavaEditionProfile> ProtocolAdapter for JavaEditionAdapter<P> {
    fn descriptor(&self) -> ProtocolDescriptor {
        self.profile.descriptor()
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

const fn sign_extend(value: i64, bits: u8) -> i64 {
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

#[doc(hidden)]
pub mod internal {
    pub use super::{
        build_chunk_data_1_7, build_chunk_data_1_8, build_chunk_data_1_12, decode_handshake_frame,
        encode_login_success_packet, encode_status_pong_packet, encode_status_response_packet,
        get_nibble, legacy_block, legacy_block_state_id, legacy_inventory_slot, legacy_item,
        legacy_window_items, legacy_window_slot, modern_inventory_slot, modern_window_items,
        modern_window_slot, pack_block_position, player_window_id, read_legacy_slot,
        read_login_byte_array, semantic_block, semantic_item, to_angle_byte, to_fixed_point,
        unpack_block_position, write_empty_metadata_1_8, write_empty_metadata_1_12,
        write_legacy_slot, write_login_byte_array, zlib_compress,
    };
}
