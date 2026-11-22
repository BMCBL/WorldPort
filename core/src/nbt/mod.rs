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
        match self.get(key)? {
            NbtTag::String(value) => Some(value),
            _ => None,
        }
    }

    fn get_int(&self, key: &str) -> Option<i64> {
        match self.get(key)? {
            NbtTag::Byte(value) => Some(i64::from(*value)),
            NbtTag::Short(value) => Some(i64::from(*value)),
            NbtTag::Int(value) => Some(i64::from(*value)),
            NbtTag::Long(value) => Some(*value),
            _ => None,
        }
    }

    fn get_f64(&self, key: &str) -> Option<f64> {
        match self.get(key)? {
            NbtTag::Float(value) => Some(f64::from(*value)),
            NbtTag::Double(value) => Some(*value),
            NbtTag::Byte(value) => Some(f64::from(*value)),
            NbtTag::Short(value) => Some(f64::from(*value)),
            NbtTag::Int(value) => Some(f64::from(*value)),
            NbtTag::Long(value) => Some(*value as f64),
            _ => None,
        }
    }

    fn get_bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            NbtTag::Byte(value) => Some(*value != 0),
            _ => None,
        }
    }

    fn get_list(&self, key: &str) -> Option<&[NbtTag]> {
        match self.get(key)? {
            NbtTag::List(values) => Some(values),
            _ => None,
        }
    }

    fn get_i32_array(&self, key: &str) -> Option<&[i32]> {
        match self.get(key)? {
            NbtTag::IntArray(values) => Some(values),
            _ => None,
        }
    }

    fn get_i64_array(&self, key: &str) -> Option<&[i64]> {
        match self.get(key)? {
            NbtTag::LongArray(values) => Some(values),
            _ => None,
        }
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

/// Java's modified UTF-8 encoding, used by every string inside Java NBT.
///
/// Differs from standard UTF-8 in two ways: `U+0000` is encoded as `C0 80`, and
/// characters outside the BMP are encoded as a UTF-16 surrogate pair (six bytes).
pub fn encode_mutf8(value: &str, out: &mut Vec<u8>) {
    for ch in value.chars() {
        let code = ch as u32;
        if ch == '\0' {
            out.extend_from_slice(&[0xC0, 0x80]);
        } else if code < 0x80 {
            out.push(code as u8);
        } else if code < 0x800 {
            out.push(0xC0 | (code >> 6) as u8);
            out.push(0x80 | (code & 0x3F) as u8);
        } else if code < 0x1_0000 {
            push_three(out, code);
        } else {
            let value = code - 0x1_0000;
            push_three(out, 0xD800 + (value >> 10));
            push_three(out, 0xDC00 + (value & 0x3FF));
        }
    }
}

fn push_three(out: &mut Vec<u8>, unit: u32) {
    out.push(0xE0 | (unit >> 12) as u8);
    out.push(0x80 | ((unit >> 6) & 0x3F) as u8);
    out.push(0x80 | (unit & 0x3F) as u8);
}

/// Decodes Java modified UTF-8. Invalid sequences decode to `U+FFFD`; standard
/// four-byte UTF-8 sequences are also accepted for tolerance.
#[must_use]
pub fn decode_mutf8(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let first = bytes[index];
        if first & 0x80 == 0 {
            out.push(first as char);
            index += 1;
            continue;
        }
        if first & 0xE0 == 0xC0 {
            let Some(&second) = bytes.get(index + 1) else {
                out.push('\u{FFFD}');
                break;
            };
            if first == 0xC0 && second == 0x80 {
                out.push('\0');
            } else {
                let code = (u32::from(first & 0x1F) << 6) | u32::from(second & 0x3F);
                out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
            }
            index += 2;
            continue;
        }
        if first & 0xF0 == 0xE0 {
            let (Some(&second), Some(&third)) = (bytes.get(index + 1), bytes.get(index + 2)) else {
                out.push('\u{FFFD}');
                break;
            };
            let unit = (u32::from(first & 0x0F) << 12)
                | (u32::from(second & 0x3F) << 6)
                | u32::from(third & 0x3F);
            if (0xD800..=0xDBFF).contains(&unit) {
                // Java NBT stores supplementary characters as surrogate pairs.
                if let (Some(&s4), Some(&s5), Some(&s6)) = (
                    bytes.get(index + 3),
                    bytes.get(index + 4),
                    bytes.get(index + 5),
                ) && s4 & 0xF0 == 0xE0
                {
                    let low = (u32::from(s4 & 0x0F) << 12)
                        | (u32::from(s5 & 0x3F) << 6)
                        | u32::from(s6 & 0x3F);
                    if (0xDC00..=0xDFFF).contains(&low) {
                        let code = 0x1_0000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        index += 6;
                        continue;
                    }
                }
                out.push('\u{FFFD}');
            } else {
                out.push(char::from_u32(unit).unwrap_or('\u{FFFD}'));
            }
            index += 3;
            continue;
        }
        if first & 0xF8 == 0xF0 {
            let (Some(&second), Some(&third), Some(&fourth)) = (
                bytes.get(index + 1),
                bytes.get(index + 2),
                bytes.get(index + 3),
            ) else {
                out.push('\u{FFFD}');
                break;
            };
            let code = (u32::from(first & 0x07) << 18)
                | (u32::from(second & 0x3F) << 12)
                | (u32::from(third & 0x3F) << 6)
                | u32::from(fourth & 0x3F);
            out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
            index += 4;
            continue;
        }
        out.push('\u{FFFD}');
        index += 1;
    }
    out
}

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
