use flate2::Compression;
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::GzEncoder;
use mc_proto_common::StorageError;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum NbtTag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(u8, Vec<Self>),
    Compound(BTreeMap<String, Self>),
    IntArray(Vec<i32>),
}

pub(super) fn zlib_compress_nbt(name: &str, tag: &NbtTag) -> Result<Vec<u8>, StorageError> {
    let mut raw = Vec::new();
    write_named_tag(&mut raw, 10, name, tag)?;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw)?;
    Ok(encoder.finish()?)
}

pub(super) fn write_gzip_nbt(path: &Path, name: &str, tag: &NbtTag) -> Result<(), StorageError> {
    let mut raw = Vec::new();
    write_named_tag(&mut raw, 10, name, tag)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw)?;
    fs::write(path, encoder.finish()?)?;
    Ok(())
}

pub(super) fn read_gzip_nbt(path: &Path) -> Result<NbtTag, StorageError> {
    let bytes = fs::read(path)?;
    read_nbt(&decompress_gzip(&bytes)?)
}

pub(super) fn decompress_gzip(bytes: &[u8]) -> Result<Vec<u8>, StorageError> {
    let mut decoder = GzDecoder::new(Cursor::new(bytes));
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    Ok(decompressed)
}

pub(super) fn decompress_zlib(bytes: &[u8]) -> Result<Vec<u8>, StorageError> {
    let mut decoder = ZlibDecoder::new(Cursor::new(bytes));
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    Ok(decompressed)
}

pub(super) fn read_nbt(bytes: &[u8]) -> Result<NbtTag, StorageError> {
    let mut cursor = Cursor::new(bytes);
    let tag_type = read_u8(&mut cursor)?;
    if tag_type != 10 {
        return Err(StorageError::InvalidData(
            "root nbt tag must be a compound".to_string(),
        ));
    }
    let _name = read_string_u16(&mut cursor)?;
    read_tag_payload(&mut cursor, tag_type)
}

pub(super) fn as_compound(tag: &NbtTag) -> Result<&BTreeMap<String, NbtTag>, StorageError> {
    match tag {
        NbtTag::Compound(values) => Ok(values),
        _ => Err(StorageError::InvalidData(
            "expected compound tag".to_string(),
        )),
    }
}

pub(super) fn compound_field<'a>(
    compound: &'a BTreeMap<String, NbtTag>,
    key: &str,
) -> Result<&'a BTreeMap<String, NbtTag>, StorageError> {
    as_compound(
        compound
            .get(key)
            .ok_or_else(|| StorageError::InvalidData(format!("missing compound field {key}")))?,
    )
}

pub(super) fn list_field<'a>(
    compound: &'a BTreeMap<String, NbtTag>,
    key: &str,
) -> Result<&'a [NbtTag], StorageError> {
    match compound.get(key) {
        Some(NbtTag::List(_, values)) => Ok(values),
        _ => Err(StorageError::InvalidData(format!(
            "missing list field {key}"
        ))),
    }
}

pub(super) fn byte_array_field(
    compound: &BTreeMap<String, NbtTag>,
    key: &str,
) -> Result<Vec<u8>, StorageError> {
    match compound.get(key) {
        Some(NbtTag::ByteArray(values)) => Ok(values.clone()),
        _ => Err(StorageError::InvalidData(format!(
            "missing byte array field {key}"
        ))),
    }
}

pub(super) fn string_field(
    compound: &BTreeMap<String, NbtTag>,
    key: &str,
) -> Result<String, StorageError> {
    match compound.get(key) {
        Some(NbtTag::String(value)) => Ok(value.clone()),
        _ => Err(StorageError::InvalidData(format!(
            "missing string field {key}"
        ))),
    }
}

pub(super) fn int_field(
    compound: &BTreeMap<String, NbtTag>,
    key: &str,
) -> Result<i32, StorageError> {
    match compound.get(key) {
        Some(NbtTag::Int(value)) => Ok(*value),
        _ => Err(StorageError::InvalidData(format!(
            "missing int field {key}"
        ))),
    }
}

pub(super) fn short_field(
    compound: &BTreeMap<String, NbtTag>,
    key: &str,
) -> Result<i16, StorageError> {
    match compound.get(key) {
        Some(NbtTag::Short(value)) => Ok(*value),
        _ => Err(StorageError::InvalidData(format!(
            "missing short field {key}"
        ))),
    }
}

pub(super) fn long_field(
    compound: &BTreeMap<String, NbtTag>,
    key: &str,
) -> Result<i64, StorageError> {
    match compound.get(key) {
        Some(NbtTag::Long(value)) => Ok(*value),
        _ => Err(StorageError::InvalidData(format!(
            "missing long field {key}"
        ))),
    }
}

pub(super) fn byte_field(
    compound: &BTreeMap<String, NbtTag>,
    key: &str,
) -> Result<i8, StorageError> {
    match compound.get(key) {
        Some(NbtTag::Byte(value)) => Ok(*value),
        _ => Err(StorageError::InvalidData(format!(
            "missing byte field {key}"
        ))),
    }
}

pub(super) fn float_field(
    compound: &BTreeMap<String, NbtTag>,
    key: &str,
) -> Result<f32, StorageError> {
    match compound.get(key) {
        Some(NbtTag::Float(value)) => Ok(*value),
        _ => Err(StorageError::InvalidData(format!(
            "missing float field {key}"
        ))),
    }
}

pub(super) fn double_from_tag(tag: &NbtTag) -> Result<f64, StorageError> {
    match tag {
        NbtTag::Double(value) => Ok(*value),
        _ => Err(StorageError::InvalidData("expected double tag".to_string())),
    }
}

pub(super) fn float_from_tag(tag: &NbtTag) -> Result<f32, StorageError> {
    match tag {
        NbtTag::Float(value) => Ok(*value),
        _ => Err(StorageError::InvalidData("expected float tag".to_string())),
    }
}

fn write_named_tag(
    writer: &mut impl Write,
    tag_type: u8,
    name: &str,
    tag: &NbtTag,
) -> Result<(), StorageError> {
    write_u8(writer, tag_type)?;
    write_string_u16(writer, name)?;
    write_tag_payload(writer, tag)
}

fn write_tag_payload(writer: &mut impl Write, tag: &NbtTag) -> Result<(), StorageError> {
    match tag {
        NbtTag::Byte(value) => write_i8(writer, *value),
        NbtTag::Short(value) => write_i16(writer, *value),
        NbtTag::Int(value) => write_i32(writer, *value),
        NbtTag::Long(value) => write_i64(writer, *value),
        NbtTag::Float(value) => write_f32(writer, *value),
        NbtTag::Double(value) => write_f64(writer, *value),
        NbtTag::ByteArray(values) => {
            write_i32(
                writer,
                i32::try_from(values.len()).expect("byte array length should fit into i32"),
            )?;
            writer.write_all(values)?;
            Ok(())
        }
        NbtTag::String(value) => write_string_u16(writer, value),
        NbtTag::List(tag_type, values) => {
            write_u8(writer, *tag_type)?;
            write_i32(
                writer,
                i32::try_from(values.len()).expect("list length should fit into i32"),
            )?;
            for value in values {
                write_tag_payload(writer, value)?;
            }
            Ok(())
        }
        NbtTag::Compound(values) => {
            for (name, value) in values {
                let tag_type = tag_type(value);
                write_named_tag(writer, tag_type, name, value)?;
            }
            write_u8(writer, 0)?;
            Ok(())
        }
        NbtTag::IntArray(values) => {
            write_i32(
                writer,
                i32::try_from(values.len()).expect("int array length should fit into i32"),
            )?;
            for value in values {
                write_i32(writer, *value)?;
            }
            Ok(())
        }
    }
}

fn read_tag_payload(reader: &mut impl Read, tag_type: u8) -> Result<NbtTag, StorageError> {
    match tag_type {
        1 => Ok(NbtTag::Byte(read_i8(reader)?)),
        2 => Ok(NbtTag::Short(read_i16(reader)?)),
        3 => Ok(NbtTag::Int(read_i32(reader)?)),
        4 => Ok(NbtTag::Long(read_i64(reader)?)),
        5 => Ok(NbtTag::Float(read_f32(reader)?)),
        6 => Ok(NbtTag::Double(read_f64(reader)?)),
        7 => {
            let len = usize::try_from(read_i32(reader)?)
                .map_err(|_| StorageError::InvalidData("negative byte array length".to_string()))?;
            let mut bytes = vec![0_u8; len];
            reader.read_exact(&mut bytes)?;
            Ok(NbtTag::ByteArray(bytes))
        }
        8 => Ok(NbtTag::String(read_string_u16(reader)?)),
        9 => {
            let child_type = read_u8(reader)?;
            let len = usize::try_from(read_i32(reader)?)
                .map_err(|_| StorageError::InvalidData("negative list length".to_string()))?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(read_tag_payload(reader, child_type)?);
            }
            Ok(NbtTag::List(child_type, values))
        }
        10 => {
            let mut values = BTreeMap::new();
            loop {
                let child_type = read_u8(reader)?;
                if child_type == 0 {
                    break;
                }
                let name = read_string_u16(reader)?;
                values.insert(name, read_tag_payload(reader, child_type)?);
            }
            Ok(NbtTag::Compound(values))
        }
        11 => {
            let len = usize::try_from(read_i32(reader)?)
                .map_err(|_| StorageError::InvalidData("negative int array length".to_string()))?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(read_i32(reader)?);
            }
            Ok(NbtTag::IntArray(values))
        }
        _ => Err(StorageError::InvalidData(format!(
            "unsupported nbt tag type {tag_type}"
        ))),
    }
}

const fn tag_type(tag: &NbtTag) -> u8 {
    match tag {
        NbtTag::Byte(_) => 1,
        NbtTag::Short(_) => 2,
        NbtTag::Int(_) => 3,
        NbtTag::Long(_) => 4,
        NbtTag::Float(_) => 5,
        NbtTag::Double(_) => 6,
        NbtTag::ByteArray(_) => 7,
        NbtTag::String(_) => 8,
        NbtTag::List(_, _) => 9,
        NbtTag::Compound(_) => 10,
        NbtTag::IntArray(_) => 11,
    }
}

fn read_u8(reader: &mut impl Read) -> Result<u8, StorageError> {
    let mut bytes = [0_u8; 1];
    reader.read_exact(&mut bytes)?;
    Ok(bytes[0])
}

fn read_i8(reader: &mut impl Read) -> Result<i8, StorageError> {
    Ok(i8::from_be_bytes([read_u8(reader)?]))
}

fn read_i16(reader: &mut impl Read) -> Result<i16, StorageError> {
    let mut bytes = [0_u8; 2];
    reader.read_exact(&mut bytes)?;
    Ok(i16::from_be_bytes(bytes))
}

fn read_i32(reader: &mut impl Read) -> Result<i32, StorageError> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(i32::from_be_bytes(bytes))
}

fn read_i64(reader: &mut impl Read) -> Result<i64, StorageError> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(i64::from_be_bytes(bytes))
}

fn read_f32(reader: &mut impl Read) -> Result<f32, StorageError> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(f32::from_be_bytes(bytes))
}

fn read_f64(reader: &mut impl Read) -> Result<f64, StorageError> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(f64::from_be_bytes(bytes))
}

fn read_string_u16(reader: &mut impl Read) -> Result<String, StorageError> {
    let mut len_bytes = [0_u8; 2];
    reader.read_exact(&mut len_bytes)?;
    let len = usize::from(u16::from_be_bytes(len_bytes));
    let mut bytes = vec![0_u8; len];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes)
        .map_err(|_| StorageError::InvalidData("invalid utf-8 string".to_string()))
}

fn write_i16(writer: &mut impl Write, value: i16) -> Result<(), StorageError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_u8(writer: &mut impl Write, value: u8) -> Result<(), StorageError> {
    writer.write_all(&[value])?;
    Ok(())
}

fn write_i8(writer: &mut impl Write, value: i8) -> Result<(), StorageError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_i32(writer: &mut impl Write, value: i32) -> Result<(), StorageError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_i64(writer: &mut impl Write, value: i64) -> Result<(), StorageError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_f32(writer: &mut impl Write, value: f32) -> Result<(), StorageError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_f64(writer: &mut impl Write, value: f64) -> Result<(), StorageError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn write_string_u16(writer: &mut impl Write, value: &str) -> Result<(), StorageError> {
    let len = u16::try_from(value.len())
        .map_err(|_| StorageError::InvalidData("nbt string too long".to_string()))?;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(value.as_bytes())?;
    Ok(())
}
