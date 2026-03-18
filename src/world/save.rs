use super::{
    BlockData, BlockType, ChunkCoord, ChunkData, ChunkSaveVersion, LocalBlockCoord, WorldLayout,
};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

const MAGIC: [u8; 4] = *b"RVCW";

pub fn chunk_path(root: &Path, chunk_coord: ChunkCoord) -> PathBuf {
    root.join(format!("chunk_{}_{}.rvc", chunk_coord.x, chunk_coord.z))
}

pub fn load_chunk(
    root: &Path,
    chunk_coord: ChunkCoord,
    layout: WorldLayout,
) -> io::Result<Option<ChunkData>> {
    let path = chunk_path(root, chunk_coord);
    if !path.exists() {
        return Ok(None);
    }

    let file = File::open(path)?;
    let mut reader = BufReader::new(file);

    let mut magic = [0_u8; 4];
    reader.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid chunk save magic",
        ));
    }

    let version = read_u32(&mut reader)?;
    if version != ChunkSaveVersion::CURRENT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported chunk save version: {version}"),
        ));
    }

    let stored_chunk_size = read_i64(&mut reader)?;
    let stored_vertical_min = read_i64(&mut reader)?;
    let stored_vertical_max = read_i64(&mut reader)?;
    let stored_layout =
        WorldLayout::new(stored_chunk_size, stored_vertical_min, stored_vertical_max);
    if stored_layout != layout {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "saved chunk layout does not match current world layout",
        ));
    }

    let generated_from_seed = read_bool(&mut reader)?;
    let block_count = read_u32(&mut reader)?;
    let mut blocks = HashMap::new();

    for _ in 0..block_count {
        let local_coord = LocalBlockCoord::new(
            read_i64(&mut reader)?,
            read_i64(&mut reader)?,
            read_i64(&mut reader)?,
        );
        let block_type = block_type_from_byte(read_u8(&mut reader)?)?;
        blocks.insert(local_coord, BlockData::new(block_type));
    }

    Ok(Some(ChunkData::loaded(blocks, generated_from_seed)))
}

pub fn save_chunk(
    root: &Path,
    chunk_coord: ChunkCoord,
    chunk_data: &ChunkData,
    layout: WorldLayout,
) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let file = File::create(chunk_path(root, chunk_coord))?;
    let mut writer = BufWriter::new(file);

    writer.write_all(&MAGIC)?;
    write_u32(&mut writer, ChunkSaveVersion::CURRENT)?;
    write_i64(&mut writer, layout.chunk_size())?;
    write_i64(&mut writer, layout.vertical_min())?;
    write_i64(&mut writer, layout.vertical_max())?;
    write_bool(&mut writer, chunk_data.generated_from_seed)?;

    let mut entries: Vec<_> = chunk_data.blocks.iter().collect();
    entries.sort_unstable_by_key(|(local_coord, _)| {
        (local_coord.y(), local_coord.z(), local_coord.x())
    });
    write_u32(
        &mut writer,
        u32::try_from(entries.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "chunk contains too many blocks to save",
            )
        })?,
    )?;

    for (&local_coord, block_data) in entries {
        write_i64(&mut writer, local_coord.x())?;
        write_i64(&mut writer, local_coord.y())?;
        write_i64(&mut writer, local_coord.z())?;
        write_u8(&mut writer, block_type_to_byte(block_data.kind))?;
    }

    writer.flush()?;
    Ok(())
}

const fn block_type_to_byte(block_type: BlockType) -> u8 {
    match block_type {
        BlockType::Grass => 0,
        BlockType::Dirt => 1,
        BlockType::Stone => 2,
    }
}

fn block_type_from_byte(byte: u8) -> io::Result<BlockType> {
    match byte {
        0 => Ok(BlockType::Grass),
        1 => Ok(BlockType::Dirt),
        2 => Ok(BlockType::Stone),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid block type tag: {byte}"),
        )),
    }
}

fn read_u8(reader: &mut impl Read) -> io::Result<u8> {
    let mut bytes = [0_u8; 1];
    reader.read_exact(&mut bytes)?;
    Ok(bytes[0])
}

fn read_bool(reader: &mut impl Read) -> io::Result<bool> {
    Ok(read_u8(reader)? != 0)
}

fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_i64(reader: &mut impl Read) -> io::Result<i64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(i64::from_le_bytes(bytes))
}

fn write_u8(writer: &mut impl Write, value: u8) -> io::Result<()> {
    writer.write_all(&[value])
}

fn write_bool(writer: &mut impl Write, value: bool) -> io::Result<()> {
    write_u8(writer, u8::from(value))
}

fn write_u32(writer: &mut impl Write, value: u32) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_i64(writer: &mut impl Write, value: i64) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}
