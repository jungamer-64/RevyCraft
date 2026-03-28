use super::JE_1_18_2_DATA_VERSION;
use super::nbt::{
    NbtTag, as_compound, byte_field, compound_field, int_field, long_field, read_gzip_nbt,
    string_field, write_gzip_nbt,
};
use mc_core::{BlockPos, DimensionId, WorldMeta};
use mc_proto_common::StorageError;
use std::collections::BTreeMap;
use std::path::Path;

const STORAGE_VERSION: i32 = 19133;

pub(super) fn read_level_dat(path: &Path) -> Result<WorldMeta, StorageError> {
    let root = read_gzip_nbt(path)?;
    let data = compound_field(as_compound(&root)?, "Data")?;
    validate_data_version(data)?;
    Ok(WorldMeta {
        level_name: string_field(data, "LevelName").unwrap_or_else(|_| "world".to_string()),
        seed: worldgen_seed(data).unwrap_or(0),
        spawn: BlockPos::new(
            int_field(data, "SpawnX").unwrap_or(0),
            int_field(data, "SpawnY").unwrap_or(4),
            int_field(data, "SpawnZ").unwrap_or(0),
        ),
        dimension: DimensionId::Overworld,
        age: long_field(data, "Time").unwrap_or(0),
        time: long_field(data, "DayTime").unwrap_or_else(|_| long_field(data, "Time").unwrap_or(0)),
        level_type: world_level_type(data),
        game_mode: u8::try_from(int_field(data, "GameType").unwrap_or(0)).unwrap_or(0),
        difficulty: byte_field(data, "Difficulty").unwrap_or(1).to_be_bytes()[0],
        max_players: 20,
    })
}

pub(super) fn write_level_dat(path: &Path, meta: &WorldMeta) -> Result<(), StorageError> {
    let existing_root = if path.exists() {
        Some(read_gzip_nbt(path)?)
    } else {
        None
    };
    let mut root = existing_root
        .as_ref()
        .map(as_compound)
        .transpose()?
        .cloned()
        .unwrap_or_default();
    let mut data = root
        .get("Data")
        .map(as_compound)
        .transpose()?
        .cloned()
        .unwrap_or_default();

    data.insert(
        "DataVersion".to_string(),
        NbtTag::Int(JE_1_18_2_DATA_VERSION),
    );
    data.insert("version".to_string(), NbtTag::Int(STORAGE_VERSION));
    data.insert("Version".to_string(), version_compound());
    data.insert(
        "LevelName".to_string(),
        NbtTag::String(meta.level_name.clone()),
    );
    data.insert("SpawnX".to_string(), NbtTag::Int(meta.spawn.x));
    data.insert("SpawnY".to_string(), NbtTag::Int(meta.spawn.y));
    data.insert("SpawnZ".to_string(), NbtTag::Int(meta.spawn.z));
    data.insert(
        "GameType".to_string(),
        NbtTag::Int(i32::from(meta.game_mode)),
    );
    data.insert(
        "Difficulty".to_string(),
        NbtTag::Byte(i8::from_be_bytes([meta.difficulty])),
    );
    data.insert(
        "Time".to_string(),
        NbtTag::Long(i64::from_be_bytes(meta.age.to_be_bytes())),
    );
    data.insert(
        "DayTime".to_string(),
        NbtTag::Long(i64::from_be_bytes(meta.time.to_be_bytes())),
    );
    data.insert("initialized".to_string(), NbtTag::Byte(1));
    if !data.contains_key("WorldGenSettings") {
        data.insert(
            "WorldGenSettings".to_string(),
            flat_world_gen_settings(meta.seed),
        );
    } else if let Some(world_gen_settings) = data.get_mut("WorldGenSettings") {
        update_world_gen_seed(world_gen_settings, meta.seed)?;
    }

    root.insert("Data".to_string(), NbtTag::Compound(data));
    write_gzip_nbt(path, "", &NbtTag::Compound(root))
}

fn validate_data_version(data: &BTreeMap<String, NbtTag>) -> Result<(), StorageError> {
    let data_version = int_field(data, "DataVersion")?;
    if data_version != JE_1_18_2_DATA_VERSION {
        return Err(StorageError::InvalidData(format!(
            "expected Data.DataVersion={}, got {data_version}",
            JE_1_18_2_DATA_VERSION
        )));
    }
    let version = compound_field(data, "Version")?;
    let version_id = int_field(version, "Id")?;
    if version_id != JE_1_18_2_DATA_VERSION {
        return Err(StorageError::InvalidData(format!(
            "expected Data.Version.Id={}, got {version_id}",
            JE_1_18_2_DATA_VERSION
        )));
    }
    Ok(())
}

fn world_level_type(data: &BTreeMap<String, NbtTag>) -> String {
    let Ok(world_gen_settings) = compound_field(data, "WorldGenSettings") else {
        return "FLAT".to_string();
    };
    let Ok(dimensions) = compound_field(world_gen_settings, "dimensions") else {
        return "FLAT".to_string();
    };
    let Ok(overworld) = compound_field(dimensions, "minecraft:overworld") else {
        return "FLAT".to_string();
    };
    let Ok(generator) = compound_field(overworld, "generator") else {
        return "FLAT".to_string();
    };
    let Ok(generator_type) = string_field(generator, "type") else {
        return "FLAT".to_string();
    };
    generator_type
        .rsplit(':')
        .next()
        .unwrap_or("flat")
        .to_ascii_uppercase()
}

fn worldgen_seed(data: &BTreeMap<String, NbtTag>) -> Result<u64, StorageError> {
    let world_gen_settings = compound_field(data, "WorldGenSettings")?;
    Ok(u64::from_be_bytes(
        long_field(world_gen_settings, "seed")?.to_be_bytes(),
    ))
}

fn version_compound() -> NbtTag {
    let mut version = BTreeMap::new();
    version.insert("Id".to_string(), NbtTag::Int(JE_1_18_2_DATA_VERSION));
    version.insert("Name".to_string(), NbtTag::String("1.18.2".to_string()));
    version.insert("Series".to_string(), NbtTag::String("main".to_string()));
    version.insert("Snapshot".to_string(), NbtTag::Byte(0));
    NbtTag::Compound(version)
}

fn flat_world_gen_settings(seed: u64) -> NbtTag {
    let mut world_gen_settings = BTreeMap::new();
    world_gen_settings.insert(
        "seed".to_string(),
        NbtTag::Long(i64::from_be_bytes(seed.to_be_bytes())),
    );
    world_gen_settings.insert("generate_features".to_string(), NbtTag::Byte(0));
    world_gen_settings.insert("bonus_chest".to_string(), NbtTag::Byte(0));

    let mut dimensions = BTreeMap::new();
    dimensions.insert(
        "minecraft:overworld".to_string(),
        NbtTag::Compound(overworld_dimension()),
    );
    world_gen_settings.insert("dimensions".to_string(), NbtTag::Compound(dimensions));
    NbtTag::Compound(world_gen_settings)
}

fn overworld_dimension() -> BTreeMap<String, NbtTag> {
    let mut overworld = BTreeMap::new();
    overworld.insert(
        "type".to_string(),
        NbtTag::String("minecraft:overworld".to_string()),
    );

    let mut generator = BTreeMap::new();
    generator.insert(
        "type".to_string(),
        NbtTag::String("minecraft:flat".to_string()),
    );

    let mut settings = BTreeMap::new();
    settings.insert(
        "biome".to_string(),
        NbtTag::String("minecraft:plains".to_string()),
    );
    settings.insert("features".to_string(), NbtTag::Byte(0));
    settings.insert("lakes".to_string(), NbtTag::Byte(0));
    settings.insert(
        "layers".to_string(),
        NbtTag::List(
            10,
            vec![
                layer("minecraft:bedrock", 1),
                layer("minecraft:stone", 2),
                layer("minecraft:dirt", 1),
                layer("minecraft:grass_block", 1),
            ],
        ),
    );
    settings.insert("structures".to_string(), NbtTag::Compound(BTreeMap::new()));
    generator.insert("settings".to_string(), NbtTag::Compound(settings));
    overworld.insert("generator".to_string(), NbtTag::Compound(generator));
    overworld
}

fn layer(block: &str, height: i32) -> NbtTag {
    let mut layer = BTreeMap::new();
    layer.insert("block".to_string(), NbtTag::String(block.to_string()));
    layer.insert("height".to_string(), NbtTag::Int(height));
    NbtTag::Compound(layer)
}

fn update_world_gen_seed(world_gen_settings: &mut NbtTag, seed: u64) -> Result<(), StorageError> {
    let world_gen_settings = match world_gen_settings {
        NbtTag::Compound(world_gen_settings) => world_gen_settings,
        _ => {
            return Err(StorageError::InvalidData(
                "WorldGenSettings was not a compound".to_string(),
            ));
        }
    };
    world_gen_settings.insert(
        "seed".to_string(),
        NbtTag::Long(i64::from_be_bytes(seed.to_be_bytes())),
    );
    Ok(())
}
