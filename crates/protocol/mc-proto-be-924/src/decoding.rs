use crate::codec::decode_v924;
use bedrockrs_proto::V924;
use bedrockrs_proto::v662::enums::PlayerActionType;
use bedrockrs_proto::v662::packets::{
    ClientCacheStatusPacket, LoginPacket, MobEquipmentPacket, MovePlayerPacket, PlayerActionPacket,
    RequestNetworkSettingsPacket, ResourcePackClientResponsePacket,
};
use mc_core::{CoreCommand, PlayerId};
use mc_proto_be_common::__version_support::{
    login::parse_bedrock_login_payload,
    world::{block_face_from_i32, block_pos_from_network, protocol_error},
};
use mc_proto_common::{LoginRequest, ProtocolError};

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
            position: Some(mc_core::Vec3::new(
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
            action:
                PlayerActionType::StartDestroyBlock { .. }
                | PlayerActionType::StopDestroyBlock { .. }
                | PlayerActionType::CreativeDestroyBlock
                | PlayerActionType::PredictDestroyBlock { .. },
            block_position,
            face,
            ..
        }) => Ok(Some(CoreCommand::DigBlock {
            player_id,
            position: block_pos_from_network(&block_position),
            status: 2,
            face: block_face_from_i32(face),
        })),
        V924::ClientCacheStatusPacket(_) | V924::ResourcePackClientResponsePacket(_) => Ok(None),
        _ => Ok(None),
    }
}
