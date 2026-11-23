//! Shared NBT helpers.
//!
//! Both editions decode into [`bedrock_nbt::NbtTag`], so conversion code only
//! rewrites one tree. Bedrock payloads use the vendored little-endian codec;
//! Java payloads use [`crate::je::nbt`], a big-endian codec plus Java's modified
//! UTF-8 string encoding.

use bedrock_nbt::NbtTag;
use indexmap::IndexMap;

/// Ergonomic accessors over [`NbtTag`]. Non-compound receivers return `None`.
pub trait NbtExt {
    /// Borrows the compound entries when this tag is a compound.
    fn as_compound(&self) -> Option<&IndexMap<String, NbtTag>>;
    /// Mutably borrows the compound entries when this tag is a compound.
    fn as_compound_mut(&mut self) -> Option<&mut IndexMap<String, NbtTag>>;
    /// Borrows a field of a compound.
    fn get(&self, key: &str) -> Option<&NbtTag>;
    /// Mutably borrows a field of a compound.
    fn get_mut(&mut self, key: &str) -> Option<&mut NbtTag>;
    /// Inserts or replaces a compound field.
    fn set(&mut self, key: impl Into<String>, value: NbtTag) -> Option<NbtTag>;
    /// Removes and returns a compound field.
    fn remove(&mut self, key: &str) -> Option<NbtTag>;
    /// Reads a string field.
    fn get_str(&self, key: &str) -> Option<&str>;
    /// Reads a numeric field as `i64` (byte/short/int/long).
    fn get_int(&self, key: &str) -> Option<i64>;
    /// Reads a numeric field as `f64` (float/double, or any integer).
    fn get_f64(&self, key: &str) -> Option<f64>;
    /// Reads a byte field as a boolean (`0` = false, anything else = true).
    fn get_bool(&self, key: &str) -> Option<bool>;
    /// Reads a numeric list field.
    fn get_list(&self, key: &str) -> Option<&[NbtTag]>;
    /// Reads a `i32` array field.
    fn get_i32_array(&self, key: &str) -> Option<&[i32]>;
    /// Reads a `i64` array field.
    fn get_i64_array(&self, key: &str) -> Option<&[i64]>;
}

impl NbtExt for NbtTag {
    fn as_compound(&self) -> Option<&IndexMap<String, NbtTag>> {
        match self {
            Self::Compound(entries) => Some(entries),
            _ => None,
        }
    }

    fn as_compound_mut(&mut self) -> Option<&mut IndexMap<String, NbtTag>> {
        match self {
            Self::Compound(entries) => Some(entries),
            _ => None,
        }
    }

    fn get(&self, key: &str) -> Option<&NbtTag> {
        self.as_compound().and_then(|entries| entries.get(key))
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut NbtTag> {
        self.as_compound_mut().and_then(|entries| entries.get_mut(key))
    }

    fn set(&mut self, key: impl Into<String>, value: NbtTag) -> Option<NbtTag> {
        self.as_compound_mut()
            .and_then(|entries| entries.insert(key.into(), value))
    }

    fn remove(&mut self, key: &str) -> Option<NbtTag> {
        self.as_compound_mut().and_then(|entries| entries.shift_remove(key))
    }

    fn get_str(&self, key: &str) -> Option<&str> {
        field_str(self.get(key))
    }

    fn get_int(&self, key: &str) -> Option<i64> {
        field_int(self.get(key))
    }

    fn get_f64(&self, key: &str) -> Option<f64> {
        field_f64(self.get(key))
    }

    fn get_bool(&self, key: &str) -> Option<bool> {
        field_bool(self.get(key))
    }

    fn get_list(&self, key: &str) -> Option<&[NbtTag]> {
        field_list(self.get(key))
    }

    fn get_i32_array(&self, key: &str) -> Option<&[i32]> {
        field_i32_array(self.get(key))
    }

    fn get_i64_array(&self, key: &str) -> Option<&[i64]> {
        field_i64_array(self.get(key))
    }
}

/// The same accessors for a compound map, so callers that already destructured
/// a `Compound` do not have to rebuild a tag.
impl NbtExt for IndexMap<String, NbtTag> {
    fn as_compound(&self) -> Option<&IndexMap<String, NbtTag>> {
        Some(self)
    }

    fn as_compound_mut(&mut self) -> Option<&mut IndexMap<String, NbtTag>> {
        Some(self)
    }

    fn get(&self, key: &str) -> Option<&NbtTag> {
        IndexMap::get(self, key)
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut NbtTag> {
        IndexMap::get_mut(self, key)
    }

    fn set(&mut self, key: impl Into<String>, value: NbtTag) -> Option<NbtTag> {
        IndexMap::insert(self, key.into(), value)
    }

    fn remove(&mut self, key: &str) -> Option<NbtTag> {
        IndexMap::shift_remove(self, key)
    }

    fn get_str(&self, key: &str) -> Option<&str> {
        field_str(IndexMap::get(self, key))
    }

    fn get_int(&self, key: &str) -> Option<i64> {
        field_int(IndexMap::get(self, key))
    }

    fn get_f64(&self, key: &str) -> Option<f64> {
        field_f64(IndexMap::get(self, key))
    }

    fn get_bool(&self, key: &str) -> Option<bool> {
        field_bool(IndexMap::get(self, key))
    }

    fn get_list(&self, key: &str) -> Option<&[NbtTag]> {
        field_list(IndexMap::get(self, key))
    }

    fn get_i32_array(&self, key: &str) -> Option<&[i32]> {
        field_i32_array(IndexMap::get(self, key))
    }

    fn get_i64_array(&self, key: &str) -> Option<&[i64]> {
        field_i64_array(IndexMap::get(self, key))
    }
}

fn field_str(tag: Option<&NbtTag>) -> Option<&str> {
    match tag? {
        NbtTag::String(value) => Some(value),
        _ => None,
    }
}

fn field_int(tag: Option<&NbtTag>) -> Option<i64> {
    match tag? {
        NbtTag::Byte(value) => Some(i64::from(*value)),
        NbtTag::Short(value) => Some(i64::from(*value)),
        NbtTag::Int(value) => Some(i64::from(*value)),
        NbtTag::Long(value) => Some(*value),
        _ => None,
    }
}

fn field_f64(tag: Option<&NbtTag>) -> Option<f64> {
    match tag? {
        NbtTag::Float(value) => Some(f64::from(*value)),
        NbtTag::Double(value) => Some(*value),
        NbtTag::Byte(value) => Some(f64::from(*value)),
        NbtTag::Short(value) => Some(f64::from(*value)),
        NbtTag::Int(value) => Some(f64::from(*value)),
        NbtTag::Long(value) => Some(*value as f64),
        _ => None,
    }
}

fn field_bool(tag: Option<&NbtTag>) -> Option<bool> {
    match tag? {
        NbtTag::Byte(value) => Some(*value != 0),
        _ => None,
    }
}

fn field_list(tag: Option<&NbtTag>) -> Option<&[NbtTag]> {
    match tag? {
        NbtTag::List(values) => Some(values),
        _ => None,
    }
}

fn field_i32_array(tag: Option<&NbtTag>) -> Option<&[i32]> {
    match tag? {
        NbtTag::IntArray(values) => Some(values),
        _ => None,
    }
}

fn field_i64_array(tag: Option<&NbtTag>) -> Option<&[i64]> {
    match tag? {
        NbtTag::LongArray(values) => Some(values),
        _ => None,
    }
}

/// Convenience accessors for a bare tag (not a field of a compound).
pub trait TagValue {
    /// Reads a string tag.
    fn as_str_value(&self) -> Option<&str>;
    /// Reads a numeric tag as `i64`.
    fn as_i64_value(&self) -> Option<i64>;
    /// Reads a numeric tag as `f64`.
    fn as_f64_value(&self) -> Option<f64>;
}

impl TagValue for NbtTag {
    fn as_str_value(&self) -> Option<&str> {
        match self {
            NbtTag::String(value) => Some(value),
            _ => None,
        }
    }

    fn as_i64_value(&self) -> Option<i64> {
        match self {
            NbtTag::Byte(value) => Some(i64::from(*value)),
            NbtTag::Short(value) => Some(i64::from(*value)),
            NbtTag::Int(value) => Some(i64::from(*value)),
            NbtTag::Long(value) => Some(*value),
            _ => None,
        }
    }

    fn as_f64_value(&self) -> Option<f64> {
        match self {
            NbtTag::Float(value) => Some(f64::from(*value)),
            NbtTag::Double(value) => Some(*value),
            NbtTag::Byte(value) => Some(f64::from(*value)),
            NbtTag::Short(value) => Some(f64::from(*value)),
            NbtTag::Int(value) => Some(f64::from(*value)),
            NbtTag::Long(value) => Some(*value as f64),
            _ => None,
        }
    }
}

/// Builds an empty compound tag.
#[must_use]
pub fn empty_compound() -> NbtTag {
    NbtTag::Compound(IndexMap::new())
}

/// Builds a compound tag from `(key, value)` pairs.
#[must_use]
pub fn compound<I, K>(entries: I) -> NbtTag
where
    I: IntoIterator<Item = (K, NbtTag)>,
    K: Into<String>,
{
    NbtTag::Compound(entries.into_iter().map(|(key, value)| (key.into(), value)).collect())
}

/// Builds a string tag.
#[must_use]
pub fn tag_string(value: impl Into<String>) -> NbtTag {
    NbtTag::String(value.into())
}

/// Builds a homogeneous list tag.
#[must_use]
pub fn tag_list(values: Vec<NbtTag>) -> NbtTag {
    NbtTag::List(values)
}

pub use crate::je::nbt::{decode_mutf8, encode_mutf8};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutf8_round_trips_basic_and_supplementary_text() {
        for text in ["hello", "中文世界", "a\u{0}b", "🎉 fireworks"] {
            let mut bytes = Vec::new();
            encode_mutf8(text, &mut bytes);
            assert_eq!(decode_mutf8(&bytes), text);
        }
    }

    #[test]
    fn mutf8_encodes_nul_and_surrogates_like_java() {
        let mut bytes = Vec::new();
        encode_mutf8("a\0", &mut bytes);
        assert_eq!(bytes, vec![b'a', 0xC0, 0x80]);

        let mut bytes = Vec::new();
        encode_mutf8("🎉", &mut bytes);
        assert_eq!(bytes.len(), 6);
    }

    #[test]
    fn compound_helpers_read_and_write_fields() {
        let mut tag = compound([
            ("Name", tag_string("minecraft:stone")),
            ("Count", NbtTag::Byte(3)),
        ]);
        assert_eq!(tag.get_str("Name"), Some("minecraft:stone"));
        assert_eq!(tag.get_int("Count"), Some(3));
        assert_eq!(tag.get_bool("Count"), Some(true));
        tag.set("Count", NbtTag::Byte(4));
        assert_eq!(tag.get_int("Count"), Some(4));
        assert!(tag.remove("Name").is_some());
        assert!(tag.get("Name").is_none());
    }
}
