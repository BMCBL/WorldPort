"""Generate cross-edition id tables from the MIT-licensed Chunker resolvers.

Run after `git clone --depth 1 --filter=blob:none --sparse https://github.com/HiveGamesOSS/Chunker _refs/chunker`
and `git -C _refs/chunker sparse-checkout set cli`.
"""
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASE = ROOT / "_refs/chunker/cli/src/main/java/com/hivemc/chunker/conversion/encoding"
OUT = ROOT / "core/src/mapping"


def load(path, pattern):
    text = path.read_text(encoding="utf-8")
    return [(m.group(1), m.group(2)) for m in re.finditer(pattern, text)]


def join(java_pairs, be_pairs, be_is_numeric):
    """Joins (enum, value) pairs from both editions by the internal enum name."""
    if be_is_numeric:
        be_map = {enum: int(value) for enum, value in be_pairs}
    else:
        be_map = {enum: value for enum, value in be_pairs}
    return sorted({(be_map[enum], name) for enum, name in java_pairs if enum in be_map})


def write(path, lines):
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


# ---------------------------------------------------------------- entities
je = load(
    BASE / "java/base/resolver/entity/JavaEntityTypeResolver.java",
    r'mapping\.put\(ChunkerVanillaEntityType\.(\w+),\s*"([^"]+)"\)',
)
be = load(
    BASE / "bedrock/base/resolver/entity/BedrockEntityTypeResolver.java",
    r'mapping\.put\(ChunkerVanillaEntityType\.(\w+),\s*"([^"]+)"\)',
)
be_by_enum = {enum: name for enum, name in be}
pairs = sorted({(name, be_by_enum[enum]) for enum, name in je if enum in be_by_enum})
aliases = [(j, b) for j, b in pairs if j != b]
print(f"entities: je={len(je)} be={len(be)} pairs={len(pairs)} aliases={len(aliases)}")

lines = [
    "//! Entity identifier translation between Java and Bedrock.",
    "//!",
    "//! Generated from the MIT-licensed Chunker entity type resolvers, which track",
    "//! the vanilla identifier renames (boats, xp orbs, thrown items, ...).",
    "",
    "/// `(Java id, Bedrock id)` for entity types whose identifiers differ.",
    "pub const ENTITY_ALIASES: &[(&str, &str)] = &[",
]
lines += [f'    ("{j}", "{b}"),' for j, b in aliases]
lines += [
    "];",
    "",
    "/// Bedrock identifier for a Java entity id (identity when unchanged).",
    "#[must_use]",
    "pub fn entity_to_bedrock(java: &str) -> String {",
    '    let short = java.strip_prefix("minecraft:").unwrap_or(java);',
    '    let full = format!("minecraft:{short}");',
    "    ENTITY_ALIASES",
    "        .iter()",
    "        .find(|(source, _)| *source == full)",
    "        .map(|(_, target)| (*target).to_string())",
    "        .unwrap_or(full)",
    "}",
    "",
    "/// Java identifier for a Bedrock entity id (identity when unchanged).",
    "#[must_use]",
    "pub fn entity_to_java(bedrock: &str) -> String {",
    '    let short = bedrock.strip_prefix("minecraft:").unwrap_or(bedrock);',
    '    let full = format!("minecraft:{short}");',
    "    ENTITY_ALIASES",
    "        .iter()",
    "        .find(|(_, target)| *target == full)",
    "        .map(|(source, _)| (*source).to_string())",
    "        .unwrap_or(full)",
    "}",
    "",
    "#[cfg(test)]",
    "mod tests {",
    "    use super::*;",
    "",
    "    #[test]",
    "    fn renames_known_entities() {",
    '        assert_eq!(entity_to_bedrock("minecraft:experience_orb"), "minecraft:xp_orb");',
    '        assert_eq!(entity_to_bedrock("minecraft:oak_boat"), "minecraft:boat");',
    '        assert_eq!(entity_to_java("minecraft:xp_orb"), "minecraft:experience_orb");',
    '        assert_eq!(entity_to_bedrock("minecraft:zombie"), "minecraft:zombie");',
    "    }",
    "}",
]
write(OUT / "entities.rs", lines)

# ------------------------------------------------------ numeric id tables
def numeric_table(name, doc, java_file, be_file, java_pat, be_pat, u16_ids=True):
    java_pairs = load(BASE / java_file, java_pat)
    be_pairs = load(BASE / be_file, be_pat)
    pairs = join(java_pairs, be_pairs, be_is_numeric=True)
    print(f"{name}: je={len(java_pairs)} be={len(be_pairs)} pairs={len(pairs)}")
    ty = "u16" if u16_ids else "u32"
    lines = [
        f"//! {doc}",
        "//!",
        "//! Generated from the MIT-licensed Chunker resolvers.",
        "",
        f"/// `(Bedrock numeric id, Java id)` sorted by id.",
        f"pub const {name.upper()}: &[({ty}, &str)] = &[",
    ]
    lines += [f'    ({value}, "{java_id}"),' for value, java_id in pairs]
    lines += [
        "];",
        "",
        "/// Bedrock numeric id for a Java identifier.",
        "#[must_use]",
        f"pub fn {name}_id(java: &str) -> Option<{ty}> {{",
        '    let short = java.strip_prefix("minecraft:").unwrap_or(java);',
        '    let full = format!("minecraft:{short}");',
        f"    {name.upper()}",
        "        .iter()",
        "        .find(|(_, name)| *name == full)",
        "        .map(|(value, _)| *value)",
        "}",
        "",
        "/// Java identifier for a Bedrock numeric id.",
        "#[must_use]",
        f"pub fn {name}_name(id: {ty}) -> Option<&'static str> {{",
        f"    {name.upper()}",
        "        .iter()",
        "        .find(|(value, _)| *value == id)",
        "        .map(|(_, name)| *name)",
        "}",
        "",
        "#[cfg(test)]",
        "mod tests {",
        "    use super::*;",
        "",
        "    #[test]",
        "    fn round_trips_some_ids() {",
        f"        for (id, name) in {name.upper()}.iter().take(5) {{",
        f"            assert_eq!({name}_id(name), Some(*id));",
        f"            assert_eq!({name}_name(*id), Some(*name));",
        "        }",
        "    }",
        "}",
    ]
    write(OUT / f"{name}s.rs", lines)


numeric_table(
    "enchantment",
    "Enchantment id translation between Java names and Bedrock numeric ids.",
    "java/base/resolver/itemstack/JavaEnchantmentResolver.java",
    "bedrock/base/resolver/itemstack/BedrockEnchantmentIDResolver.java",
    r'mapping\.put\(ChunkerEnchantmentType\.(\w+),\s*"([^"]+)"\)',
    r'mapping\.put\(ChunkerEnchantmentType\.(\w+),\s*(\d+)\)',
)

numeric_table(
    "effect",
    "Status effect id translation between Java names and Bedrock numeric ids.",
    "java/base/resolver/itemstack/JavaEffectResolver.java",
    "bedrock/base/resolver/itemstack/BedrockEffectIDResolver.java",
    r'mapping\.put\(ChunkerEffectType\.(\w+),\s*"([^"]+)"\)',
    r'mapping\.put\(ChunkerEffectType\.(\w+),\s*(\d+)\)',
)

numeric_table(
    "potion",
    "Potion id translation between Java names and Bedrock numeric ids.",
    "java/base/resolver/itemstack/JavaPotionTypeResolver.java",
    "bedrock/base/resolver/itemstack/BedrockPotionIDResolver.java",
    r'mapping\.put\(ChunkerPotionType\.(\w+),\s*"([^"]+)"\)',
    r'mapping\.put\(ChunkerPotionType\.(\w+),\s*(\d+)\)',
    u16_ids=False,
)
print("written")
