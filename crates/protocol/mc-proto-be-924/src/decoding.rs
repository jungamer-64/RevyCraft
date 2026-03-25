use crate::codec::decode_v924;
use crate::inventory::{
    request_transaction, translate_drop_action, translate_place_action, translate_swap_action,
    translate_take_action,
};
use bedrockrs_proto::V924;
use bedrockrs_proto::v662::enums::{
    ComplexInventoryTransactionType, ItemUseInventoryTransactionType, PlayerActionType,
};
use bedrockrs_proto::v662::packets::{
    ClientCacheStatusPacket, ItemStackRequestPacket, LoginPacket, MobEquipmentPacket,
    MovePlayerPacket, PlayerActionPacket, RequestNetworkSettingsPacket,
    ResourcePackClientResponsePacket,
};
use bedrockrs_proto::v712::enums::ItemStackRequestActionType;
use bedrockrs_proto::v766::packets::PlayerAuthInputPacket;
use bedrockrs_proto_core::{PacketHeader, ProtoCodec, ProtoCodecVAR};
use mc_core::{CoreCommand, InteractionHand, PlayerId, Vec3};
use mc_proto_be_common::__version_support::{
    login::parse_bedrock_login_payload,
    world::{block_face_from_i32, block_pos_from_network, protocol_error},
};
use mc_proto_common::{LoginRequest, ProtocolError};
use std::io::Cursor;

pub(crate) fn decode_login_request(frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
    let packets = decode_v924(frame)?;
    let packet = packets
        .into_iter()
        .next()
        .ok_or_else(|| protocol_error("bedrock login frame was empty"))?;
    match packet {
        V924::RequestNetworkSettingsPacket(RequestNetworkSettingsPacket {
            client_network_version,
        }) => Ok(LoginRequest::BedrockNetworkSettingsRequest {
            protocol_number: client_network_version,
        }),
        V924::LoginPacket(LoginPacket {
            client_network_version,
            connection_request,
        }) => {
            let login = parse_bedrock_login_payload(&connection_request)
                .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
            Ok(LoginRequest::BedrockLogin {
                protocol_number: client_network_version,
                display_name: login.display_name,
                chain_jwts: login.chain_jwts,
                client_data_jwt: login.client_data_jwt,
            })
        }
        V924::ClientCacheStatusPacket(ClientCacheStatusPacket { .. })
        | V924::ResourcePackClientResponsePacket(ResourcePackClientResponsePacket { .. }) => Err(
            protocol_error("bedrock login control packet arrived in login phase"),
        ),
        _ => Err(protocol_error("unsupported bedrock login packet")),
    }
}

pub(crate) fn decode_play_packet(
    player_id: PlayerId,
    frame: &[u8],
) -> Result<Option<CoreCommand>, ProtocolError> {
    if let Some(command) = decode_inventory_transaction_frame(player_id, frame)? {
        return Ok(Some(command));
    }

    let packets = decode_v924(frame)?;
    let Some(packet) = packets.into_iter().next() else {
        return Ok(None);
    };
    match packet {
        V924::MovePlayerPacket(MovePlayerPacket {
            position,
            rotation,
            on_ground,
            ..
        }) => Ok(Some(CoreCommand::MoveIntent {
            player_id,
            position: Some(Vec3::new(
                f64::from(position.x),
                f64::from(position.y),
                f64::from(position.z),
            )),
            yaw: Some(rotation.x),
            pitch: Some(rotation.y),
            on_ground,
        })),
        V924::MobEquipmentPacket(MobEquipmentPacket { slot, .. }) => {
            Ok(Some(CoreCommand::SetHeldSlot {
                player_id,
                slot: i16::from(slot),
            }))
        }
        V924::PlayerActionPacket(PlayerActionPacket {
            action,
            block_position,
            face,
            ..
        }) => match action {
            PlayerActionType::StartDestroyBlock | PlayerActionType::ContinueDestroyBlock => {
                Ok(Some(CoreCommand::DigBlock {
                    player_id,
                    position: block_pos_from_network(&block_position),
                    status: 0,
                    face: block_face_from_i32(face),
                }))
            }
            PlayerActionType::AbortDestroyBlock | PlayerActionType::StopDestroyBlock => {
                Ok(Some(CoreCommand::DigBlock {
                    player_id,
                    position: block_pos_from_network(&block_position),
                    status: 1,
                    face: block_face_from_i32(face),
                }))
            }
            PlayerActionType::CreativeDestroyBlock | PlayerActionType::PredictDestroyBlock => {
                Ok(Some(CoreCommand::DigBlock {
                    player_id,
                    position: block_pos_from_network(&block_position),
                    status: 2,
                    face: block_face_from_i32(face),
                }))
            }
            _ => Ok(None),
        },
        V924::ItemStackRequestPacket(ItemStackRequestPacket { requests }) => {
            decode_item_stack_request_packet(player_id, &requests)
        }
        V924::PlayerAuthInputPacket(PlayerAuthInputPacket {
            item_stack_request,
            item_use_transaction,
            player_position,
            player_rotation,
            ..
        }) => {
            if let Some(request) = item_stack_request {
                return decode_auth_input_stack_request(player_id, &request);
            }
            if let Some(transaction) = item_use_transaction {
                return decode_item_use_transaction(player_id, &transaction);
            }
            Ok(Some(CoreCommand::MoveIntent {
                player_id,
                position: Some(Vec3::new(
                    f64::from(player_position.x),
                    f64::from(player_position.y),
                    f64::from(player_position.z),
                )),
                yaw: Some(player_rotation.x),
                pitch: Some(player_rotation.y),
                on_ground: true,
            }))
        }
        V924::ContainerClosePacket(packet) => Ok(matches!(
            packet.container_id,
            bedrockrs_proto::v662::enums::ContainerID::First
        )
        .then_some(CoreCommand::CloseContainer {
            player_id,
            window_id: 0,
        })),
        V924::ClientCacheStatusPacket(_) | V924::ResourcePackClientResponsePacket(_) => Ok(None),
        _ => Ok(None),
    }
}

fn decode_item_use_transaction(
    player_id: PlayerId,
    transaction: &bedrockrs_proto::v712::types::PackedItemUseLegacyInventoryTransaction<V924>,
) -> Result<Option<CoreCommand>, ProtocolError> {
    Ok(Some(match transaction.action_type {
        ItemUseInventoryTransactionType::Place => CoreCommand::PlaceBlock {
            player_id,
            hand: InteractionHand::Main,
            position: block_pos_from_network(&transaction.position),
            face: block_face_from_i32(transaction.face),
            held_item: None,
        },
        ItemUseInventoryTransactionType::Use => CoreCommand::UseBlock {
            player_id,
            hand: InteractionHand::Main,
            position: block_pos_from_network(&transaction.position),
            face: block_face_from_i32(transaction.face),
            held_item: None,
        },
        ItemUseInventoryTransactionType::Destroy => CoreCommand::DigBlock {
            player_id,
            position: block_pos_from_network(&transaction.position),
            status: 2,
            face: block_face_from_i32(transaction.face),
        },
    }))
}

fn decode_inventory_transaction_frame(
    player_id: PlayerId,
    frame: &[u8],
) -> Result<Option<CoreCommand>, ProtocolError> {
    let mut frame_cursor = Cursor::new(frame);
    let packet_len = <u32 as ProtoCodecVAR>::deserialize(&mut frame_cursor)
        .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
    let packet_start = usize::try_from(frame_cursor.position())
        .expect("frame cursor position should fit into usize");
    let packet_end = packet_start
        .checked_add(usize::try_from(packet_len).expect("packet length should fit into usize"))
        .ok_or_else(|| protocol_error("bedrock packet length overflow"))?;
    let packet = frame
        .get(packet_start..packet_end)
        .ok_or_else(|| protocol_error("bedrock frame truncated"))?;

    let mut packet_cursor = Cursor::new(packet);
    let header = PacketHeader::deserialize(&mut packet_cursor)
        .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
    if header.packet_id != 30 {
        return Ok(None);
    }

    let _raw_id = <i32 as ProtoCodecVAR>::deserialize(&mut packet_cursor)
        .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
    let _: Vec<bedrockrs_proto::v662::packets::LegacySetItemSlotsEntry> =
        <Vec<bedrockrs_proto::v662::packets::LegacySetItemSlotsEntry> as ProtoCodec>::deserialize(
            &mut packet_cursor,
        )
        .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
    let transaction_type = ComplexInventoryTransactionType::deserialize(&mut packet_cursor)
        .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
    if !matches!(
        transaction_type,
        ComplexInventoryTransactionType::ItemUseTransaction
    ) {
        return Ok(None);
    }

    let transaction =
        bedrockrs_proto::v712::types::PackedItemUseLegacyInventoryTransaction::<V924>::deserialize(
            &mut packet_cursor,
        )
        .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
    decode_item_use_transaction(player_id, &transaction)
}

fn decode_item_stack_request_packet(
    player_id: PlayerId,
    requests: &[bedrockrs_proto::v662::packets::RequestsEntry<V924>],
) -> Result<Option<CoreCommand>, ProtocolError> {
    let Some(request) = requests.first() else {
        return Ok(None);
    };
    decode_request_actions(
        player_id,
        request_transaction(request.client_request_id),
        &request.actions,
    )
}

fn decode_auth_input_stack_request(
    player_id: PlayerId,
    request: &bedrockrs_proto::v766::packets::player_auth_input_packet::PerformItemStackRequestData<
        V924,
    >,
) -> Result<Option<CoreCommand>, ProtocolError> {
    let actions = request
        .actions
        .iter()
        .map(|entry| entry.action_type.clone())
        .collect::<Vec<_>>();
    decode_request_actions(
        player_id,
        request_transaction(request.client_request_id),
        &actions,
    )
}

fn decode_request_actions(
    player_id: PlayerId,
    transaction: mc_core::InventoryTransactionContext,
    actions: &[ItemStackRequestActionType<V924>],
) -> Result<Option<CoreCommand>, ProtocolError> {
    let Some(action) = actions.first() else {
        return Ok(None);
    };
    let translated = match action {
        ItemStackRequestActionType::Take {
            amount,
            source,
            destination,
        } => translate_take_action(transaction, source, destination, *amount),
        ItemStackRequestActionType::Place {
            amount,
            source,
            destination,
        } => translate_place_action(transaction, source, destination, *amount),
        ItemStackRequestActionType::Swap {
            source,
            destination,
        } => translate_swap_action(transaction, source, destination),
        ItemStackRequestActionType::Drop { amount, source, .. } => {
            translate_drop_action(transaction, source, *amount)
        }
        _ => None,
    };
    Ok(translated.map(
        |(transaction, target, button)| CoreCommand::InventoryClick {
            player_id,
            transaction,
            target,
            button,
            clicked_item: None,
        },
    ))
}
