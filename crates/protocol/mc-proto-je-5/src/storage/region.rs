use super::chunk_nbt::{chunk_from_nbt, chunk_to_nbt, region_chunk_index};
use super::nbt::{decompress_gzip, decompress_zlib, read_nbt, zlib_compress_nbt};
use mc_core::{ChunkColumn, ChunkPos};
use mc_proto_common::StorageError;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

const ANVIL_SECTOR_BYTES: usize = 4096;
const ANVIL_HEADER_BYTES: usize = ANVIL_SECTOR_BYTES * 2;
const CHUNK_COMPRESSION_ZLIB: u8 = 2;

pub(super) fn write_regions(
    region_dir: &Path,
    chunks: &BTreeMap<ChunkPos, ChunkColumn>,
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
        write_region_file(&path, &region_chunks)?;
    }
    Ok(())
}

pub(super) fn read_regions(
    region_dir: &Path,
) -> Result<BTreeMap<ChunkPos, ChunkColumn>, StorageError> {
    let mut chunks = BTreeMap::new();
    if !region_dir.exists() {
        return Ok(chunks);
    }
    for entry in fs::read_dir(region_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("mca") {
            continue;
        }
        for chunk in read_region_file(&path)? {
            chunks.insert(chunk.pos, chunk);
        }
    }
    Ok(chunks)
}

fn write_region_file(path: &Path, chunks: &[&ChunkColumn]) -> Result<(), StorageError> {
    let mut locations = vec![0_u8; ANVIL_SECTOR_BYTES];
    let mut timestamps = vec![0_u8; ANVIL_SECTOR_BYTES];
    let mut body = Vec::new();
    let mut sector_offset = 2_u32;

    for chunk in chunks {
        let index = region_chunk_index(chunk.pos);
        let chunk_nbt = chunk_to_nbt(chunk);
        let compressed = zlib_compress_nbt("", &chunk_nbt)?;
        let length = u32::try_from(compressed.len() + 1)
            .map_err(|_| StorageError::InvalidData("compressed chunk too large".to_string()))?;
        let total_bytes = usize::try_from(length + 4).expect("chunk length should fit into usize");
        let sector_count = total_bytes.div_ceil(ANVIL_SECTOR_BYTES);
        let location = (sector_offset << 8)
            | u32::try_from(sector_count).expect("sector count should fit into u32");
        locations[index * 4..index * 4 + 4].copy_from_slice(&location.to_be_bytes());
        timestamps[index * 4..index * 4 + 4].copy_from_slice(&0_u32.to_be_bytes());

        body.extend_from_slice(&length.to_be_bytes());
        body.push(CHUNK_COMPRESSION_ZLIB);
        body.extend_from_slice(&compressed);
        let padding = sector_count * ANVIL_SECTOR_BYTES - total_bytes;
        body.resize(body.len() + padding, 0);
        sector_offset = sector_offset
            .saturating_add(u32::try_from(sector_count).expect("sector count should fit into u32"));
    }

    let mut file = File::create(path)?;
    file.write_all(&locations)?;
    file.write_all(&timestamps)?;
    file.write_all(&body)?;
    Ok(())
}

fn read_region_file(path: &Path) -> Result<Vec<ChunkColumn>, StorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() < ANVIL_HEADER_BYTES {
        return Err(StorageError::InvalidData(
            "region file is too small".to_string(),
        ));
    }
    let mut chunks = Vec::new();
    for index in 0..1024 {
        let location = u32::from_be_bytes(
            bytes[index * 4..index * 4 + 4]
                .try_into()
                .expect("region location should fit"),
        );
        if location == 0 {
            continue;
        }
        let sector_offset =
            usize::try_from(location >> 8).expect("sector offset should fit into usize");
        let sector_count =
            usize::try_from(location & 0xff).expect("sector count should fit into usize");
        let start = sector_offset * ANVIL_SECTOR_BYTES;
        let end = start + sector_count * ANVIL_SECTOR_BYTES;
        if end > bytes.len() || start + 5 > end {
            continue;
        }
        let length = usize::try_from(u32::from_be_bytes(
            bytes[start..start + 4]
                .try_into()
                .expect("chunk length should fit"),
        ))
        .expect("chunk length should fit into usize");
        if length == 0 || start + 4 + length > end {
            continue;
        }
        let compression = bytes[start + 4];
        let payload = &bytes[start + 5..start + 4 + length];
        let decompressed = match compression {
            1 => decompress_gzip(payload)?,
            2 => decompress_zlib(payload)?,
            _ => {
                return Err(StorageError::InvalidData(
                    "unsupported region compression".to_string(),
                ));
            }
        };
        chunks.push(chunk_from_nbt(&read_nbt(&decompressed)?)?);
    }
    Ok(chunks)
}
