#!/usr/bin/env python3
"""Diffs WorldPort's catalogs against Chunker's per-version registries.

Chunker ships the block registries it extracted from the real game
(`_refs/chunker/cli/data/<edition>/<version>/`), which is the closest thing to an
authoritative name list for a version. Comparing them with ours answers two
questions that a roundtrip test cannot:

  * which Bedrock blocks can a modern world contain that we cannot name at all,
  * which block names changed between versions in a way our alias tables miss.

Requires `dump_catalog` output (see `core/examples/dump_catalog.rs`):

    cargo run --example dump_catalog -- bedrock  > our_bedrock.json
    cargo run --example dump_catalog -- 26.2     > our_je_26.2.json

Usage:
    python -I scripts/chunker_gap.py <chunker-data-dir> <our-bedrock.json> [<our-je.json> ...]
"""
from __future__ import annotations

import json
import sys
from collections import Counter
from pathlib import Path


def load_names(path: Path) -> set[str]:
    """Block names from a Chunker registry or from `dump_catalog` output."""
    data = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(data, dict) and "blocks" in data:
        return {block["name"] for block in data["blocks"]}
    return set(data)


def suffix(name: str) -> str:
    return name.split(":", 1)[-1]


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    chunker = Path(sys.argv[1])
    our_bedrock = load_names(Path(sys.argv[2]))
    our_java = [load_names(Path(p)) for p in sys.argv[3:]]

    bedrock_versions = sorted((chunker / "bedrock").iterdir())
    latest = bedrock_versions[-1]
    reference = load_names(latest / "block_states.json")
    print(f"reference: Bedrock {latest.name} - {len(reference)} blocks")
    print(f"our Bedrock catalog: {len(our_bedrock)} blocks")

    missing = sorted(reference - our_bedrock)
    extra = sorted(our_bedrock - reference)
    print(f"\nmissing from our catalog: {len(missing)}")
    print(f"only in our catalog (legacy ids we still read): {len(extra)}")

    # A block we cannot name is a block that becomes air. Grouping by the last
    # suffix component shows whether the gap is one family or many.
    families = Counter()
    for name in missing:
        short = suffix(name)
        for family in ("_slab", "_stairs", "_wall", "_fence", "_door", "_sign", "_button", "_plate"):
            if short.endswith(family):
                families[family] += 1
                break
        else:
            families["(no family suffix)"] += 1
    print("\nmissing, grouped by family suffix:")
    for family, count in families.most_common():
        print(f"  {count:>4}  {family}")

    # Names that differ only by a prefix are the renames worth aliasing; the rest
    # are genuinely absent.
    java_latest = sorted((chunker / "java").iterdir())[-1]
    java_names = load_names(java_latest / "blocks.json")
    print(f"\nreference: Java {java_latest.name} - {len(java_names)} blocks")
    ours_union = set().union(*our_java) if our_java else set()
    if our_java:
        print(f"our Java catalogs (union): {len(ours_union)} blocks")
        print(f"Java blocks we never see: {len(java_names - ours_union)}")
        unseen = sorted(java_names - ours_union)
        print("  first 15:", ", ".join(unseen[:15]))

    # Cross-edition candidates: a Bedrock name missing from Java whose *suffix*
    # appears in a Java name that ours also lacks. Those are the renames the
    # alias tables should cover, and it is exactly how `chain`/`iron_chain` fell
    # through before.
    candidates = []
    for name in missing:
        short = suffix(name)
        if any(short == suffix(other) for other in java_names):
            continue
        for other in java_names:
            other_suffix = suffix(other)
            if short.endswith(other_suffix) or other_suffix.endswith(short):
                candidates.append((name, other))
                break
    print(f"\nrename candidates (Bedrock name vs a similar Java name): {len(candidates)}")
    for bedrock_name, java_name in candidates[:25]:
        print(f"  {bedrock_name}  <->  {java_name}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
