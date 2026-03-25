use crate::codec::encode_v924;
use crate::runtime_ids::block_runtime_id;
use bedrockrs_proto::V924;
use bedrockrs_proto::v662::enums::{ContainerEnumName, ContainerID, ContainerType};
use bedrockrs_proto::v662::types::{
    ActorUniqueID, NetworkItemInstanceDescriptor, NetworkItemStackDescriptor,
};
use bedrockrs_proto::v685::packets::ContainerClosePacket;
use bedrockrs_proto::v712::types::ItemStackRequestSlotInfo;
use bedrockrs_proto::v729::types::FullContainerName;
use bedrockrs_proto::v748::packets::{InventoryContentPacket, InventorySlotPacket};
use bedrockrs_proto::v776::packets::{CreativeContentPacket, CreativeItemData};
use bedrockrs_proto_core::{ProtoCodec, ProtoCodecLE, ProtoCodecVAR};
use mc_core::{
    InventoryClickButton, InventoryClickTarget, InventoryContainer, InventorySlot,
    InventoryTransactionContext, InventoryWindowContents, ItemStack,
};
use mc_proto_common::ProtocolError;
use std::io::Cursor;

const PLAYER_STORAGE_SLOT_COUNT: u32 = 36;
const PLAYER_MAIN_INVENTORY_SLOT_COUNT: u32 = 27;
const OFFHAND_SLOT: u32 = 0;
const PLAYER_INVENTORY_ID: u32 = 0;
const ACTIVE_CONTAINER_INVENTORY_ID: u32 = 1;
const OFFHAND_INVENTORY_ID: u32 = 119;
const EMPTY_USER_DATA: &str = "";
const EMPTY_BLOCK_RUNTIME_ID: i32 = 0;
const NO_DYNAMIC_CONTAINER_ID: Option<i32> = None;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestSlot {
    Slot(InventorySlot),
    Cursor,
}

pub(crate) fn encode_creative_content_packet() -> Result<Vec<Vec<u8>>, ProtocolError> {
    let contents = supported_creative_items()
        .into_iter()
        .enumerate()
        .map(|(index, stack)| {
            Ok(CreativeItemData {
                creative_net_id: u32::try_from(index + 1).expect("creative ids should fit in u32"),
                item_instance: network_item_instance_descriptor(Some(&stack))?,
                group_id: 0,
            })
        })
        .collect::<Result<Vec<_>, ProtocolError>>()?;
    Ok(vec![encode_v924(&[V924::CreativeContentPacket(
        CreativeContentPacket {
            groups: Vec::new(),
            contents,
        },
    )])?])
}

pub(crate) fn encode_inventory_contents_packets(
    window_id: u8,
    container: InventoryContainer,
    contents: &InventoryWindowContents,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    let mut packets = Vec::new();
    if container != InventoryContainer::Player && window_id != 0 {
        let local_slots = contents
            .container_slots
            .iter()
            .map(|stack| network_item_stack_descriptor(stack.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;
        packets.push(V924::InventoryContentPacket(InventoryContentPacket {
            inventory_id: ACTIVE_CONTAINER_INVENTORY_ID,
            slots: local_slots,
            container_name_data: full_container_name(
                active_container_enum_name(container),
                NO_DYNAMIC_CONTAINER_ID,
            )?,
            storage_item: network_item_stack_descriptor(None)?,
        }));
    }

    let storage_slots = (0..PLAYER_STORAGE_SLOT_COUNT)
        .map(|slot_index| {
            let slot = player_storage_slot(slot_index)
                .expect("player storage slot should resolve for bedrock encoding");
            network_item_stack_descriptor(contents.get_slot(slot))
        })
        .collect::<Result<Vec<_>, _>>()?;
    packets.push(V924::InventoryContentPacket(InventoryContentPacket {
        inventory_id: PLAYER_INVENTORY_ID,
        slots: storage_slots,
        container_name_data: full_container_name(
            ContainerEnumName::CombinedHotbarAndInventoryContainer,
            NO_DYNAMIC_CONTAINER_ID,
        )?,
        storage_item: network_item_stack_descriptor(None)?,
    }));
    packets.push(V924::InventoryContentPacket(InventoryContentPacket {
        inventory_id: OFFHAND_INVENTORY_ID,
        slots: vec![network_item_stack_descriptor(
            contents.get_slot(InventorySlot::Offhand),
        )?],
        container_name_data: full_container_name(
            ContainerEnumName::OffhandContainer,
            NO_DYNAMIC_CONTAINER_ID,
        )?,
        storage_item: network_item_stack_descriptor(None)?,
    }));

    Ok(vec![encode_v924(&packets)?])
}

pub(crate) fn encode_inventory_slot_changed_packets(
    _window_id: u8,
    container: InventoryContainer,
    slot: InventorySlot,
    stack: Option<&ItemStack>,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    let Some((container_id, network_slot, container_name_data)) =
        encode_slot_location(container, slot)?
    else {
        return Ok(Vec::new());
    };
    Ok(vec![encode_v924(&[V924::InventorySlotPacket(
        InventorySlotPacket {
            container_id,
            slot: network_slot,
            container_name_data,
            storage_item: network_item_stack_descriptor(None)?,
            item: network_item_stack_descriptor(stack)?,
        },
    )])?])
}

pub(crate) fn encode_selected_hotbar_slot_changed_packets(
    slot: u8,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    Ok(vec![encode_v924(&[V924::PlayerHotbarPacket(
        bedrockrs_proto::v662::packets::PlayerHotbarPacket {
            selected_slot: u32::from(slot),
            container_id: ContainerID::Inventory,
            should_select_slot: true,
        },
    )])?])
}

pub(crate) fn encode_container_opened_packets(
    window_id: u8,
    container: InventoryContainer,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    if window_id == 0 || container == InventoryContainer::Player {
        return Ok(Vec::new());
    }
    Ok(vec![encode_v924(&[V924::ContainerOpenPacket(
        bedrockrs_proto::v662::packets::ContainerOpenPacket {
            container_id: ContainerID::First,
            container_type: container_type(container),
            position: bedrockrs_proto::v662::types::NetworkBlockPosition { x: 0, y: 0, z: 0 },
            target_actor_id: ActorUniqueID(0),
        },
    )])?])
}

pub(crate) fn encode_container_closed_packets(
    window_id: u8,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    if window_id == 0 {
        return Ok(Vec::new());
    }
    Ok(vec![encode_v924(&[V924::ContainerClosePacket(
        ContainerClosePacket {
            container_id: ContainerID::First,
            container_type: ContainerType::None,
            server_initiated_close: true,
        },
    )])?])
}

pub(crate) fn encode_container_property_changed_packets(
    window_id: u8,
    property_id: u8,
    value: i16,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    if window_id == 0 {
        return Ok(Vec::new());
    }
    Ok(vec![encode_v924(&[V924::ContainerSetDataPacket(
        bedrockrs_proto::v662::packets::ContainerSetDataPacket {
            container_id: ContainerID::First,
            id: i32::from(property_id),
            value: i32::from(value),
        },
    )])?])
}

pub(crate) fn decode_request_slot(slot: &ItemStackRequestSlotInfo<V924>) -> Option<RequestSlot> {
    let slot_index = u8::try_from(slot.slot).ok()?;
    match decode_container_enum_name(&slot.container_name).ok()? {
        ContainerEnumName::CursorContainer => Some(RequestSlot::Cursor),
        ContainerEnumName::InventoryContainer => {
            Some(RequestSlot::Slot(InventorySlot::MainInventory(slot_index)))
        }
        ContainerEnumName::HotbarContainer => {
            Some(RequestSlot::Slot(InventorySlot::Hotbar(slot_index)))
        }
        ContainerEnumName::CombinedHotbarAndInventoryContainer => {
            player_storage_slot(u32::from(slot_index)).map(RequestSlot::Slot)
        }
        ContainerEnumName::OffhandContainer if slot_index == 0 => {
            Some(RequestSlot::Slot(InventorySlot::Offhand))
        }
        ContainerEnumName::CraftingOutputPreviewContainer if slot_index == 0 => {
            Some(RequestSlot::Slot(InventorySlot::Container(0)))
        }
        ContainerEnumName::CraftingInputContainer if slot_index < 9 => {
            Some(RequestSlot::Slot(InventorySlot::Container(slot_index + 1)))
        }
        ContainerEnumName::LevelEntityContainer => {
            Some(RequestSlot::Slot(InventorySlot::Container(slot_index)))
        }
        ContainerEnumName::FurnaceIngredientContainer if slot_index == 0 => {
            Some(RequestSlot::Slot(InventorySlot::Container(0)))
        }
        ContainerEnumName::FurnaceFuelContainer if slot_index == 0 => {
            Some(RequestSlot::Slot(InventorySlot::Container(1)))
        }
        ContainerEnumName::FurnaceResultContainer if slot_index == 0 => {
            Some(RequestSlot::Slot(InventorySlot::Container(2)))
        }
        _ => None,
    }
}

pub(crate) fn translate_take_action(
    transaction: InventoryTransactionContext,
    source: &ItemStackRequestSlotInfo<V924>,
    destination: &ItemStackRequestSlotInfo<V924>,
    amount: i8,
) -> Option<(
    InventoryTransactionContext,
    InventoryClickTarget,
    InventoryClickButton,
)> {
    let source = decode_request_slot(source)?;
    let destination = decode_request_slot(destination)?;
    match (source, destination) {
        (RequestSlot::Slot(slot), RequestSlot::Cursor) => Some((
            transaction,
            InventoryClickTarget::Slot(slot),
            amount_to_button(amount),
        )),
        _ => None,
    }
}

pub(crate) fn translate_place_action(
    transaction: InventoryTransactionContext,
    source: &ItemStackRequestSlotInfo<V924>,
    destination: &ItemStackRequestSlotInfo<V924>,
    amount: i8,
) -> Option<(
    InventoryTransactionContext,
    InventoryClickTarget,
    InventoryClickButton,
)> {
    let source = decode_request_slot(source)?;
    let destination = decode_request_slot(destination)?;
    match (source, destination) {
        (RequestSlot::Cursor, RequestSlot::Slot(slot)) => Some((
            transaction,
            InventoryClickTarget::Slot(slot),
            amount_to_button(amount),
        )),
        _ => None,
    }
}

pub(crate) fn translate_swap_action(
    transaction: InventoryTransactionContext,
    source: &ItemStackRequestSlotInfo<V924>,
    destination: &ItemStackRequestSlotInfo<V924>,
) -> Option<(
    InventoryTransactionContext,
    InventoryClickTarget,
    InventoryClickButton,
)> {
    let source = decode_request_slot(source)?;
    let destination = decode_request_slot(destination)?;
    match (source, destination) {
        (RequestSlot::Cursor, RequestSlot::Slot(slot))
        | (RequestSlot::Slot(slot), RequestSlot::Cursor) => Some((
            transaction,
            InventoryClickTarget::Slot(slot),
            InventoryClickButton::Left,
        )),
        _ => None,
    }
}

pub(crate) fn translate_drop_action(
    transaction: InventoryTransactionContext,
    source: &ItemStackRequestSlotInfo<V924>,
    amount: i8,
) -> Option<(
    InventoryTransactionContext,
    InventoryClickTarget,
    InventoryClickButton,
)> {
    match decode_request_slot(source)? {
        RequestSlot::Cursor => Some((
            transaction,
            InventoryClickTarget::Outside,
            amount_to_button(amount),
        )),
        RequestSlot::Slot(_) => None,
    }
}

pub(crate) fn request_transaction(client_request_id: u32) -> InventoryTransactionContext {
    InventoryTransactionContext {
        window_id: 0,
        action_number: i16::try_from(client_request_id.min(i16::MAX as u32))
            .expect("bounded request id should fit into i16"),
    }
}

fn supported_creative_items() -> Vec<ItemStack> {
    [
        ("minecraft:stone", 64),
        ("minecraft:dirt", 64),
        ("minecraft:grass_block", 64),
        ("minecraft:cobblestone", 64),
        ("minecraft:oak_planks", 64),
        ("minecraft:sand", 64),
        ("minecraft:sandstone", 64),
        ("minecraft:glass", 64),
        ("minecraft:bricks", 64),
        ("minecraft:oak_log", 64),
        ("minecraft:stick", 64),
        ("minecraft:chest", 64),
    ]
    .into_iter()
    .map(|(key, count)| ItemStack::new(key, count, 0))
    .collect()
}

fn amount_to_button(amount: i8) -> InventoryClickButton {
    if amount <= 1 {
        InventoryClickButton::Right
    } else {
        InventoryClickButton::Left
    }
}

fn encode_slot_location(
    container: InventoryContainer,
    slot: InventorySlot,
) -> Result<Option<(u32, u32, FullContainerName<V924>)>, ProtocolError> {
    match slot {
        InventorySlot::MainInventory(index) => Ok(Some((
            PLAYER_INVENTORY_ID,
            u32::from(index),
            full_container_name(
                ContainerEnumName::CombinedHotbarAndInventoryContainer,
                NO_DYNAMIC_CONTAINER_ID,
            )?,
        ))),
        InventorySlot::Hotbar(index) => Ok(Some((
            PLAYER_INVENTORY_ID,
            PLAYER_MAIN_INVENTORY_SLOT_COUNT + u32::from(index),
            full_container_name(
                ContainerEnumName::CombinedHotbarAndInventoryContainer,
                NO_DYNAMIC_CONTAINER_ID,
            )?,
        ))),
        InventorySlot::Offhand => Ok(Some((
            OFFHAND_INVENTORY_ID,
            OFFHAND_SLOT,
            full_container_name(ContainerEnumName::OffhandContainer, NO_DYNAMIC_CONTAINER_ID)?,
        ))),
        InventorySlot::Container(index) if container != InventoryContainer::Player => {
            let (slot_index, container_name) = active_container_slot_location(container, index)?;
            Ok(Some((
                ACTIVE_CONTAINER_INVENTORY_ID,
                slot_index,
                full_container_name(container_name, NO_DYNAMIC_CONTAINER_ID)?,
            )))
        }
        InventorySlot::Auxiliary(_) | InventorySlot::Container(_) => Ok(None),
    }
}

fn active_container_slot_location(
    container: InventoryContainer,
    slot: u8,
) -> Result<(u32, ContainerEnumName), ProtocolError> {
    match container {
        InventoryContainer::Player => Err(ProtocolError::Plugin(
            "player container should not use active container slot mapping".to_string(),
        )),
        InventoryContainer::CraftingTable => match slot {
            0 => Ok((0, ContainerEnumName::CraftingOutputPreviewContainer)),
            1..=9 => Ok((
                u32::from(slot - 1),
                ContainerEnumName::CraftingInputContainer,
            )),
            _ => Err(ProtocolError::Plugin(format!(
                "unsupported crafting-table slot for bedrock encoding: {slot}"
            ))),
        },
        InventoryContainer::Chest => Ok((u32::from(slot), ContainerEnumName::LevelEntityContainer)),
        InventoryContainer::Furnace => match slot {
            0 => Ok((0, ContainerEnumName::FurnaceIngredientContainer)),
            1 => Ok((0, ContainerEnumName::FurnaceFuelContainer)),
            2 => Ok((0, ContainerEnumName::FurnaceResultContainer)),
            _ => Err(ProtocolError::Plugin(format!(
                "unsupported furnace slot for bedrock encoding: {slot}"
            ))),
        },
    }
}

fn active_container_enum_name(container: InventoryContainer) -> ContainerEnumName {
    match container {
        InventoryContainer::Player => ContainerEnumName::CombinedHotbarAndInventoryContainer,
        InventoryContainer::CraftingTable
        | InventoryContainer::Chest
        | InventoryContainer::Furnace => ContainerEnumName::LevelEntityContainer,
    }
}

fn player_storage_slot(slot_index: u32) -> Option<InventorySlot> {
    match slot_index {
        0..=26 => Some(InventorySlot::MainInventory(
            u8::try_from(slot_index).expect("bedrock inventory slot should fit into u8"),
        )),
        27..=35 => Some(InventorySlot::Hotbar(
            u8::try_from(slot_index - PLAYER_MAIN_INVENTORY_SLOT_COUNT)
                .expect("bedrock hotbar slot should fit into u8"),
        )),
        _ => None,
    }
}

fn container_type(container: InventoryContainer) -> ContainerType {
    match container {
        InventoryContainer::Player => ContainerType::Inventory,
        InventoryContainer::CraftingTable => ContainerType::Workbench,
        InventoryContainer::Chest => ContainerType::Container,
        InventoryContainer::Furnace => ContainerType::Furnace,
    }
}

fn network_item_stack_descriptor(
    stack: Option<&ItemStack>,
) -> Result<NetworkItemStackDescriptor, ProtocolError> {
    decode_descriptor(build_item_stack_descriptor_bytes(stack)?)
}

fn network_item_instance_descriptor(
    stack: Option<&ItemStack>,
) -> Result<NetworkItemInstanceDescriptor, ProtocolError> {
    decode_instance_descriptor(build_item_instance_descriptor_bytes(stack)?)
}

fn build_item_stack_descriptor_bytes(stack: Option<&ItemStack>) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = Vec::new();
    match stack {
        None => <i32 as ProtoCodecVAR>::serialize(&0, &mut bytes)
            .map_err(|error| ProtocolError::Plugin(error.to_string()))?,
        Some(stack) => {
            let item = bedrock_item_encoding(stack)?;
            <i32 as ProtoCodecVAR>::serialize(&item.item_id, &mut bytes)
                .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
            <u16 as ProtoCodecLE>::serialize(&u16::from(stack.count), &mut bytes)
                .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
            <u32 as ProtoCodecVAR>::serialize(&u32::from(stack.damage), &mut bytes)
                .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
            <Option<i32> as ProtoCodecVAR>::serialize(&None, &mut bytes)
                .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
            <i32 as ProtoCodecVAR>::serialize(&item.block_runtime_id, &mut bytes)
                .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
            EMPTY_USER_DATA
                .to_string()
                .serialize(&mut bytes)
                .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
        }
    }
    Ok(bytes)
}

fn build_item_instance_descriptor_bytes(
    stack: Option<&ItemStack>,
) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = Vec::new();
    match stack {
        None => <i32 as ProtoCodecVAR>::serialize(&0, &mut bytes)
            .map_err(|error| ProtocolError::Plugin(error.to_string()))?,
        Some(stack) => {
            let item = bedrock_item_encoding(stack)?;
            <i32 as ProtoCodecVAR>::serialize(&item.item_id, &mut bytes)
                .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
            <u16 as ProtoCodecLE>::serialize(&u16::from(stack.count), &mut bytes)
                .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
            <u32 as ProtoCodecVAR>::serialize(&u32::from(stack.damage), &mut bytes)
                .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
            <i32 as ProtoCodecVAR>::serialize(&item.block_runtime_id, &mut bytes)
                .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
            EMPTY_USER_DATA
                .to_string()
                .serialize(&mut bytes)
                .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
        }
    }
    Ok(bytes)
}

fn decode_descriptor(bytes: Vec<u8>) -> Result<NetworkItemStackDescriptor, ProtocolError> {
    NetworkItemStackDescriptor::deserialize(&mut Cursor::new(bytes))
        .map_err(|error| ProtocolError::Plugin(error.to_string()))
}

fn decode_instance_descriptor(
    bytes: Vec<u8>,
) -> Result<NetworkItemInstanceDescriptor, ProtocolError> {
    NetworkItemInstanceDescriptor::deserialize(&mut Cursor::new(bytes))
        .map_err(|error| ProtocolError::Plugin(error.to_string()))
}

struct BedrockItemEncoding {
    item_id: i32,
    block_runtime_id: i32,
}

fn full_container_name(
    container: ContainerEnumName,
    dynamic_id: Option<i32>,
) -> Result<FullContainerName<V924>, ProtocolError> {
    let mut bytes = Vec::new();
    container
        .serialize(&mut bytes)
        .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
    <Option<i32> as ProtoCodecLE>::serialize(&dynamic_id, &mut bytes)
        .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
    FullContainerName::deserialize(&mut Cursor::new(bytes))
        .map_err(|error| ProtocolError::Plugin(error.to_string()))
}

fn decode_container_enum_name(
    container_name: &FullContainerName<V924>,
) -> Result<ContainerEnumName, ProtocolError> {
    let mut bytes = Vec::new();
    container_name
        .serialize(&mut bytes)
        .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
    ContainerEnumName::deserialize(&mut Cursor::new(bytes))
        .map_err(|error| ProtocolError::Plugin(error.to_string()))
}

fn bedrock_item_encoding(stack: &ItemStack) -> Result<BedrockItemEncoding, ProtocolError> {
    let block_runtime_id =
        mc_core::catalog::placeable_block_state_from_item_key(stack.key.as_str())
            .map(|block| {
                i32::try_from(block_runtime_id(&block))
                    .expect("bedrock block runtime id should fit into i32")
            })
            .unwrap_or(EMPTY_BLOCK_RUNTIME_ID);
    let item_id = match stack.key.as_str() {
        "minecraft:stone" => 1,
        "minecraft:grass_block" => 2,
        "minecraft:dirt" => 3,
        "minecraft:cobblestone" => 4,
        "minecraft:oak_planks" => 5,
        "minecraft:bedrock" => 7,
        "minecraft:sand" => 12,
        "minecraft:glass" => 20,
        "minecraft:sandstone" => 24,
        "minecraft:bricks" => 45,
        "minecraft:chest" => 54,
        "minecraft:oak_log" => 17,
        "minecraft:stick" => 280,
        _ => {
            return Err(ProtocolError::Plugin(format!(
                "unsupported bedrock inventory item: {}",
                stack.key.as_str()
            )));
        }
    };
    Ok(BedrockItemEncoding {
        item_id,
        block_runtime_id,
    })
}
