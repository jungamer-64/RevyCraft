use super::nbt::{
    NbtTag, as_compound, byte_field, double_from_tag, float_field, float_from_tag, int_field,
    list_field, read_gzip_nbt, string_field, write_gzip_nbt,
};
use mc_core::{PlayerId, PlayerSnapshot};
use mc_model::{DimensionId, InventorySlot, ItemStack, PlayerInventory, Vec3};
use mc_proto_common::StorageError;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use uuid::Uuid;

const PLAYERDATA_OFFHAND_SLOT: i8 = 40;

pub(super) fn write_playerdata(
    playerdata_dir: &Path,
    players: &BTreeMap<PlayerId, PlayerSnapshot>,
) -> Result<(), StorageError> {
    fs::create_dir_all(playerdata_dir)?;
    for player in players.values() {
        let path = playerdata_dir.join(format!("{}.dat", player.id.0.hyphenated()));
        let root = if path.exists() {
            merge_player_nbt(read_gzip_nbt(&path)?, player)?
        } else {
            player_to_nbt(player)
        };
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
        let player = player_from_nbt(&read_gzip_nbt(&path)?, &path)?;
        players.insert(player.id, player);
    }
    Ok(players)
}

fn merge_player_nbt(root: NbtTag, player: &PlayerSnapshot) -> Result<NbtTag, StorageError> {
    let mut compound = as_compound(&root)?.clone();
    let player_root = as_compound(&player_to_nbt(player))?.clone();
    for (key, value) in player_root {
        compound.insert(key, value);
    }
    Ok(NbtTag::Compound(compound))
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
    compound.insert(
        "UUID".to_string(),
        NbtTag::IntArray(uuid_to_int_array(player.id.0)),
    );
    compound.insert(
        "Dimension".to_string(),
        NbtTag::String("minecraft:overworld".to_string()),
    );
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

fn player_from_nbt(root: &NbtTag, path: &Path) -> Result<PlayerSnapshot, StorageError> {
    let compound = as_compound(root)?;
    ensure_overworld(compound)?;
    let pos = list_field(compound, "Pos")?;
    let rotation = list_field(compound, "Rotation")?;
    if pos.len() != 3 || rotation.len() != 2 {
        return Err(StorageError::InvalidData(
            "player pos/rotation list had an unexpected length".to_string(),
        ));
    }
    let inventory = compound
        .get("Inventory")
        .map(inventory_from_tag)
        .transpose()?
        .unwrap_or_else(PlayerInventory::new_empty);
    Ok(PlayerSnapshot {
        id: PlayerId(player_uuid(compound, path)?),
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
    let mut entries = inventory
        .slots
        .iter()
        .enumerate()
        .filter_map(|(window_slot, stack): (usize, &Option<ItemStack>)| {
            let stack = stack.as_ref()?;
            let nbt_slot = window_slot_to_playerdata_slot(
                u8::try_from(window_slot).expect("window slot should fit into u8"),
            )?;
            Some(item_stack_to_nbt(stack, nbt_slot))
        })
        .collect::<Vec<_>>();
    if let Some(stack) = inventory.offhand.as_ref() {
        entries.push(item_stack_to_nbt(stack, PLAYERDATA_OFFHAND_SLOT));
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
        validate_item_keys(compound, true)?;
        if compound.contains_key("tag") {
            return Err(StorageError::InvalidData(
                "player inventory item tag is not supported".to_string(),
            ));
        }
        let slot = byte_field(compound, "Slot")?;
        let count = byte_field(compound, "Count").unwrap_or(0);
        if count <= 0 {
            continue;
        }
        let stack = item_stack_from_nbt(compound)?;
        if slot == PLAYERDATA_OFFHAND_SLOT {
            let _ = inventory.set_slot(InventorySlot::Offhand, Some(stack));
            continue;
        }
        let Some(window_slot) = playerdata_slot_to_window_slot(slot) else {
            return Err(StorageError::InvalidData(format!(
                "unsupported player inventory slot {slot}"
            )));
        };
        let _ = inventory.set(window_slot, Some(stack));
    }
    Ok(inventory)
}

fn item_stack_to_nbt(stack: &ItemStack, slot: i8) -> NbtTag {
    let mut compound = BTreeMap::new();
    compound.insert("Slot".to_string(), NbtTag::Byte(slot));
    compound.insert(
        "id".to_string(),
        NbtTag::String(stack.key.as_str().to_string()),
    );
    compound.insert(
        "Count".to_string(),
        NbtTag::Byte(i8::try_from(stack.count).expect("count should fit into i8")),
    );
    if stack.damage != 0 {
        compound.insert("Damage".to_string(), NbtTag::Int(i32::from(stack.damage)));
    }
    NbtTag::Compound(compound)
}

fn item_stack_from_nbt(compound: &BTreeMap<String, NbtTag>) -> Result<ItemStack, StorageError> {
    let key = string_field(compound, "id")?;
    let count = u8::try_from(byte_field(compound, "Count")?)
        .map_err(|_| StorageError::InvalidData("negative item count not supported".to_string()))?;
    let damage = match compound.get("Damage") {
        Some(NbtTag::Short(value)) => u16::try_from(*value).map_err(|_| {
            StorageError::InvalidData("negative item damage not supported".to_string())
        })?,
        Some(NbtTag::Int(value)) => u16::try_from(*value).map_err(|_| {
            StorageError::InvalidData("item damage did not fit into u16".to_string())
        })?,
        Some(_) => {
            return Err(StorageError::InvalidData(
                "item Damage field had an unsupported type".to_string(),
            ));
        }
        None => 0,
    };
    Ok(ItemStack::new(key, count, damage))
}

fn validate_item_keys(
    compound: &BTreeMap<String, NbtTag>,
    allow_slot: bool,
) -> Result<(), StorageError> {
    for key in compound.keys() {
        let allowed = matches!(key.as_str(), "id" | "Count" | "Damage")
            || (allow_slot && key == "Slot")
            || key == "tag";
        if !allowed {
            return Err(StorageError::InvalidData(format!(
                "unsupported item field `{key}`"
            )));
        }
    }
    Ok(())
}

fn ensure_overworld(compound: &BTreeMap<String, NbtTag>) -> Result<(), StorageError> {
    match compound.get("Dimension") {
        Some(NbtTag::String(value)) if value == "minecraft:overworld" => Ok(()),
        Some(NbtTag::Int(0)) => Ok(()),
        Some(other) => Err(StorageError::InvalidData(format!(
            "only overworld playerdata is supported, got {other:?}"
        ))),
        None => Ok(()),
    }
}

fn player_uuid(compound: &BTreeMap<String, NbtTag>, path: &Path) -> Result<Uuid, StorageError> {
    if let Some(NbtTag::IntArray(values)) = compound.get("UUID") {
        if values.len() != 4 {
            return Err(StorageError::InvalidData(
                "player UUID int array must contain 4 ints".to_string(),
            ));
        }
        let mut bytes = [0_u8; 16];
        for (index, value) in values.iter().enumerate() {
            let start = index * 4;
            bytes[start..start + 4].copy_from_slice(&value.to_be_bytes());
        }
        return Ok(Uuid::from_bytes(bytes));
    }
    let file_stem = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| {
            StorageError::InvalidData("playerdata filename was not valid utf-8".to_string())
        })?;
    Uuid::parse_str(file_stem).map_err(|error| {
        StorageError::InvalidData(format!("invalid player uuid `{file_stem}`: {error}"))
    })
}

fn uuid_to_int_array(uuid: Uuid) -> Vec<i32> {
    uuid.as_bytes()
        .chunks_exact(4)
        .map(|chunk| i32::from_be_bytes(chunk.try_into().expect("uuid chunk should fit")))
        .collect()
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
