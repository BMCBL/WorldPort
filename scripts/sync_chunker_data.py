#!/usr/bin/env python3
"""Syncs Chunker's per-version registries into `_refs/chunker/cli/data`.

Chunker extracts `block_states.json` from each Bedrock build and `blocks.json` from
each Java server release, which makes its `cli/data` tree the authoritative name
list for a version. It is not a dependency we can add, and cloning the whole
repository to read two directories is wasteful, so this downloads the source
tarball once and keeps only those two trees.

The data is reference input, not build input: the catalogs are generated from it
and committed, so a build never needs the network or this script.

Usage:
    python -I scripts/sync_chunker_data.py [--ref main] [--dry-run]
"""
from __future__ import annotations

import io
import sys
import tarfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "_refs" / "chunker" / "cli" / "data"
WANTED = ("cli/data/java/", "cli/data/bedrock/")
TARBALL = "https://codeload.github.com/HiveGamesOSS/Chunker/tar.gz/refs/heads/{ref}"


def main(argv: list[str]) -> int:
    ref = "main"
    dry_run = "--dry-run" in argv
    if "--ref" in argv:
        ref = argv[argv.index("--ref") + 1]

    url = TARBALL.format(ref=ref)
    print(f"fetching {url}")
    with urllib.request.urlopen(url, timeout=300) as response:
        payload = response.read()
    print(f"downloaded {len(payload) / 1024 / 1024:.1f} MiB")

    written = 0
    changed = 0
    versions = {"java": set(), "bedrock": set()}
    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:gz") as archive:
        for member in archive.getmembers():
            if not member.isfile():
                continue
            # Strip the leading `<repo>-<ref>/` component the tarball adds.
            _, _, relative = member.name.partition("/")
            if not relative.startswith(WANTED):
                continue
            parts = relative.split("/")
            # cli/data/<edition>/<version>/<file>
            if len(parts) != 5:
                continue
            _, _, edition, version, name = parts
            versions[edition].add(version)
            destination = TARGET / edition / version / name
            content = archive.extractfile(member)
            if content is None:
                continue
            data = content.read()
            written += 1
            if dry_run:
                continue
            if destination.is_file() and destination.read_bytes() == data:
                continue
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
            changed += 1

    for edition in ("java", "bedrock"):
        found = sorted(versions[edition])
        print(f"{edition}: {len(found)} versions ({found[0] if found else '-'} .. {found[-1] if found else '-'})")
    print(f"{'would write' if dry_run else 'wrote'} {written} files, {changed} changed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
