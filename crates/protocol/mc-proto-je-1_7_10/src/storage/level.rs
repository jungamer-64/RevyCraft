use super::nbt::{
    NbtTag, as_compound, byte_field, compound_field, int_field, long_field, read_gzip_nbt,
    string_field, write_gzip_nbt,
};
use mc_core::{BlockPos, DimensionId, WorldMeta};
use mc_proto_common::StorageError;
use std::collections::BTreeMap;
use std::path::Path;

pub(super) fn write_level_dat(path: &Path, meta: &WorldMeta) -> Result<(), StorageError> {
    let mut data = BTreeMap::new();
    data.insert("version".to_string(), NbtTag::Int(19133));
    data.insert(
        "LevelName".to_string(),
        NbtTag::String(meta.level_name.clone()),
    );
    data.insert(
        "generatorName".to_string(),
        NbtTag::String(meta.level_type.to_ascii_lowercase()),
    );
    data.insert("SpawnX".to_string(), NbtTag::Int(meta.spawn.x));
    data.insert("SpawnY".to_string(), NbtTag::Int(meta.spawn.y));
    data.insert("SpawnZ".to_string(), NbtTag::Int(meta.spawn.z));
    data.insert(
        "RandomSeed".to_string(),
        NbtTag::Long(meta.seed.cast_signed()),
    );
    data.insert("Time".to_string(), NbtTag::Long(meta.time));
    data.insert(
        "GameType".to_string(),
        NbtTag::Int(i32::from(meta.game_mode)),
    );
    data.insert(
        "Difficulty".to_string(),
        NbtTag::Byte(i8::from_be_bytes([meta.difficulty])),
    );
    data.insert("MapFeatures".to_string(), NbtTag::Byte(0));
    data.insert("initialized".to_string(), NbtTag::Byte(1));

    let mut root = BTreeMap::new();
    root.insert("Data".to_string(), NbtTag::Compound(data));
    write_gzip_nbt(path, "", &NbtTag::Compound(root))
}

pub(super) fn read_level_dat(path: &Path) -> Result<WorldMeta, StorageError> {
    let root = read_gzip_nbt(path)?;
    let data = compound_field(as_compound(&root)?, "Data")?;
    Ok(WorldMeta {
        level_name: string_field(data, "LevelName").unwrap_or_else(|_| "world".to_string()),
        seed: long_field(data, "RandomSeed").unwrap_or(0).cast_unsigned(),
        spawn: BlockPos::new(
            int_field(data, "SpawnX").unwrap_or(0),
            int_field(data, "SpawnY").unwrap_or(4),
            int_field(data, "SpawnZ").unwrap_or(0),
        ),
        dimension: DimensionId::Overworld,
        age: long_field(data, "Time").unwrap_or(0),
        time: long_field(data, "Time").unwrap_or(0),
        level_type: string_field(data, "generatorName")
            .unwrap_or_else(|_| "flat".to_string())
            .to_ascii_uppercase(),
        game_mode: u8::try_from(int_field(data, "GameType").unwrap_or(0)).unwrap_or(0),
        difficulty: byte_field(data, "Difficulty").unwrap_or(1).to_be_bytes()[0],
        max_players: 20,
    })
}
