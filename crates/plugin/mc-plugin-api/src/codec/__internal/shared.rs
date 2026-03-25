use crate::codec::__internal::binary::{Decoder, Encoder, ProtocolCodecError};
use crate::codec::__internal::inventory::{
    decode_inventory_click_button, decode_inventory_click_target,
    decode_inventory_click_validation, decode_inventory_container, decode_inventory_slot,
    decode_inventory_transaction_context, decode_inventory_window_contents, decode_item_stack,
    decode_player_inventory, encode_inventory_click_button, encode_inventory_click_target,
    encode_inventory_click_validation, encode_inventory_container, encode_inventory_slot,
    encode_inventory_transaction_context, encode_inventory_window_contents, encode_item_stack,
    encode_player_inventory,
};
use mc_core::{
    BlockEntityState, BlockFace, BlockPos, BlockState, CapabilityAnnouncement, ChunkColumn,
    ChunkSection, ClosedCapability, ClosedCapabilitySet, ConnectionId, CoreCommand, CoreEvent,
    DimensionId, DroppedItemSnapshot, EntityId, InteractionHand, PlayerId, PlayerSnapshot,
    PluginBuildTag, Vec3, WorldMeta, WorldSnapshot, expand_block_index,
};
use mc_proto_common::ConnectionPhase;
use std::collections::BTreeMap;
use uuid::Uuid;

pub(crate) fn encode_option<T>(
    encoder: &mut Encoder,
    value: Option<&T>,
    encode: fn(&mut Encoder, &T) -> Result<(), ProtocolCodecError>,
) -> Result<(), ProtocolCodecError> {
    if let Some(value) = value {
        encoder.write_bool(true);
        encode(encoder, value)
    } else {
        encoder.write_bool(false);
        Ok(())
    }
}

pub(crate) fn decode_option<T>(
    decoder: &mut Decoder<'_>,
    decode: fn(&mut Decoder<'_>) -> Result<T, ProtocolCodecError>,
) -> Result<Option<T>, ProtocolCodecError> {
    if decoder.read_bool()? {
        Ok(Some(decode(decoder)?))
    } else {
        Ok(None)
    }
}

pub(crate) fn decode_u8_value(decoder: &mut Decoder<'_>) -> Result<u8, ProtocolCodecError> {
    decoder.read_u8()
}

pub(crate) fn decode_f32_value(decoder: &mut Decoder<'_>) -> Result<f32, ProtocolCodecError> {
    decoder.read_f32()
}

pub(crate) fn encode_connection_phase(encoder: &mut Encoder, phase: ConnectionPhase) {
    encoder.write_u8(match phase {
        ConnectionPhase::Handshaking => 1,
        ConnectionPhase::Status => 2,
        ConnectionPhase::Login => 3,
        ConnectionPhase::Play => 4,
    });
}

pub(crate) fn decode_connection_phase(
    decoder: &mut Decoder<'_>,
) -> Result<ConnectionPhase, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(ConnectionPhase::Handshaking),
        2 => Ok(ConnectionPhase::Status),
        3 => Ok(ConnectionPhase::Login),
        4 => Ok(ConnectionPhase::Play),
        _ => Err(ProtocolCodecError::InvalidValue("invalid connection phase")),
    }
}

pub(crate) fn encode_dimension_id(encoder: &mut Encoder, dimension: DimensionId) {
    encoder.write_u8(match dimension {
        DimensionId::Overworld => 1,
    });
}

pub(crate) fn decode_dimension_id(
    decoder: &mut Decoder<'_>,
) -> Result<DimensionId, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(DimensionId::Overworld),
        _ => Err(ProtocolCodecError::InvalidValue("invalid dimension id")),
    }
}

pub(crate) fn encode_interaction_hand(encoder: &mut Encoder, hand: InteractionHand) {
    encoder.write_u8(match hand {
        InteractionHand::Main => 1,
        InteractionHand::Offhand => 2,
    });
}

pub(crate) fn decode_interaction_hand(
    decoder: &mut Decoder<'_>,
) -> Result<InteractionHand, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(InteractionHand::Main),
        2 => Ok(InteractionHand::Offhand),
        _ => Err(ProtocolCodecError::InvalidValue("invalid interaction hand")),
    }
}

pub(crate) fn encode_block_face(encoder: &mut Encoder, face: BlockFace) {
    encoder.write_u8(match face {
        BlockFace::Bottom => 1,
        BlockFace::Top => 2,
        BlockFace::North => 3,
        BlockFace::South => 4,
        BlockFace::West => 5,
        BlockFace::East => 6,
    });
}

pub(crate) fn decode_block_face(
    decoder: &mut Decoder<'_>,
) -> Result<BlockFace, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(BlockFace::Bottom),
        2 => Ok(BlockFace::Top),
        3 => Ok(BlockFace::North),
        4 => Ok(BlockFace::South),
        5 => Ok(BlockFace::West),
        6 => Ok(BlockFace::East),
        _ => Err(ProtocolCodecError::InvalidValue("invalid block face")),
    }
}

pub(crate) fn encode_player_id(encoder: &mut Encoder, player_id: PlayerId) {
    encoder.write_raw(player_id.0.as_bytes());
}

pub(crate) fn decode_player_id(decoder: &mut Decoder<'_>) -> Result<PlayerId, ProtocolCodecError> {
    let bytes = decoder.read_exact::<16>()?;
    Ok(PlayerId(Uuid::from_bytes(bytes)))
}

pub(crate) fn encode_entity_id(encoder: &mut Encoder, entity_id: EntityId) {
    encoder.write_i32(entity_id.0);
}

pub(crate) fn decode_entity_id(decoder: &mut Decoder<'_>) -> Result<EntityId, ProtocolCodecError> {
    Ok(EntityId(decoder.read_i32()?))
}

pub(crate) fn encode_connection_id(encoder: &mut Encoder, connection_id: ConnectionId) {
    encoder.write_u64(connection_id.0);
}

pub(crate) fn decode_connection_id(
    decoder: &mut Decoder<'_>,
) -> Result<ConnectionId, ProtocolCodecError> {
    Ok(ConnectionId(decoder.read_u64()?))
}

pub(crate) fn encode_capability_announcement<C>(
    encoder: &mut Encoder,
    announcement: &CapabilityAnnouncement<C>,
) -> Result<(), ProtocolCodecError>
where
    C: ClosedCapability,
{
    encoder.write_len(
        announcement.capabilities.len() + usize::from(announcement.build_tag.is_some()),
    )?;
    for capability in announcement.capabilities.iter() {
        encoder.write_string(capability.as_str())?;
    }
    if let Some(build_tag) = &announcement.build_tag {
        encoder.write_string(&format!("build-tag:{}", build_tag.as_str()))?;
    }
    Ok(())
}

pub(crate) fn decode_capability_announcement<C>(
    decoder: &mut Decoder<'_>,
) -> Result<CapabilityAnnouncement<C>, ProtocolCodecError>
where
    C: ClosedCapability,
{
    let len = decoder.read_len()?;
    let mut capabilities = ClosedCapabilitySet::new();
    let mut build_tag = None;
    for _ in 0..len {
        let token = decoder.read_string()?;
        if let Some(raw_build_tag) = token.strip_prefix("build-tag:") {
            if raw_build_tag.is_empty() {
                return Err(ProtocolCodecError::InvalidValue(
                    "build tag capability must not be empty",
                ));
            }
            if build_tag.is_some() {
                return Err(ProtocolCodecError::InvalidValue(
                    "duplicate build tag capability",
                ));
            }
            build_tag = Some(PluginBuildTag::new(raw_build_tag));
            continue;
        }
        let capability = C::parse(&token)
            .map_err(|_| ProtocolCodecError::InvalidValue("invalid plugin capability"))?;
        if !capabilities.insert(capability) {
            return Err(ProtocolCodecError::InvalidValue(
                "duplicate plugin capability",
            ));
        }
    }
    Ok(CapabilityAnnouncement {
        capabilities,
        build_tag,
    })
}

pub(crate) fn encode_block_pos(encoder: &mut Encoder, position: BlockPos) {
    encoder.write_i32(position.x);
    encoder.write_i32(position.y);
    encoder.write_i32(position.z);
}

pub(crate) fn decode_block_pos(decoder: &mut Decoder<'_>) -> Result<BlockPos, ProtocolCodecError> {
    Ok(BlockPos::new(
        decoder.read_i32()?,
        decoder.read_i32()?,
        decoder.read_i32()?,
    ))
}

pub(crate) fn encode_vec3(encoder: &mut Encoder, position: Vec3) {
    encoder.write_f64(position.x);
    encoder.write_f64(position.y);
    encoder.write_f64(position.z);
}

pub(crate) fn decode_vec3(decoder: &mut Decoder<'_>) -> Result<Vec3, ProtocolCodecError> {
    Ok(Vec3::new(
        decoder.read_f64()?,
        decoder.read_f64()?,
        decoder.read_f64()?,
    ))
}

pub(crate) fn encode_block_state(
    encoder: &mut Encoder,
    block_state: &BlockState,
) -> Result<(), ProtocolCodecError> {
    encoder.write_string(block_state.key.as_str())?;
    encoder.write_len(block_state.properties.len())?;
    for (key, value) in &block_state.properties {
        encoder.write_string(key)?;
        encoder.write_string(value)?;
    }
    Ok(())
}

pub(crate) fn decode_block_state(
    decoder: &mut Decoder<'_>,
) -> Result<BlockState, ProtocolCodecError> {
    let key = decoder.read_string()?;
    let len = decoder.read_len()?;
    let mut properties = BTreeMap::new();
    for _ in 0..len {
        let key = decoder.read_string()?;
        let value = decoder.read_string()?;
        properties.insert(key, value);
    }
    Ok(BlockState {
        key: mc_core::BlockKey::new(key),
        properties,
    })
}

pub(crate) fn encode_dropped_item_snapshot(
    encoder: &mut Encoder,
    item: &DroppedItemSnapshot,
) -> Result<(), ProtocolCodecError> {
    encode_item_stack(encoder, &item.item)?;
    encode_vec3(encoder, item.position);
    encode_vec3(encoder, item.velocity);
    Ok(())
}

pub(crate) fn decode_dropped_item_snapshot(
    decoder: &mut Decoder<'_>,
) -> Result<DroppedItemSnapshot, ProtocolCodecError> {
    Ok(DroppedItemSnapshot {
        item: decode_item_stack(decoder)?,
        position: decode_vec3(decoder)?,
        velocity: decode_vec3(decoder)?,
    })
}

pub(crate) fn encode_block_entity_state(
    encoder: &mut Encoder,
    block_entity: &BlockEntityState,
) -> Result<(), ProtocolCodecError> {
    match block_entity {
        BlockEntityState::Chest { slots } => {
            encoder.write_u8(1);
            encoder.write_len(slots.len())?;
            for slot in slots {
                encode_option(encoder, slot.as_ref(), encode_item_stack)?;
            }
        }
        BlockEntityState::Furnace {
            input,
            fuel,
            output,
            burn_left,
            burn_max,
            cook_progress,
            cook_total,
        } => {
            encoder.write_u8(2);
            encode_option(encoder, input.as_ref(), encode_item_stack)?;
            encode_option(encoder, fuel.as_ref(), encode_item_stack)?;
            encode_option(encoder, output.as_ref(), encode_item_stack)?;
            encoder.write_i16(*burn_left);
            encoder.write_i16(*burn_max);
            encoder.write_i16(*cook_progress);
            encoder.write_i16(*cook_total);
        }
    }
    Ok(())
}

pub(crate) fn decode_block_entity_state(
    decoder: &mut Decoder<'_>,
) -> Result<BlockEntityState, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => {
            let len = decoder.read_len()?;
            let mut slots = Vec::with_capacity(len);
            for _ in 0..len {
                slots.push(decode_option(decoder, decode_item_stack)?);
            }
            Ok(BlockEntityState::Chest { slots })
        }
        2 => Ok(BlockEntityState::Furnace {
            input: decode_option(decoder, decode_item_stack)?,
            fuel: decode_option(decoder, decode_item_stack)?,
            output: decode_option(decoder, decode_item_stack)?,
            burn_left: decoder.read_i16()?,
            burn_max: decoder.read_i16()?,
            cook_progress: decoder.read_i16()?,
            cook_total: decoder.read_i16()?,
        }),
        _ => Err(ProtocolCodecError::InvalidValue(
            "invalid block entity state",
        )),
    }
}

fn encode_chunk_section(
    encoder: &mut Encoder,
    section: &ChunkSection,
) -> Result<(), ProtocolCodecError> {
    encoder.write_i32(section.y);
    let blocks = section.iter_blocks().collect::<Vec<_>>();
    encoder.write_len(blocks.len())?;
    for (index, state) in blocks {
        encoder.write_u16(index.into());
        encode_block_state(encoder, state)?;
    }
    Ok(())
}

fn decode_chunk_section(decoder: &mut Decoder<'_>) -> Result<ChunkSection, ProtocolCodecError> {
    let section_y = decoder.read_i32()?;
    let block_len = decoder.read_len()?;
    let mut section = ChunkSection::new(section_y);
    for _ in 0..block_len {
        let index = decoder.read_u16()?;
        let state = decode_block_state(decoder)?;
        let (x, y, z) = expand_block_index(index);
        section.set_block(x, y, z, state);
    }
    Ok(section)
}

pub(crate) fn encode_chunk_column(
    encoder: &mut Encoder,
    chunk: &ChunkColumn,
) -> Result<(), ProtocolCodecError> {
    encoder.write_i32(chunk.pos.x);
    encoder.write_i32(chunk.pos.z);
    encoder.write_len(chunk.sections.len())?;
    for section in chunk.sections.values() {
        encode_chunk_section(encoder, section)?;
    }
    encoder.write_bytes(&chunk.biomes)?;
    Ok(())
}

pub(crate) fn decode_chunk_column(
    decoder: &mut Decoder<'_>,
) -> Result<ChunkColumn, ProtocolCodecError> {
    let chunk_pos = mc_core::ChunkPos::new(decoder.read_i32()?, decoder.read_i32()?);
    let section_len = decoder.read_len()?;
    let mut sections = BTreeMap::new();
    for _ in 0..section_len {
        let section = decode_chunk_section(decoder)?;
        sections.insert(section.y, section);
    }
    Ok(ChunkColumn {
        pos: chunk_pos,
        sections,
        biomes: decoder.read_bytes()?,
    })
}

pub(crate) fn encode_world_meta(
    encoder: &mut Encoder,
    meta: &WorldMeta,
) -> Result<(), ProtocolCodecError> {
    encoder.write_string(&meta.level_name)?;
    encoder.write_u64(meta.seed);
    encode_block_pos(encoder, meta.spawn);
    encode_dimension_id(encoder, meta.dimension);
    encoder.write_i64(meta.age);
    encoder.write_i64(meta.time);
    encoder.write_string(&meta.level_type)?;
    encoder.write_u8(meta.game_mode);
    encoder.write_u8(meta.difficulty);
    encoder.write_u8(meta.max_players);
    Ok(())
}

pub(crate) fn decode_world_meta(
    decoder: &mut Decoder<'_>,
) -> Result<WorldMeta, ProtocolCodecError> {
    Ok(WorldMeta {
        level_name: decoder.read_string()?,
        seed: decoder.read_u64()?,
        spawn: decode_block_pos(decoder)?,
        dimension: decode_dimension_id(decoder)?,
        age: decoder.read_i64()?,
        time: decoder.read_i64()?,
        level_type: decoder.read_string()?,
        game_mode: decoder.read_u8()?,
        difficulty: decoder.read_u8()?,
        max_players: decoder.read_u8()?,
    })
}

pub(crate) fn encode_world_snapshot(
    encoder: &mut Encoder,
    snapshot: &WorldSnapshot,
) -> Result<(), ProtocolCodecError> {
    encode_world_meta(encoder, &snapshot.meta)?;
    encoder.write_len(snapshot.chunks.len())?;
    for chunk in snapshot.chunks.values() {
        encode_chunk_column(encoder, chunk)?;
    }
    encoder.write_len(snapshot.block_entities.len())?;
    for (position, block_entity) in &snapshot.block_entities {
        encode_block_pos(encoder, *position);
        encode_block_entity_state(encoder, block_entity)?;
    }
    encoder.write_len(snapshot.players.len())?;
    for player in snapshot.players.values() {
        encode_player_snapshot(encoder, player)?;
    }
    Ok(())
}

pub(crate) fn decode_world_snapshot(
    decoder: &mut Decoder<'_>,
) -> Result<WorldSnapshot, ProtocolCodecError> {
    let meta = decode_world_meta(decoder)?;
    let chunk_len = decoder.read_len()?;
    let mut chunks = BTreeMap::new();
    for _ in 0..chunk_len {
        let chunk = decode_chunk_column(decoder)?;
        chunks.insert(chunk.pos, chunk);
    }
    let block_entity_len = decoder.read_len()?;
    let mut block_entities = BTreeMap::new();
    for _ in 0..block_entity_len {
        let position = decode_block_pos(decoder)?;
        let block_entity = decode_block_entity_state(decoder)?;
        block_entities.insert(position, block_entity);
    }
    let player_len = decoder.read_len()?;
    let mut players = BTreeMap::new();
    for _ in 0..player_len {
        let player = decode_player_snapshot(decoder)?;
        players.insert(player.id, player);
    }
    Ok(WorldSnapshot {
        meta,
        chunks,
        block_entities,
        players,
    })
}

pub(crate) fn encode_player_snapshot(
    encoder: &mut Encoder,
    player: &PlayerSnapshot,
) -> Result<(), ProtocolCodecError> {
    encode_player_id(encoder, player.id);
    encoder.write_string(&player.username)?;
    encode_vec3(encoder, player.position);
    encoder.write_f32(player.yaw);
    encoder.write_f32(player.pitch);
    encoder.write_bool(player.on_ground);
    encode_dimension_id(encoder, player.dimension);
    encoder.write_f32(player.health);
    encoder.write_i16(player.food);
    encoder.write_f32(player.food_saturation);
    encode_player_inventory(encoder, &player.inventory)?;
    encoder.write_u8(player.selected_hotbar_slot);
    Ok(())
}

pub(crate) fn decode_player_snapshot(
    decoder: &mut Decoder<'_>,
) -> Result<PlayerSnapshot, ProtocolCodecError> {
    Ok(PlayerSnapshot {
        id: decode_player_id(decoder)?,
        username: decoder.read_string()?,
        position: decode_vec3(decoder)?,
        yaw: decoder.read_f32()?,
        pitch: decoder.read_f32()?,
        on_ground: decoder.read_bool()?,
        dimension: decode_dimension_id(decoder)?,
        health: decoder.read_f32()?,
        food: decoder.read_i16()?,
        food_saturation: decoder.read_f32()?,
        inventory: decode_player_inventory(decoder)?,
        selected_hotbar_slot: decoder.read_u8()?,
    })
}

pub(crate) fn encode_core_command(
    encoder: &mut Encoder,
    command: &CoreCommand,
) -> Result<(), ProtocolCodecError> {
    match command {
        CoreCommand::LoginStart {
            connection_id,
            username,
            player_id,
        } => {
            encoder.write_u8(1);
            encode_connection_id(encoder, *connection_id);
            encoder.write_string(username)?;
            encode_player_id(encoder, *player_id);
        }
        CoreCommand::UpdateClientView {
            player_id,
            view_distance,
        } => {
            encoder.write_u8(2);
            encode_player_id(encoder, *player_id);
            encoder.write_u8(*view_distance);
        }
        CoreCommand::ClientStatus {
            player_id,
            action_id,
        } => {
            encoder.write_u8(3);
            encode_player_id(encoder, *player_id);
            encoder.write_i8(*action_id);
        }
        CoreCommand::MoveIntent {
            player_id,
            position,
            yaw,
            pitch,
            on_ground,
        } => {
            encoder.write_u8(4);
            encode_player_id(encoder, *player_id);
            encode_option(encoder, position.as_ref(), |encoder, position| {
                encode_vec3(encoder, *position);
                Ok(())
            })?;
            encode_option(encoder, yaw.as_ref(), |encoder, value| {
                encoder.write_f32(*value);
                Ok(())
            })?;
            encode_option(encoder, pitch.as_ref(), |encoder, value| {
                encoder.write_f32(*value);
                Ok(())
            })?;
            encoder.write_bool(*on_ground);
        }
        CoreCommand::KeepAliveResponse {
            player_id,
            keep_alive_id,
        } => {
            encoder.write_u8(5);
            encode_player_id(encoder, *player_id);
            encoder.write_i32(*keep_alive_id);
        }
        CoreCommand::SetHeldSlot { player_id, slot } => {
            encoder.write_u8(6);
            encode_player_id(encoder, *player_id);
            encoder.write_i16(*slot);
        }
        CoreCommand::CreativeInventorySet {
            player_id,
            slot,
            stack,
        } => {
            encoder.write_u8(7);
            encode_player_id(encoder, *player_id);
            encode_inventory_slot(encoder, *slot);
            encode_option(encoder, stack.as_ref(), encode_item_stack)?;
        }
        CoreCommand::InventoryClick {
            player_id,
            transaction,
            target,
            button,
            validation,
        } => {
            encoder.write_u8(11);
            encode_player_id(encoder, *player_id);
            encode_inventory_transaction_context(encoder, *transaction);
            encode_inventory_click_target(encoder, *target);
            encode_inventory_click_button(encoder, *button);
            encode_inventory_click_validation(encoder, validation)?;
        }
        CoreCommand::InventoryTransactionAck {
            player_id,
            transaction,
            accepted,
        } => {
            encoder.write_u8(12);
            encode_player_id(encoder, *player_id);
            encode_inventory_transaction_context(encoder, *transaction);
            encoder.write_bool(*accepted);
        }
        CoreCommand::CloseContainer {
            player_id,
            window_id,
        } => {
            encoder.write_u8(13);
            encode_player_id(encoder, *player_id);
            encoder.write_u8(*window_id);
        }
        CoreCommand::DigBlock {
            player_id,
            position,
            status,
            face,
        } => {
            encoder.write_u8(8);
            encode_player_id(encoder, *player_id);
            encode_block_pos(encoder, *position);
            encoder.write_u8(*status);
            encode_option(encoder, face.as_ref(), |encoder, face| {
                encode_block_face(encoder, *face);
                Ok(())
            })?;
        }
        CoreCommand::PlaceBlock {
            player_id,
            hand,
            position,
            face,
            held_item,
        } => {
            encoder.write_u8(9);
            encode_player_id(encoder, *player_id);
            encode_interaction_hand(encoder, *hand);
            encode_block_pos(encoder, *position);
            encode_option(encoder, face.as_ref(), |encoder, face| {
                encode_block_face(encoder, *face);
                Ok(())
            })?;
            encode_option(encoder, held_item.as_ref(), encode_item_stack)?;
        }
        CoreCommand::UseBlock {
            player_id,
            hand,
            position,
            face,
            held_item,
        } => {
            encoder.write_u8(14);
            encode_player_id(encoder, *player_id);
            encode_interaction_hand(encoder, *hand);
            encode_block_pos(encoder, *position);
            encode_option(encoder, face.as_ref(), |encoder, face| {
                encode_block_face(encoder, *face);
                Ok(())
            })?;
            encode_option(encoder, held_item.as_ref(), encode_item_stack)?;
        }
        CoreCommand::Disconnect { player_id } => {
            encoder.write_u8(10);
            encode_player_id(encoder, *player_id);
        }
    }
    Ok(())
}

pub(crate) fn decode_core_command(
    decoder: &mut Decoder<'_>,
) -> Result<CoreCommand, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(CoreCommand::LoginStart {
            connection_id: decode_connection_id(decoder)?,
            username: decoder.read_string()?,
            player_id: decode_player_id(decoder)?,
        }),
        2 => Ok(CoreCommand::UpdateClientView {
            player_id: decode_player_id(decoder)?,
            view_distance: decoder.read_u8()?,
        }),
        3 => Ok(CoreCommand::ClientStatus {
            player_id: decode_player_id(decoder)?,
            action_id: decoder.read_i8()?,
        }),
        4 => Ok(CoreCommand::MoveIntent {
            player_id: decode_player_id(decoder)?,
            position: decode_option(decoder, decode_vec3)?,
            yaw: decode_option(decoder, decode_f32_value)?,
            pitch: decode_option(decoder, decode_f32_value)?,
            on_ground: decoder.read_bool()?,
        }),
        5 => Ok(CoreCommand::KeepAliveResponse {
            player_id: decode_player_id(decoder)?,
            keep_alive_id: decoder.read_i32()?,
        }),
        6 => Ok(CoreCommand::SetHeldSlot {
            player_id: decode_player_id(decoder)?,
            slot: decoder.read_i16()?,
        }),
        7 => Ok(CoreCommand::CreativeInventorySet {
            player_id: decode_player_id(decoder)?,
            slot: decode_inventory_slot(decoder)?,
            stack: decode_option(decoder, decode_item_stack)?,
        }),
        11 => Ok(CoreCommand::InventoryClick {
            player_id: decode_player_id(decoder)?,
            transaction: decode_inventory_transaction_context(decoder)?,
            target: decode_inventory_click_target(decoder)?,
            button: decode_inventory_click_button(decoder)?,
            validation: decode_inventory_click_validation(decoder)?,
        }),
        12 => Ok(CoreCommand::InventoryTransactionAck {
            player_id: decode_player_id(decoder)?,
            transaction: decode_inventory_transaction_context(decoder)?,
            accepted: decoder.read_bool()?,
        }),
        13 => Ok(CoreCommand::CloseContainer {
            player_id: decode_player_id(decoder)?,
            window_id: decoder.read_u8()?,
        }),
        8 => Ok(CoreCommand::DigBlock {
            player_id: decode_player_id(decoder)?,
            position: decode_block_pos(decoder)?,
            status: decoder.read_u8()?,
            face: decode_option(decoder, decode_block_face)?,
        }),
        9 => Ok(CoreCommand::PlaceBlock {
            player_id: decode_player_id(decoder)?,
            hand: decode_interaction_hand(decoder)?,
            position: decode_block_pos(decoder)?,
            face: decode_option(decoder, decode_block_face)?,
            held_item: decode_option(decoder, decode_item_stack)?,
        }),
        14 => Ok(CoreCommand::UseBlock {
            player_id: decode_player_id(decoder)?,
            hand: decode_interaction_hand(decoder)?,
            position: decode_block_pos(decoder)?,
            face: decode_option(decoder, decode_block_face)?,
            held_item: decode_option(decoder, decode_item_stack)?,
        }),
        10 => Ok(CoreCommand::Disconnect {
            player_id: decode_player_id(decoder)?,
        }),
        _ => Err(ProtocolCodecError::InvalidValue("invalid core command tag")),
    }
}

pub(crate) fn encode_core_event(
    encoder: &mut Encoder,
    event: &CoreEvent,
) -> Result<(), ProtocolCodecError> {
    match event {
        CoreEvent::LoginAccepted {
            player_id,
            entity_id,
            player,
        } => {
            encoder.write_u8(1);
            encode_player_id(encoder, *player_id);
            encode_entity_id(encoder, *entity_id);
            encode_player_snapshot(encoder, player)?;
        }
        CoreEvent::PlayBootstrap {
            player,
            entity_id,
            world_meta,
            view_distance,
        } => {
            encoder.write_u8(2);
            encode_player_snapshot(encoder, player)?;
            encode_entity_id(encoder, *entity_id);
            encode_world_meta(encoder, world_meta)?;
            encoder.write_u8(*view_distance);
        }
        CoreEvent::ChunkBatch { chunks } => {
            encoder.write_u8(3);
            encoder.write_len(chunks.len())?;
            for chunk in chunks {
                encode_chunk_column(encoder, chunk)?;
            }
        }
        CoreEvent::EntitySpawned { entity_id, player } => {
            encoder.write_u8(4);
            encode_entity_id(encoder, *entity_id);
            encode_player_snapshot(encoder, player)?;
        }
        CoreEvent::EntityMoved { entity_id, player } => {
            encoder.write_u8(5);
            encode_entity_id(encoder, *entity_id);
            encode_player_snapshot(encoder, player)?;
        }
        CoreEvent::DroppedItemSpawned { entity_id, item } => {
            encoder.write_u8(18);
            encode_entity_id(encoder, *entity_id);
            encode_dropped_item_snapshot(encoder, item)?;
        }
        CoreEvent::BlockBreakingProgress {
            breaker_entity_id,
            position,
            stage,
            duration_ms,
        } => {
            encoder.write_u8(19);
            encode_entity_id(encoder, *breaker_entity_id);
            encode_block_pos(encoder, *position);
            encode_option(encoder, stage.as_ref(), |encoder, stage| {
                encoder.write_u8(*stage);
                Ok(())
            })?;
            encoder.write_u64(*duration_ms);
        }
        CoreEvent::EntityDespawned { entity_ids } => {
            encoder.write_u8(6);
            encoder.write_len(entity_ids.len())?;
            for entity_id in entity_ids {
                encode_entity_id(encoder, *entity_id);
            }
        }
        CoreEvent::InventoryContents {
            window_id,
            container,
            contents,
        } => {
            encoder.write_u8(7);
            encoder.write_u8(*window_id);
            encode_inventory_container(encoder, *container);
            encode_inventory_window_contents(encoder, contents)?;
        }
        CoreEvent::ContainerOpened {
            window_id,
            container,
            title,
        } => {
            encoder.write_u8(15);
            encoder.write_u8(*window_id);
            encode_inventory_container(encoder, *container);
            encoder.write_string(title)?;
        }
        CoreEvent::ContainerClosed { window_id } => {
            encoder.write_u8(16);
            encoder.write_u8(*window_id);
        }
        CoreEvent::ContainerPropertyChanged {
            window_id,
            property_id,
            value,
        } => {
            encoder.write_u8(17);
            encoder.write_u8(*window_id);
            encoder.write_u8(*property_id);
            encoder.write_i16(*value);
        }
        CoreEvent::InventorySlotChanged {
            window_id,
            container,
            slot,
            stack,
        } => {
            encoder.write_u8(8);
            encoder.write_u8(*window_id);
            encode_inventory_container(encoder, *container);
            encode_inventory_slot(encoder, *slot);
            encode_option(encoder, stack.as_ref(), encode_item_stack)?;
        }
        CoreEvent::InventoryTransactionProcessed {
            transaction,
            accepted,
        } => {
            encoder.write_u8(14);
            encode_inventory_transaction_context(encoder, *transaction);
            encoder.write_bool(*accepted);
        }
        CoreEvent::CursorChanged { stack } => {
            encoder.write_u8(13);
            encode_option(encoder, stack.as_ref(), encode_item_stack)?;
        }
        CoreEvent::SelectedHotbarSlotChanged { slot } => {
            encoder.write_u8(9);
            encoder.write_u8(*slot);
        }
        CoreEvent::BlockChanged { position, block } => {
            encoder.write_u8(10);
            encode_block_pos(encoder, *position);
            encode_block_state(encoder, block)?;
        }
        CoreEvent::KeepAliveRequested { keep_alive_id } => {
            encoder.write_u8(11);
            encoder.write_i32(*keep_alive_id);
        }
        CoreEvent::Disconnect { reason } => {
            encoder.write_u8(12);
            encoder.write_string(reason)?;
        }
    }
    Ok(())
}

pub(crate) fn decode_core_event(
    decoder: &mut Decoder<'_>,
) -> Result<CoreEvent, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(CoreEvent::LoginAccepted {
            player_id: decode_player_id(decoder)?,
            entity_id: decode_entity_id(decoder)?,
            player: decode_player_snapshot(decoder)?,
        }),
        2 => Ok(CoreEvent::PlayBootstrap {
            player: decode_player_snapshot(decoder)?,
            entity_id: decode_entity_id(decoder)?,
            world_meta: decode_world_meta(decoder)?,
            view_distance: decoder.read_u8()?,
        }),
        3 => {
            let len = decoder.read_len()?;
            let mut chunks = Vec::with_capacity(len);
            for _ in 0..len {
                chunks.push(decode_chunk_column(decoder)?);
            }
            Ok(CoreEvent::ChunkBatch { chunks })
        }
        4 => Ok(CoreEvent::EntitySpawned {
            entity_id: decode_entity_id(decoder)?,
            player: decode_player_snapshot(decoder)?,
        }),
        5 => Ok(CoreEvent::EntityMoved {
            entity_id: decode_entity_id(decoder)?,
            player: decode_player_snapshot(decoder)?,
        }),
        18 => Ok(CoreEvent::DroppedItemSpawned {
            entity_id: decode_entity_id(decoder)?,
            item: decode_dropped_item_snapshot(decoder)?,
        }),
        19 => Ok(CoreEvent::BlockBreakingProgress {
            breaker_entity_id: decode_entity_id(decoder)?,
            position: decode_block_pos(decoder)?,
            stage: decode_option(decoder, |decoder| decoder.read_u8())?,
            duration_ms: decoder.read_u64()?,
        }),
        6 => {
            let len = decoder.read_len()?;
            let mut entity_ids = Vec::with_capacity(len);
            for _ in 0..len {
                entity_ids.push(decode_entity_id(decoder)?);
            }
            Ok(CoreEvent::EntityDespawned { entity_ids })
        }
        7 => Ok(CoreEvent::InventoryContents {
            window_id: decoder.read_u8()?,
            container: decode_inventory_container(decoder)?,
            contents: decode_inventory_window_contents(decoder)?,
        }),
        15 => Ok(CoreEvent::ContainerOpened {
            window_id: decoder.read_u8()?,
            container: decode_inventory_container(decoder)?,
            title: decoder.read_string()?,
        }),
        16 => Ok(CoreEvent::ContainerClosed {
            window_id: decoder.read_u8()?,
        }),
        17 => Ok(CoreEvent::ContainerPropertyChanged {
            window_id: decoder.read_u8()?,
            property_id: decoder.read_u8()?,
            value: decoder.read_i16()?,
        }),
        8 => Ok(CoreEvent::InventorySlotChanged {
            window_id: decoder.read_u8()?,
            container: decode_inventory_container(decoder)?,
            slot: decode_inventory_slot(decoder)?,
            stack: decode_option(decoder, decode_item_stack)?,
        }),
        14 => Ok(CoreEvent::InventoryTransactionProcessed {
            transaction: decode_inventory_transaction_context(decoder)?,
            accepted: decoder.read_bool()?,
        }),
        13 => Ok(CoreEvent::CursorChanged {
            stack: decode_option(decoder, decode_item_stack)?,
        }),
        9 => Ok(CoreEvent::SelectedHotbarSlotChanged {
            slot: decoder.read_u8()?,
        }),
        10 => Ok(CoreEvent::BlockChanged {
            position: decode_block_pos(decoder)?,
            block: decode_block_state(decoder)?,
        }),
        11 => Ok(CoreEvent::KeepAliveRequested {
            keep_alive_id: decoder.read_i32()?,
        }),
        12 => Ok(CoreEvent::Disconnect {
            reason: decoder.read_string()?,
        }),
        _ => Err(ProtocolCodecError::InvalidValue("invalid core event tag")),
    }
}
