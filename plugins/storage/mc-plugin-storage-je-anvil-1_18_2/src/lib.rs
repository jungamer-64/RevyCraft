#![allow(clippy::multiple_crate_versions)]

mod chunk_nbt;
mod level;
mod nbt;
mod playerdata;
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
use revy_voxel_model::{ChunkColumn, ChunkPos, ItemStack};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

pub(crate) const JE_1_18_2_DATA_VERSION: i32 = 2975;
pub(crate) const JE_1_18_2_MIN_SECTION_Y: i32 = -4;
pub(crate) const JE_1_18_2_MAX_SECTION_Y: i32 = 19;
const LEVEL_DAT: &str = "level.dat";
const PLAYERDATA_DIR: &str = "playerdata";
const REGION_DIR: &str = "region";

pub const JE_1_18_2_STORAGE_PROFILE_ID: &str = "je-anvil-1_18_2";
pub const JE_1_18_2_STORAGE_PLUGIN_ID: &str = "storage-je-anvil-1_18_2";

#[derive(Default)]
pub struct Je1182StoragePlugin;

impl RustStoragePlugin for Je1182StoragePlugin {
    fn descriptor(&self) -> StorageDescriptor {
        StorageDescriptor {
            storage_profile: JE_1_18_2_STORAGE_PROFILE_ID.into(),
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
    JE_1_18_2_STORAGE_PLUGIN_ID,
    "JE 1.18.2 Anvil Storage Plugin",
    JE_1_18_2_STORAGE_PROFILE_ID,
);

export_plugin!(storage, Je1182StoragePlugin, MANIFEST);

pub(crate) fn item_stack_to_nbt(
    stack: &ItemStack,
    slot: Option<i8>,
) -> Result<NbtTag, StorageError> {
    if !stack.is_legacy_compatible() {
        return Err(StorageError::InvalidData(
            "modern item components are not supported by je-anvil-1_18_2".to_string(),
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
        "Count".to_string(),
        NbtTag::Byte(i8::try_from(stack.count).expect("count should fit into i8")),
    );
    if stack.damage != 0 {
        compound.insert("Damage".to_string(), NbtTag::Int(i32::from(stack.damage)));
    }
    Ok(NbtTag::Compound(compound))
}

pub(crate) fn item_stack_from_nbt(
    compound: &BTreeMap<String, NbtTag>,
) -> Result<ItemStack, StorageError> {
    let key = nbt::string_field(compound, "id")?;
    let count = item_count_or_zero(compound)?;
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

pub(crate) fn item_count_or_zero(compound: &BTreeMap<String, NbtTag>) -> Result<u8, StorageError> {
    match compound.get("Count") {
        Some(NbtTag::Byte(value)) => u8::try_from(*value).map_err(|_| {
            StorageError::InvalidData("negative item count not supported".to_string())
        }),
        Some(NbtTag::Short(value)) => u8::try_from(*value).map_err(|_| {
            StorageError::InvalidData("negative item count not supported".to_string())
        }),
        Some(NbtTag::Int(value)) => u8::try_from(*value).map_err(|_| {
            StorageError::InvalidData("negative item count not supported".to_string())
        }),
        Some(_) => Err(StorageError::InvalidData(
            "item Count field had an unsupported type".to_string(),
        )),
        None => Ok(0),
    }
}

pub(crate) fn validate_storage_item(
    compound: &BTreeMap<String, NbtTag>,
    allow_slot: bool,
    context: &str,
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
    if compound.contains_key("tag") {
        return Err(StorageError::InvalidData(format!(
            "{context} item tag is not supported"
        )));
    }
    Ok(())
}
