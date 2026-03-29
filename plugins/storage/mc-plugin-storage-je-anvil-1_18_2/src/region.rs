use super::chunk_nbt::{chunk_from_nbt, chunk_to_nbt};
use super::nbt::{NbtTag, decompress_zlib, read_nbt, zlib_compress_nbt};
use mc_proto_common::StorageError;
use revy_voxel_model::{BlockPos, ChunkColumn, ChunkPos};
use revy_voxel_rules::BlockEntityState;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const ANVIL_SECTOR_BYTES: usize = 4096;
const ANVIL_HEADER_BYTES: usize = ANVIL_SECTOR_BYTES * 2;

pub(super) fn write_regions(
    region_dir: &Path,
    chunks: &BTreeMap<ChunkPos, ChunkColumn>,
    block_entities: &BTreeMap<BlockPos, BlockEntityState>,
) -> Result<(), StorageError> {
    fs::create_dir_all(region_dir)?;
    let mut grouped = BTreeMap::<(i32, i32), Vec<&ChunkColumn>>::new();
    for chunk in chunks.values() {
        grouped
            .entry((chunk.pos.x.div_euclid(32), chunk.pos.z.div_euclid(32)))
            .or_default()
            .push(chunk);
    }

    for ((region_x, region_z), region_chunks) in grouped {
        let path = region_dir.join(format!("r.{region_x}.{region_z}.mca"));
        let existing = if path.exists() {
            read_region_file(&path)?
        } else {
            BTreeMap::new()
        };
        let mut merged = existing;
        for chunk in region_chunks {
            let existing_chunk = merged.get(&chunk.pos);
            let encoded = chunk_to_nbt(chunk, block_entities, existing_chunk)?;
            merged.insert(chunk.pos, encoded);
        }
        write_region_file(&path, &merged)?;
    }
    Ok(())
}

pub(super) fn read_regions(
    region_dir: &Path,
) -> Result<
    (
        BTreeMap<ChunkPos, ChunkColumn>,
        BTreeMap<BlockPos, BlockEntityState>,
    ),
    StorageError,
> {
    let mut chunks = BTreeMap::new();
    let mut block_entities = BTreeMap::new();
    if !region_dir.exists() {
        return Ok((chunks, block_entities));
    }
    for entry in fs::read_dir(region_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("mca") {
            continue;
        }
        let region_chunks = read_region_file(&path)?;
        for (chunk_pos, chunk_root) in region_chunks {
            let (chunk, chunk_block_entities) = chunk_from_nbt(&chunk_root)?;
            chunks.insert(chunk_pos, chunk);
            block_entities.extend(chunk_block_entities);
        }
    }
    Ok((chunks, block_entities))
}

fn write_region_file(path: &Path, chunks: &BTreeMap<ChunkPos, NbtTag>) -> Result<(), StorageError> {
    let mut locations = vec![0_u8; ANVIL_SECTOR_BYTES];
    let timestamps = vec![0_u8; ANVIL_SECTOR_BYTES];
    let mut sectors = Vec::new();
    let mut next_sector = 2_usize;

    for (chunk_pos, chunk_root) in chunks {
        let chunk_nbt = zlib_compress_nbt("", chunk_root)?;
        let total_bytes = 4 + 1 + chunk_nbt.len();
        let sector_count = total_bytes.div_ceil(ANVIL_SECTOR_BYTES);
        let index = region_chunk_index(*chunk_pos);
        let offset = next_sector;
        next_sector += sector_count;
        let location = ((u32::try_from(offset).expect("offset should fit into u32")) << 8)
            | u32::try_from(sector_count).expect("sector count should fit into u32");
        locations[index * 4..index * 4 + 4].copy_from_slice(&location.to_be_bytes());

        let mut bytes = Vec::with_capacity(sector_count * ANVIL_SECTOR_BYTES);
        bytes.extend_from_slice(
            &u32::try_from(chunk_nbt.len() + 1)
                .expect("chunk payload length should fit into u32")
                .to_be_bytes(),
        );
        bytes.push(2);
        bytes.extend_from_slice(&chunk_nbt);
        let padding = sector_count * ANVIL_SECTOR_BYTES - total_bytes;
        bytes.extend(std::iter::repeat_n(0_u8, padding));
        sectors.push(bytes);
    }

    let mut region_bytes = Vec::with_capacity(next_sector * ANVIL_SECTOR_BYTES);
    region_bytes.extend_from_slice(&locations);
    region_bytes.extend_from_slice(&timestamps);
    for sector in sectors {
        region_bytes.extend_from_slice(&sector);
    }
    fs::write(path, region_bytes)?;
    Ok(())
}

fn read_region_file(path: &Path) -> Result<BTreeMap<ChunkPos, NbtTag>, StorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() < ANVIL_HEADER_BYTES {
        return Err(StorageError::InvalidData(
            "region file is too small".to_string(),
        ));
    }
    let mut chunks = BTreeMap::new();
    for index in 0..1024 {
        let base = index * 4;
        let sector_offset = (usize::from(bytes[base]) << 16)
            | (usize::from(bytes[base + 1]) << 8)
            | usize::from(bytes[base + 2]);
        let sector_count = usize::from(bytes[base + 3]);
        if sector_offset == 0 || sector_count == 0 {
            continue;
        }
        let start = sector_offset * ANVIL_SECTOR_BYTES;
        let end = start + sector_count * ANVIL_SECTOR_BYTES;
        if end > bytes.len() || start + 5 > end {
            return Err(StorageError::InvalidData(
                "region chunk location was out of bounds".to_string(),
            ));
        }
        let length = usize::try_from(u32::from_be_bytes(
            bytes[start..start + 4]
                .try_into()
                .expect("chunk length should fit"),
        ))
        .expect("chunk length should fit into usize");
        if length == 0 || start + 4 + length > end {
            return Err(StorageError::InvalidData(
                "region chunk length was invalid".to_string(),
            ));
        }
        let compression = bytes[start + 4];
        if compression & 0x80 != 0 {
            return Err(StorageError::InvalidData(
                "external chunk streams are not supported".to_string(),
            ));
        }
        let payload = &bytes[start + 5..start + 4 + length];
        let decompressed = match compression {
            1 => super::nbt::decompress_gzip(payload)?,
            2 => decompress_zlib(payload)?,
            3 => payload.to_vec(),
            _ => {
                return Err(StorageError::InvalidData(format!(
                    "unsupported region compression {compression}"
                )));
            }
        };
        let chunk_root = read_nbt(&decompressed)?;
        let (chunk, _) = chunk_from_nbt(&chunk_root)?;
        chunks.insert(chunk.pos, chunk_root);
    }
    Ok(chunks)
}

fn region_chunk_index(pos: ChunkPos) -> usize {
    let local_x =
        usize::try_from(pos.x.rem_euclid(32)).expect("local region x should fit into usize");
    let local_z =
        usize::try_from(pos.z.rem_euclid(32)).expect("local region z should fit into usize");
    local_x + local_z * 32
}
