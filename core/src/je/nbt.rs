//! Big-endian NBT codec for Java Edition payloads.
//!
//! Decodes into the same [`NbtTag`] tree used for Bedrock payloads, so the
//! conversion code never converts between two NBT models. Strings use Java's
//! modified UTF-8 (see [`crate::nbt::decode_mutf8`]).
//!
//! Round-tripping is tolerant on the read side (negative list lengths and
//! standard four-byte UTF-8 are accepted) and strict on the write side: list
//! elements must be homogeneous, which is what Java requires.

use std::io::{Cursor, Read, Write};
use std::path::Path;

use bedrock_nbt::NbtTag;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use indexmap::IndexMap;

use crate::error::{Result, WorldPortError, io_result};
use crate::nbt::{decode_mutf8, encode_mutf8};

const TAG_END: u8 = 0;
const TAG_BYTE: u8 = 1;
const TAG_SHORT: u8 = 2;
const TAG_INT: u8 = 3;
const TAG_LONG: u8 = 4;
const TAG_FLOAT: u8 = 5;
const TAG_DOUBLE: u8 = 6;
const TAG_BYTE_ARRAY: u8 = 7;
const TAG_STRING: u8 = 8;
const TAG_LIST: u8 = 9;
const TAG_COMPOUND: u8 = 10;
const TAG_INT_ARRAY: u8 = 11;
const TAG_LONG_ARRAY: u8 = 12;

/// Maximum nesting depth accepted while decoding.
const MAX_DEPTH: u16 = 512;

/// A decoded named root compound.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedTag {
    /// Root compound name. Vanilla writes `""` for `level.dat`, chunks and player files.
    pub name: String,
    /// Root compound payload.
    pub tag: NbtTag,
}

/// Decodes a complete named root compound.
pub fn decode(data: &[u8]) -> Result<NamedTag> {
    let mut reader = Reader::new(data);
    let id = reader.u8()?;
    if id == TAG_END {
        return Ok(NamedTag {
            name: String::new(),
            tag: NbtTag::Compound(IndexMap::new()),
        });
    }
    if id != TAG_COMPOUND {
        return Err(WorldPortError::nbt(format!(
            "root tag must be a compound, found id {id}"
        )));
    }
    let name = reader.mutf8_string()?;
    let tag = reader.payload(TAG_COMPOUND, 0)?;
    Ok(NamedTag { name, tag })
}

/// Decodes a root compound that has no name (network-style payloads).
pub fn decode_unnamed(data: &[u8]) -> Result<NbtTag> {
    let mut reader = Reader::new(data);
    let id = reader.u8()?;
    if id == TAG_END {
        return Ok(NbtTag::Compound(IndexMap::new()));
    }
    if id != TAG_COMPOUND {
        return Err(WorldPortError::nbt(format!(
            "root tag must be a compound, found id {id}"
        )));
    }
    reader.payload(TAG_COMPOUND, 0)
}

/// Encodes a named root compound.
pub fn encode(name: &str, tag: &NbtTag) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(512);
    encode_into(&mut out, name, tag)?;
    Ok(out)
}

/// Encodes a root tag into an existing buffer, which the caller can recycle
/// across chunks to avoid one allocation per converted chunk.
pub fn encode_into(out: &mut Vec<u8>, name: &str, tag: &NbtTag) -> Result<()> {
    if !matches!(tag, NbtTag::Compound(_)) {
        return Err(WorldPortError::nbt("root tag must be a compound"));
    }
    out.clear();
    out.push(TAG_COMPOUND);
    write_mutf8_string(out, name)?;
    write_payload(out, tag)?;
    Ok(())
}

/// Reads a gzip-compressed `level.dat`-style file.
pub fn read_gzip(path: &Path) -> Result<NamedTag> {
    let bytes = io_result(path, std::fs::read(path))?;
    decode_gzip(&bytes)
        .map_err(|error| WorldPortError::nbt(format!("{}: {error}", path.display())))
}

/// Decodes gzip-compressed NBT bytes.
pub fn decode_gzip(bytes: &[u8]) -> Result<NamedTag> {
    let mut decoder = GzDecoder::new(Cursor::new(bytes));
    let mut decoded = Vec::with_capacity(bytes.len().saturating_mul(2).max(1024));
    decoder
        .read_to_end(&mut decoded)
        .map_err(|error| WorldPortError::nbt(format!("gzip: {error}")))?;
    decode(&decoded)
}

/// Writes a gzip-compressed NBT file (Java's default for `level.dat` and `playerdata`).
pub fn write_gzip(path: &Path, name: &str, tag: &NbtTag) -> Result<()> {
    let payload = encode(name, tag)?;
    let mut encoder = GzEncoder::new(Vec::with_capacity(payload.len()), Compression::default());
    encoder
        .write_all(&payload)
        .map_err(|error| WorldPortError::nbt(format!("gzip: {error}")))?;
    let compressed = encoder
        .finish()
        .map_err(|error| WorldPortError::nbt(format!("gzip: {error}")))?;
    if let Some(parent) = path.parent() {
        io_result(parent, std::fs::create_dir_all(parent))?;
    }
    io_result(path, std::fs::write(path, compressed))
}

fn write_payload(out: &mut Vec<u8>, tag: &NbtTag) -> Result<()> {
    match tag {
        NbtTag::End => {}
        NbtTag::Byte(value) => out.push(*value as u8),
        NbtTag::Short(value) => out.extend_from_slice(&value.to_be_bytes()),
        NbtTag::Int(value) => out.extend_from_slice(&value.to_be_bytes()),
        NbtTag::Long(value) => out.extend_from_slice(&value.to_be_bytes()),
        NbtTag::Float(value) => out.extend_from_slice(&value.to_bits().to_be_bytes()),
        NbtTag::Double(value) => out.extend_from_slice(&value.to_bits().to_be_bytes()),
        NbtTag::ByteArray(values) => {
            write_len(out, values.len())?;
            out.extend(values.iter().map(|value| *value as u8));
        }
        NbtTag::String(value) => write_mutf8_string(out, value)?,
        // Java NBT has no non-UTF-8 string type; Bedrock `StorageKey` values are
        // stripped during conversion, and anything left is lossily widened.
        NbtTag::ByteString(value) => write_mutf8_string(out, &String::from_utf8_lossy(value))?,
        NbtTag::List(values) => {
            let element = values.first().map_or(TAG_END, tag_id);
            if values.iter().any(|value| tag_id(value) != element) {
                return Err(WorldPortError::nbt(
                    "list elements must share one tag type (Java NBT requirement)",
                ));
            }
            out.push(element);
            write_len(out, values.len())?;
            for value in values {
                write_payload(out, value)?;
            }
        }
        NbtTag::Compound(entries) => {
            for (key, value) in entries {
                out.push(tag_id(value));
                write_mutf8_string(out, key)?;
                write_payload(out, value)?;
            }
            out.push(TAG_END);
        }
        NbtTag::IntArray(values) => {
            write_len(out, values.len())?;
            for value in values {
                out.extend_from_slice(&value.to_be_bytes());
            }
        }
        NbtTag::LongArray(values) => {
            write_len(out, values.len())?;
            for value in values {
                out.extend_from_slice(&value.to_be_bytes());
            }
        }
        // Java has no short array; widen so Bedrock-only data still writes.
        NbtTag::ShortArray(values) => {
            write_len(out, values.len())?;
            for value in values {
                out.extend_from_slice(&i32::from(*value).to_be_bytes());
            }
        }
    }
    Ok(())
}

const fn tag_id(tag: &NbtTag) -> u8 {
    match tag {
        NbtTag::End => TAG_END,
        NbtTag::Byte(_) => TAG_BYTE,
        NbtTag::Short(_) => TAG_SHORT,
        NbtTag::Int(_) => TAG_INT,
        NbtTag::Long(_) => TAG_LONG,
        NbtTag::Float(_) => TAG_FLOAT,
        NbtTag::Double(_) => TAG_DOUBLE,
        NbtTag::ByteArray(_) => TAG_BYTE_ARRAY,
        NbtTag::String(_) | NbtTag::ByteString(_) => TAG_STRING,
        NbtTag::List(_) => TAG_LIST,
        NbtTag::Compound(_) => TAG_COMPOUND,
        NbtTag::IntArray(_) | NbtTag::ShortArray(_) => TAG_INT_ARRAY,
        NbtTag::LongArray(_) => TAG_LONG_ARRAY,
    }
}

fn write_len(out: &mut Vec<u8>, length: usize) -> Result<()> {
    let length = i32::try_from(length)
        .map_err(|_| WorldPortError::nbt("collection length exceeds i32::MAX"))?;
    out.extend_from_slice(&length.to_be_bytes());
    Ok(())
}

fn write_mutf8_string(out: &mut Vec<u8>, value: &str) -> Result<()> {
    let mut encoded = Vec::with_capacity(value.len());
    encode_mutf8(value, &mut encoded);
    let length = u16::try_from(encoded.len())
        .map_err(|_| WorldPortError::nbt("string exceeds 65535 encoded bytes"))?;
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&encoded);
    Ok(())
}

struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| WorldPortError::nbt("nbt offset overflow"))?;
        let slice = self
            .data
            .get(self.offset..end)
            .ok_or_else(|| WorldPortError::nbt("unexpected end of nbt payload"))?;
        self.offset = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().expect("2 bytes")))
    }

    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().expect("2 bytes")))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }

    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().expect("8 bytes")))
    }

    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(u32::from_be_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        )))
    }

    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(u64::from_be_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        )))
    }

    fn mutf8_string(&mut self) -> Result<String> {
        let length = usize::from(self.u16()?);
        Ok(decode_mutf8(self.take(length)?))
    }

    fn payload(&mut self, id: u8, depth: u16) -> Result<NbtTag> {
        if depth > MAX_DEPTH {
            return Err(WorldPortError::nbt("nbt nesting is too deep"));
        }
        Ok(match id {
            TAG_END => NbtTag::End,
            TAG_BYTE => NbtTag::Byte(self.i8()?),
            TAG_SHORT => NbtTag::Short(self.i16()?),
            TAG_INT => NbtTag::Int(self.i32()?),
            TAG_LONG => NbtTag::Long(self.i64()?),
            TAG_FLOAT => NbtTag::Float(self.f32()?),
            TAG_DOUBLE => NbtTag::Double(self.f64()?),
            TAG_BYTE_ARRAY => {
                let length = self.list_len()?;
                let bytes = self.take(length)?;
                NbtTag::ByteArray(bytes.iter().map(|value| *value as i8).collect())
            }
            TAG_STRING => NbtTag::String(self.mutf8_string()?),
            TAG_LIST => {
                let element = self.u8()?;
                let length = self.list_len()?;
                if element == TAG_END {
                    if length != 0 {
                        return Err(WorldPortError::nbt("end-typed list with entries"));
                    }
                    NbtTag::List(Vec::new())
                } else {
                    let mut values = Vec::with_capacity(length.min(4096));
                    for _ in 0..length {
                        values.push(self.payload(element, depth + 1)?);
                    }
                    NbtTag::List(values)
                }
            }
            TAG_COMPOUND => {
                let mut entries = IndexMap::new();
                loop {
                    let element = self.u8()?;
                    if element == TAG_END {
                        break;
                    }
                    let key = self.mutf8_string()?;
                    entries.insert(key, self.payload(element, depth + 1)?);
                }
                NbtTag::Compound(entries)
            }
            TAG_INT_ARRAY => {
                let length = self.list_len()?;
                let mut values = Vec::with_capacity(length.min(4096));
                for _ in 0..length {
                    values.push(self.i32()?);
                }
                NbtTag::IntArray(values)
            }
            TAG_LONG_ARRAY => {
                let length = self.list_len()?;
                let mut values = Vec::with_capacity(length.min(4096));
                for _ in 0..length {
                    values.push(self.i64()?);
                }
                NbtTag::LongArray(values)
            }
            other => {
                return Err(WorldPortError::nbt(format!("unknown nbt tag id {other}")));
            }
        })
    }

    /// Reads a collection length. Negative values are tolerated as zero, which
    /// is what several third-party writers emit for empty lists.
    fn list_len(&mut self) -> Result<usize> {
        let raw = self.i32()?;
        Ok(usize::try_from(raw).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::{NbtExt, compound, tag_list, tag_string};

    fn sample() -> NbtTag {
        compound([
            ("DataVersion", NbtTag::Int(4189)),
            ("LevelName", tag_string("测试世界 🎉")),
            (
                "Sections",
                tag_list(vec![compound([
                    ("Y", NbtTag::Byte(-4)),
                    ("block_states", compound([("palette", tag_list(vec![
                        compound([("Name", tag_string("minecraft:air"))]),
                        compound([
                            ("Name", tag_string("minecraft:stone")),
                            ("Properties", compound([("axis", tag_string("y"))])),
                        ]),
                    ]))]))
                ])]),
            ),
            ("Empty", tag_list(Vec::new())),
            ("Bytes", NbtTag::ByteArray(vec![1, -2, 3])),
            ("Ints", NbtTag::IntArray(vec![1, 2, 3])),
            ("Longs", NbtTag::LongArray(vec![1 << 40, -5])),
        ])
    }

    #[test]
    fn named_root_round_trips() {
        let tag = sample();
        let bytes = encode("", &tag).expect("encode");
        let decoded = decode(&bytes).expect("decode");
        assert_eq!(decoded.tag, tag);
    }

    #[test]
    fn gzip_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("level.dat");
        write_gzip(&path, "", &sample()).expect("write");
        let decoded = read_gzip(&path).expect("read");
        assert_eq!(decoded.tag, sample());
    }

    #[test]
    fn rejects_heterogeneous_list_on_write() {
        let tag = compound([(
            "Bad",
            tag_list(vec![NbtTag::Byte(1), NbtTag::Int(2)]),
        )]);
        assert!(encode("", &tag).is_err());
    }

    #[test]
    fn tolerates_negative_list_length() {
        // Root compound: name "", field "L" of type list, element byte, length -1.
        let mut bytes = vec![TAG_COMPOUND, 0, 0];
        bytes.push(TAG_LIST);
        bytes.extend_from_slice(&[0, 1, b'L']);
        bytes.push(TAG_BYTE);
        bytes.extend_from_slice(&(-1_i32).to_be_bytes());
        bytes.push(TAG_END);
        let decoded = decode(&bytes).expect("decode");
        assert_eq!(decoded.tag.get_list("L").map(|values| values.len()), Some(0));
    }
}
