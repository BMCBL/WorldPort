"""Fetch per-version Java block catalogs from misode/mcmeta and write our compact form.

Run from the repository root:

    python scripts/fetch_je_catalogs.py 1.20.6 1.21.4 1.21.8 26.1 26.2

Each version produces `mappings/data/java/<version>/blocks.json.gz` (name ->
properties -> accepted values) plus `version.json`, and `mappings/je_versions.json`
is rewritten with the DataVersion table used for target selection.
"""

import gzip
import json
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "mappings" / "data" / "java"
BASE = "https://cdn.jsdelivr.net/gh/misode/mcmeta@{version}-summary"


def fetch(url: str) -> dict:
    with urllib.request.urlopen(url, timeout=60) as response:
        return json.load(response)


def compact(blocks: dict) -> dict:
    """mcmeta: name -> [ {property: [values]}, ... ]  ->  minecraft:name -> {properties: {...}}"""
    out = {}
    for name, states in blocks.items():
        properties: dict[str, list[str]] = {}
        if isinstance(states, list):
            for state in states:
                for key, values in state.items():
                    # mcmeta stores a scalar for single-value properties.
                    if isinstance(values, (str, bool, int)):
                        values = [values]
                    existing = properties.setdefault(key, [])
                    for value in values:
                        text = str(value).lower() if isinstance(value, bool) else str(value)
                        if text not in existing:
                            existing.append(text)
        out[f"minecraft:{name}"] = {"properties": properties}
    return out


def main(versions: list[str]) -> None:
    table = {}
    for version in versions:
        metadata = fetch(f"{BASE.format(version=version)}/version.json")
        blocks = fetch(f"{BASE.format(version=version)}/blocks/data.json")
        target = DATA / version
        target.mkdir(parents=True, exist_ok=True)
        payload = json.dumps(compact(blocks), ensure_ascii=False, separators=(",", ":"))
        with gzip.open(target / "blocks.json.gz", "wb", compresslevel=9) as handle:
            handle.write(payload.encode("utf-8"))
        (target / "version.json").write_text(
            json.dumps(metadata, indent=2), encoding="utf-8"
        )
        table[version] = int(metadata["data_version"])
        print(
            f"{version}: data_version={metadata['data_version']} blocks={len(blocks)} "
            f"gzip={len(gzip.compress(payload.encode('utf-8')))} bytes"
        )

    versions_file = ROOT / "mappings" / "je_versions.json"
    existing = {}
    if versions_file.is_file():
        existing = json.loads(versions_file.read_text(encoding="utf-8"))
    existing.update(table)
    versions_file.write_text(
        json.dumps(dict(sorted(existing.items(), key=lambda item: item[1])), indent=2),
        encoding="utf-8",
    )
    print(f"wrote {versions_file} with {len(existing)} versions")


if __name__ == "__main__":
    main(sys.argv[1:] or ["1.20.6", "1.21.4", "1.21.8", "26.1", "26.2"])
