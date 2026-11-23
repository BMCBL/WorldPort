#!/usr/bin/env python3
"""Bakes Bedrock block catalogs from Chunker's per-version registries.

`bedrock-world` embeds one snapshot of the Bedrock block schema, which is enough
to write the version WorldPort targets but blind to every older world: a 1.12 save
is full of ids that no longer exist, and a 1.21 save predates ids we do emit. The
same shape of problem applies to Java, which already ships one catalog per
supported version.

Chunker extracted `block_states.json` per Bedrock build straight from the game, so
it is the authoritative name list for each one. This turns the ones we bundle into
the same compact form as the Java catalogs, which `core/build.rs` then bakes.

Usage:
    python -I scripts/gen_bedrock_catalogs.py [<chunker-data-dir>]
"""
from __future__ import annotations

import gzip
import json
import sys
from collections import defaultdict
from pathlib import Path

# Versions worth carrying: every Bedrock storage-format change plus the releases
# players actually still open. Chunker has 59; bundling all of them multiplies the
# binary for the long tail of builds nobody reads.
BUNDLED: tuple[str, ...] = (
    "1.12.0.28",
    "1.14.20.1",
    "1.16.101.01",
    "1.17.41.01",
    "1.18.31.04",
    "1.19.83.01",
    "1.20.80.05",
    "1.21.60.0",
    "1.26.60.0",
)

ROOT = Path(__file__).resolve().parents[1]


def catalog_of(path: Path) -> dict[str, dict[str, list[str]]]:
    """Name -> property -> allowed values, from a Chunker registry.

    Chunker lists one entry per *state*, so the value domains are recovered by
    collecting the distinct values each property takes for a block.
    """
    data = json.loads(path.read_text(encoding="utf-8"))
    blocks: dict[str, dict[str, list[str]]] = defaultdict(lambda: defaultdict(set))
    for block in data.get("blocks", []):
        name = block["name"]
        # Touch the entry first: a block with no properties has no states, and a
        # defaultdict would otherwise never create a row for it - which silently
        # dropped hundreds of property-less blocks such as `copper_block`.
        blocks[name]
        for state in block.get("states") or []:
            value = state.get("value")
            if isinstance(value, bool):
                value = "1" if value else "0"
            blocks[name][state["name"]].add(str(value))
    # Same shape as `mappings/data/java/<version>/blocks.json.gz`, so the one
    # baker in core/build.rs handles both editions.
    return {
        name: {
            "properties": {
                prop: sorted(values, key=lambda v: (len(v), v))
                for prop, values in properties.items()
            }
        }
        for name, properties in sorted(blocks.items())
    }


def main() -> int:
    chunker = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "_refs/chunker/cli/data"
    source = chunker / "bedrock"
    if not source.is_dir():
        print(f"missing {source}", file=sys.stderr)
        return 2

    manifest: dict[str, str] = {}
    for version in BUNDLED:
        registry = source / version / "block_states.json"
        if not registry.is_file():
            print(f"skip {version}: no registry", file=sys.stderr)
            continue
        catalog = catalog_of(registry)
        target = ROOT / "mappings/data/bedrock" / version / "blocks.json.gz"
        target.parent.mkdir(parents=True, exist_ok=True)
        payload = json.dumps(catalog, separators=(",", ":"), sort_keys=True).encode()
        with gzip.open(target, "wb", compresslevel=9) as handle:
            handle.write(payload)
        manifest[version] = str(target.relative_to(ROOT)).replace("\\", "/")
        print(f"{version}: {len(catalog)} blocks -> {target.relative_to(ROOT)}")

    (ROOT / "mappings/bedrock_versions.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"wrote mappings/bedrock_versions.json ({len(manifest)} versions)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
