use super::nbt::{
    NbtTag, as_compound, byte_field, double_from_tag, float_field, float_from_tag, int_field,
    list_field, long_field, read_gzip_nbt, short_field, string_field, write_gzip_nbt,
};
use mc_core::{PlayerId, PlayerSnapshot};
use mc_model::{DimensionId, InventorySlot, PlayerInventory, Vec3};
use mc_proto_common::StorageError;
use mc_proto_je_common::__version_support::blocks::{legacy_item, semantic_item};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use uuid::Uuid;

const PLAYERDATA_OFFHAND_SLOT: i8 = -106;

pub(super) fn write_playerdata(
    playerdata_dir: &Path,
    players: &BTreeMap<PlayerId, PlayerSnapshot>,
) -> Result<(), StorageError> {
    fs::create_dir_all(playerdata_dir)?;
    for player in players.values() {
        let path = playerdata_dir.join(format!("{}.dat", player.id.0.hyphenated()));
        let root = player_to_nbt(player);
        write_gzip_nbt(&path, "", &root)?;
    }
    Ok(())
}

pub(super) fn read_playerdata(
    playerdata_dir: &Path,
) -> Result<BTreeMap<PlayerId, PlayerSnapshot>, StorageError> {
    let mut players = BTreeMap::new();
    if !playerdata_dir.exists() {
        return Ok(players);
    }
    for entry in fs::read_dir(playerdata_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("dat") {
            continue;
        }
        let player = player_from_nbt(&read_gzip_nbt(&path)?)?;
        players.insert(player.id, player);
    }
    Ok(players)
}

fn player_to_nbt(player: &PlayerSnapshot) -> NbtTag {
    let mut compound = BTreeMap::new();
    compound.insert(
        "Pos".to_string(),
        NbtTag::List(
            6,
            vec![
                NbtTag::Double(player.position.x),
                NbtTag::Double(player.position.y),
                NbtTag::Double(player.position.z),
            ],
        ),
    );
    compound.insert(
        "Rotation".to_string(),
        NbtTag::List(
            5,
            vec![NbtTag::Float(player.yaw), NbtTag::Float(player.pitch)],
        ),
    );
    let uuid_bytes = player.id.0.as_u128().to_be_bytes();
    let most = i64::from_be_bytes(uuid_bytes[0..8].try_into().expect("uuid most should fit"));
    let least = i64::from_be_bytes(uuid_bytes[8..16].try_into().expect("uuid least should fit"));
    compound.insert("UUIDMost".to_string(), NbtTag::Long(most));
    compound.insert("UUIDLeast".to_string(), NbtTag::Long(least));
    compound.insert("Dimension".to_string(), NbtTag::Int(0));
    compound.insert(
        "OnGround".to_string(),
        NbtTag::Byte(i8::from(player.on_ground)),
    );
    compound.insert("Health".to_string(), NbtTag::Float(player.health));
    compound.insert("foodLevel".to_string(), NbtTag::Int(i32::from(player.food)));
    compound.insert(
        "foodSaturationLevel".to_string(),
        NbtTag::Float(player.food_saturation),
    );
    compound.insert(
        "SelectedItemSlot".to_string(),
        NbtTag::Int(i32::from(player.selected_hotbar_slot)),
    );
    compound.insert(
        "Inventory".to_string(),
        NbtTag::List(10, inventory_to_nbt(&player.inventory)),
    );
    compound.insert("Name".to_string(), NbtTag::String(player.username.clone()));
    NbtTag::Compound(compound)
}

fn player_from_nbt(root: &NbtTag) -> Result<PlayerSnapshot, StorageError> {
    let compound = as_compound(root)?;
    let pos = list_field(compound, "Pos")?;
    let rotation = list_field(compound, "Rotation")?;
    let most = long_field(compound, "UUIDMost")?;
    let least = long_field(compound, "UUIDLeast")?;
    let mut uuid_bytes = [0_u8; 16];
    uuid_bytes[0..8].copy_from_slice(&most.to_be_bytes());
    uuid_bytes[8..16].copy_from_slice(&least.to_be_bytes());
    let inventory = compound
        .get("Inventory")
        .map(inventory_from_tag)
        .transpose()?
        .unwrap_or_else(mc_content_canonical::creative_starter_inventory);
    Ok(PlayerSnapshot {
        id: PlayerId(Uuid::from_u128(u128::from_be_bytes(uuid_bytes))),
        username: string_field(compound, "Name").unwrap_or_else(|_| "player".to_string()),
        position: Vec3::new(
            double_from_tag(&pos[0])?,
            double_from_tag(&pos[1])?,
            double_from_tag(&pos[2])?,
        ),
        yaw: float_from_tag(&rotation[0])?,
        pitch: float_from_tag(&rotation[1])?,
        on_ground: byte_field(compound, "OnGround").unwrap_or(1) != 0,
        dimension: DimensionId::Overworld,
        health: float_field(compound, "Health").unwrap_or(20.0),
        food: i16::try_from(int_field(compound, "foodLevel").unwrap_or(20)).unwrap_or(20),
        food_saturation: float_field(compound, "foodSaturationLevel").unwrap_or(5.0),
        inventory,
        selected_hotbar_slot: u8::try_from(int_field(compound, "SelectedItemSlot").unwrap_or(0))
            .unwrap_or(0)
            .min(8),
    })
}

fn inventory_to_nbt(inventory: &PlayerInventory) -> Vec<NbtTag> {
    let mut entries: Vec<_> = inventory
        .slots
        .iter()
        .enumerate()
        .filter_map(
            |(window_slot, stack): (usize, &Option<mc_model::ItemStack>)| {
                let stack = stack.as_ref()?;
                let (item_id, damage) = legacy_item(stack)?;
                let nbt_slot = window_slot_to_playerdata_slot(
                    u8::try_from(window_slot).expect("window slot should fit into u8"),
                )?;
                let mut compound = BTreeMap::new();
                compound.insert("Slot".to_string(), NbtTag::Byte(nbt_slot));
                compound.insert("id".to_string(), NbtTag::Short(item_id));
                compound.insert(
                    "Damage".to_string(),
                    NbtTag::Short(i16::from_be_bytes(damage.to_be_bytes())),
                );
                compound.insert(
                    "Count".to_string(),
                    NbtTag::Byte(i8::try_from(stack.count).expect("count should fit into i8")),
                );
                Some(NbtTag::Compound(compound))
            },
        )
        .collect();
    if let Some(stack) = inventory.offhand.as_ref()
        && let Some((item_id, damage)) = legacy_item(stack)
    {
        let mut compound = BTreeMap::new();
        compound.insert("Slot".to_string(), NbtTag::Byte(PLAYERDATA_OFFHAND_SLOT));
        compound.insert("id".to_string(), NbtTag::Short(item_id));
        compound.insert(
            "Damage".to_string(),
            NbtTag::Short(i16::from_be_bytes(damage.to_be_bytes())),
        );
        compound.insert(
            "Count".to_string(),
            NbtTag::Byte(i8::try_from(stack.count).expect("count should fit into i8")),
        );
        entries.push(NbtTag::Compound(compound));
    }
    entries
}

fn inventory_from_tag(tag: &NbtTag) -> Result<PlayerInventory, StorageError> {
    let mut inventory = PlayerInventory::new_empty();
    let NbtTag::List(_, entries) = tag else {
        return Err(StorageError::InvalidData(
            "expected inventory list".to_string(),
        ));
    };
    for entry in entries {
        let compound = as_compound(entry)?;
        let slot = byte_field(compound, "Slot")?;
        let count = byte_field(compound, "Count").unwrap_or(0);
        if count <= 0 {
            continue;
        }
        let item_id = short_field(compound, "id")?;
        let damage = u16::from_be_bytes(short_field(compound, "Damage").unwrap_or(0).to_be_bytes());
        let stack = semantic_item(item_id, damage, count.cast_unsigned());
        if stack.key.as_str() == "minecraft:unsupported" {
            continue;
        }
        if slot == PLAYERDATA_OFFHAND_SLOT {
            let _ = inventory.set_slot(InventorySlot::Offhand, Some(stack));
            continue;
        }
        let Some(window_slot) = playerdata_slot_to_window_slot(slot) else {
            continue;
        };
        let _ = inventory.set(window_slot, Some(stack));
    }
    Ok(inventory)
}

fn window_slot_to_playerdata_slot(window_slot: u8) -> Option<i8> {
    match window_slot {
        9..=35 => Some(i8::try_from(window_slot).expect("main inventory slot should fit into i8")),
        36..=44 => Some(i8::try_from(window_slot - 36).expect("hotbar slot should fit into i8")),
        _ => None,
    }
}

fn playerdata_slot_to_window_slot(slot: i8) -> Option<u8> {
    match slot {
        0..=8 => Some(36 + u8::try_from(slot).expect("hotbar slot should fit into u8")),
        9..=35 => Some(u8::try_from(slot).expect("main inventory slot should fit into u8")),
        _ => None,
    }
}
