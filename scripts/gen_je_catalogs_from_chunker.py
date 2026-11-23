#!/usr/bin/env python3
"""Bakes Java block catalogs from Chunker's local registries.

`fetch_je_catalogs.py` pulls the same data from misode/mcmeta over the network.
Chunker ships what it extracted from the real server, already on disk, which makes
this the offline path and the one that can cover versions mcmeta does not publish
summaries for.

Usage:
    python -I scripts/gen_je_catalogs_from_chunker.py 26.3 26.4
"""
from __future__ import annotations

import gzip
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "mappings" / "data" / "java"
CHUNKER = ROOT / "_refs" / "chunker" / "cli" / "data" / "java"

# Chunker did not record metadata for the first three registries it extracted.
# These are the DataVersions those releases shipped with.
KNOWN_DATA_VERSIONS = {"1.13": 1519, "1.13.1": 1628, "1.13.2": 1631}


def compact(blocks: dict) -> dict:
    """Chunker: name -> {properties, states}  ->  minecraft:name -> {properties}."""
    out = {}
    for name, block in blocks.items():
        properties = {}
        if isinstance(block, dict):
            for key, values in (block.get("properties") or {}).items():
                properties[key] = [str(value) for value in values]
        out[name if name.startswith("minecraft:") else f"minecraft:{name}"] = {
            "properties": properties
        }
    return out


def main(versions: list[str]) -> int:
    table = {}
    for version in versions:
        source = CHUNKER / version
        blocks_file = source / "blocks.json"
        if not blocks_file.is_file():
            print(f"skip {version}: no {blocks_file}", file=sys.stderr)
            continue
        blocks = json.loads(blocks_file.read_text(encoding="utf-8"))
        metadata_path = source / "version.json"
        metadata = (
            json.loads(metadata_path.read_text(encoding="utf-8"))
            if metadata_path.is_file()
            else {}
        )
        data_version = metadata.get("world_version") or KNOWN_DATA_VERSIONS.get(version)
        if not isinstance(data_version, int):
            print(f"skip {version}: no world_version", file=sys.stderr)
            continue

        target = DATA / version
        target.mkdir(parents=True, exist_ok=True)
        payload = json.dumps(compact(blocks), separators=(",", ":"), sort_keys=True)
        with gzip.open(target / "blocks.json.gz", "wb", compresslevel=9) as handle:
            handle.write(payload.encode("utf-8"))
        (target / "version.json").write_text(
            json.dumps(
                {
                    "id": version,
                    "name": metadata.get("name", version),
                    "type": metadata.get("type", "release"),
                    "data_version": data_version,
                    "source": "Chunker cli/data/java (extracted from the vanilla server)",
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        table[version] = data_version
        print(f"{version}: data_version={data_version} blocks={len(blocks)}")

    versions_file = ROOT / "mappings" / "je_versions.json"
    existing = json.loads(versions_file.read_text(encoding="utf-8")) if versions_file.is_file() else {}
    existing.update(table)
    versions_file.write_text(
        json.dumps(dict(sorted(existing.items(), key=lambda item: item[1])), indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"wrote {versions_file} with {len(existing)} versions")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:] or ["26.3", "26.4"]))
