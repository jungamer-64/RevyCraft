#![allow(clippy::multiple_crate_versions)]

#[path = "../../mc-plugin-storage-je-anvil-1_18_2/src/chunk_nbt.rs"]
mod chunk_nbt;
mod level;
#[path = "../../mc-plugin-storage-je-anvil-1_18_2/src/nbt.rs"]
mod nbt;
#[path = "../../mc-plugin-storage-je-anvil-1_18_2/src/playerdata.rs"]
mod playerdata;
#[path = "../../mc-plugin-storage-je-anvil-1_18_2/src/region.rs"]
mod region;

#[cfg(test)]
mod tests;

use self::nbt::NbtTag;
use mc_plugin_api::codec::storage::StorageDescriptor;
use mc_plugin_sdk_rust::capabilities::{build_tag_contains, storage_capabilities};
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use mc_plugin_sdk_rust::storage::RustStoragePlugin;
use mc_plugin_sdk_rust::{StorageCapability, StorageCapabilitySet, WorldSnapshot};
use mc_storage_common::StorageError;
use revy_voxel_semantic::{
    ChunkColumn, ChunkPos, ItemDataMap, ItemDataValue, ItemStack, OpaqueF32, OpaqueF64,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

pub(crate) const JE_26_1_DATA_VERSION: i32 = 4786;
pub(crate) const JE_26_1_MIN_SECTION_Y: i32 = -4;
pub(crate) const JE_26_1_MAX_SECTION_Y: i32 = 19;
pub(crate) const JE_1_18_2_DATA_VERSION: i32 = JE_26_1_DATA_VERSION;
pub(crate) const JE_1_18_2_MIN_SECTION_Y: i32 = JE_26_1_MIN_SECTION_Y;
pub(crate) const JE_1_18_2_MAX_SECTION_Y: i32 = JE_26_1_MAX_SECTION_Y;
const LEVEL_DAT: &str = "level.dat";
const PLAYERDATA_DIR: &str = "playerdata";
const REGION_DIR: &str = "region";

pub const JE_26_1_STORAGE_PROFILE_ID: &str = "je-anvil-26_1";
pub const JE_26_1_STORAGE_PLUGIN_ID: &str = "storage-je-anvil-26_1";

#[derive(Default)]
pub struct Je261StoragePlugin;

impl RustStoragePlugin for Je261StoragePlugin {
    fn descriptor(&self) -> StorageDescriptor {
        StorageDescriptor {
            storage_profile: JE_26_1_STORAGE_PROFILE_ID.into(),
        }
    }

    fn capability_set(&self) -> StorageCapabilitySet {
        storage_capabilities(&[StorageCapability::RuntimeReload])
    }

    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
        let level_path = world_dir.join(LEVEL_DAT);
        if !level_path.exists() {
            return Ok(None);
        }
        let meta = level::read_level_dat(&level_path)?;
        let (chunks, block_entities) = region::read_regions(&world_dir.join(REGION_DIR))?;
        let players = playerdata::read_playerdata(&world_dir.join(PLAYERDATA_DIR))?;
        Ok(Some(WorldSnapshot {
            meta,
            chunks,
            block_entities,
            players,
        }))
    }

    fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        fs::create_dir_all(world_dir)?;
        level::write_level_dat(&world_dir.join(LEVEL_DAT), &snapshot.meta)?;
        region::write_regions(
            &world_dir.join(REGION_DIR),
            &chunks_for_save(snapshot),
            &snapshot.block_entities,
        )?;
        playerdata::write_playerdata(&world_dir.join(PLAYERDATA_DIR), &snapshot.players)?;
        Ok(())
    }

    fn import_runtime_state(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        if build_tag_contains("reload-fail") {
            return Err(StorageError::Plugin(
                "storage plugin refused runtime state import".to_string(),
            ));
        }
        self.save_snapshot(world_dir, snapshot)
    }
}

fn chunks_for_save(snapshot: &WorldSnapshot) -> BTreeMap<ChunkPos, ChunkColumn> {
    if !snapshot.chunks.is_empty() {
        return snapshot.chunks.clone();
    }
    let chunk_pos = snapshot.meta.spawn.chunk_pos();
    let mut chunks = BTreeMap::new();
    chunks.insert(chunk_pos, mc_content_canonical::default_chunk(chunk_pos));
    chunks
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::storage(
    JE_26_1_STORAGE_PLUGIN_ID,
    "JE 26.1 Anvil Storage Plugin",
    JE_26_1_STORAGE_PROFILE_ID,
);

export_plugin!(storage, Je261StoragePlugin, MANIFEST);

pub(crate) fn item_stack_to_nbt(
    stack: &ItemStack,
    slot: Option<i8>,
) -> Result<NbtTag, StorageError> {
    if stack.damage != 0 {
        return Err(StorageError::InvalidData(
            "legacy item damage is not supported by je-anvil-26_1".to_string(),
        ));
    }
    let mut compound = BTreeMap::new();
    if let Some(slot) = slot {
        compound.insert("Slot".to_string(), NbtTag::Byte(slot));
    }
    compound.insert(
        "id".to_string(),
        NbtTag::String(stack.key.as_str().to_string()),
    );
    compound.insert(
        "count".to_string(),
        NbtTag::Byte(i8::try_from(stack.count).expect("count should fit into i8")),
    );
    if !stack.components.is_empty() {
        compound.insert(
            "components".to_string(),
            NbtTag::Compound(item_data_map_to_compound(&stack.components)?),
        );
    }
    for (key, value) in stack.extra.iter() {
        if matches!(
            key.as_str(),
            "id" | "count" | "components" | "Slot" | "Count" | "Damage" | "tag"
        ) {
            return Err(StorageError::InvalidData(format!(
                "reserved 26.1 item field `{key}` cannot be stored as extra data"
            )));
        }
        compound.insert(key.clone(), item_data_value_to_nbt(value)?);
    }
    Ok(NbtTag::Compound(compound))
}

pub(crate) fn item_stack_from_nbt(
    compound: &BTreeMap<String, NbtTag>,
) -> Result<ItemStack, StorageError> {
    validate_storage_item(compound, compound.contains_key("Slot"), "item")?;
    let key = nbt::string_field(compound, "id")?;
    let count = modern_item_count(compound)?;
    let components = match compound.get("components") {
        Some(NbtTag::Compound(components)) => item_data_map_from_compound(components),
        Some(_) => {
            return Err(StorageError::InvalidData(
                "26.1 item components field must be a compound".to_string(),
            ));
        }
        None => ItemDataMap::default(),
    };
    let extra = compound
        .iter()
        .filter(|(field, _)| !matches!(field.as_str(), "id" | "count" | "components" | "Slot"))
        .map(|(field, value)| (field.clone(), item_data_value_from_nbt(value)))
        .collect::<BTreeMap<_, _>>()
        .into();
    let mut stack = ItemStack::new(key, count, 0);
    stack.components = components;
    stack.extra = extra;
    Ok(stack)
}

pub(crate) fn item_count_or_zero(compound: &BTreeMap<String, NbtTag>) -> Result<u8, StorageError> {
    match compound.get("count") {
        Some(_) => modern_item_count(compound),
        None => Ok(0),
    }
}

pub(crate) fn validate_storage_item(
    compound: &BTreeMap<String, NbtTag>,
    allow_slot: bool,
    _context: &str,
) -> Result<(), StorageError> {
    if compound.contains_key("Count")
        || compound.contains_key("Damage")
        || compound.contains_key("tag")
    {
        return Err(StorageError::InvalidData(
            "legacy-style item fields are not supported in je-anvil-26_1".to_string(),
        ));
    }
    if !allow_slot && compound.contains_key("Slot") {
        return Err(StorageError::InvalidData(
            "unexpected item Slot field".to_string(),
        ));
    }
    Ok(())
}

fn modern_item_count(compound: &BTreeMap<String, NbtTag>) -> Result<u8, StorageError> {
    let raw = match compound.get("count") {
        Some(NbtTag::Byte(value)) => i32::from(*value),
        Some(NbtTag::Short(value)) => i32::from(*value),
        Some(NbtTag::Int(value)) => *value,
        Some(_) => {
            return Err(StorageError::InvalidData(
                "26.1 item count field had an unsupported type".to_string(),
            ));
        }
        None => {
            return Err(StorageError::InvalidData(
                "missing item count field".to_string(),
            ));
        }
    };
    u8::try_from(raw)
        .map_err(|_| StorageError::InvalidData("negative item count not supported".to_string()))
}

fn item_data_value_from_nbt(tag: &NbtTag) -> ItemDataValue {
    match tag {
        NbtTag::Byte(value) => ItemDataValue::Byte(*value),
        NbtTag::Short(value) => ItemDataValue::Short(*value),
        NbtTag::Int(value) => ItemDataValue::Int(*value),
        NbtTag::Long(value) => ItemDataValue::Long(*value),
        NbtTag::Float(value) => ItemDataValue::Float(OpaqueF32::from_f32(*value)),
        NbtTag::Double(value) => ItemDataValue::Double(OpaqueF64::from_f64(*value)),
        NbtTag::ByteArray(value) => {
            ItemDataValue::ByteArray(value.iter().map(|byte| byte.to_be_bytes()[0]).collect())
        }
        NbtTag::String(value) => ItemDataValue::String(value.clone()),
        NbtTag::List(_, value) => {
            ItemDataValue::List(value.iter().map(item_data_value_from_nbt).collect())
        }
        NbtTag::Compound(value) => ItemDataValue::Compound(
            value
                .iter()
                .map(|(key, value)| (key.clone(), item_data_value_from_nbt(value)))
                .collect(),
        ),
        NbtTag::IntArray(value) => ItemDataValue::IntArray(value.clone()),
        NbtTag::LongArray(value) => ItemDataValue::LongArray(value.clone()),
    }
}

fn item_data_map_from_compound(compound: &BTreeMap<String, NbtTag>) -> ItemDataMap {
    compound
        .iter()
        .map(|(key, value)| (key.clone(), item_data_value_from_nbt(value)))
        .collect::<BTreeMap<_, _>>()
        .into()
}

fn item_data_map_to_compound(data: &ItemDataMap) -> Result<BTreeMap<String, NbtTag>, StorageError> {
    data.iter()
        .map(|(key, value)| Ok((key.clone(), item_data_value_to_nbt(value)?)))
        .collect()
}

fn item_data_value_to_nbt(value: &ItemDataValue) -> Result<NbtTag, StorageError> {
    Ok(match value {
        ItemDataValue::Byte(value) => NbtTag::Byte(*value),
        ItemDataValue::Short(value) => NbtTag::Short(*value),
        ItemDataValue::Int(value) => NbtTag::Int(*value),
        ItemDataValue::Long(value) => NbtTag::Long(*value),
        ItemDataValue::Float(value) => NbtTag::Float(value.into_f32()),
        ItemDataValue::Double(value) => NbtTag::Double(value.into_f64()),
        ItemDataValue::ByteArray(value) => NbtTag::ByteArray(value.clone()),
        ItemDataValue::String(value) => NbtTag::String(value.clone()),
        ItemDataValue::List(value) => NbtTag::List(
            nbt_list_kind(value)?,
            value
                .iter()
                .map(item_data_value_to_nbt)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ItemDataValue::Compound(value) => NbtTag::Compound(
            value
                .iter()
                .map(|(key, value)| Ok((key.clone(), item_data_value_to_nbt(value)?)))
                .collect::<Result<BTreeMap<_, _>, StorageError>>()?,
        ),
        ItemDataValue::IntArray(value) => NbtTag::IntArray(value.clone()),
        ItemDataValue::LongArray(value) => NbtTag::LongArray(value.clone()),
    })
}

fn nbt_list_kind(values: &[ItemDataValue]) -> Result<u8, StorageError> {
    let Some(first) = values.first() else {
        return Ok(0);
    };
    let kind = nbt_tag_kind(first);
    if values
        .iter()
        .skip(1)
        .any(|value| nbt_tag_kind(value) != kind)
    {
        return Err(StorageError::InvalidData(
            "heterogeneous item component lists are not supported".to_string(),
        ));
    }
    Ok(kind)
}

fn nbt_tag_kind(value: &ItemDataValue) -> u8 {
    match value {
        ItemDataValue::Byte(_) => 1,
        ItemDataValue::Short(_) => 2,
        ItemDataValue::Int(_) => 3,
        ItemDataValue::Long(_) => 4,
        ItemDataValue::Float(_) => 5,
        ItemDataValue::Double(_) => 6,
        ItemDataValue::ByteArray(_) => 7,
        ItemDataValue::String(_) => 8,
        ItemDataValue::List(_) => 9,
        ItemDataValue::Compound(_) => 10,
        ItemDataValue::IntArray(_) => 11,
        ItemDataValue::LongArray(_) => 12,
    }
}
