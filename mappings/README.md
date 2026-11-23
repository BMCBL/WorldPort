# Block data

Everything in `mappings/data/` is generated. Do not edit it by hand; edit the
generator and re-run it.

## Where the data comes from

| Directory | Source | Licence |
| --- | --- | --- |
| `data/java/<version>/` | Chunker's `cli/data/java/<version>/blocks.json` | MIT |
| `data/bedrock/<version>/` | Chunker's `cli/data/bedrock/<version>/block_states.json` | MIT |

Both come from [HiveGamesOSS/Chunker](https://github.com/HiveGamesOSS/Chunker),
which extracts the registries from the game itself: the Java side by running each
server release, the Bedrock side from each client build. That makes it the
authoritative name list for a version rather than a reconstruction.

`scripts/sync_chunker_data.py` downloads Chunker's source tarball and keeps only
`cli/data/{java,bedrock}`, so refreshing 111 registries is one command. The
scheduled workflow `.github/workflows/sync-data.yml` runs it weekly.

## How it is regenerated

```
python -I scripts/sync_chunker_data.py
python -I scripts/gen_bedrock_catalogs.py
python -I scripts/gen_je_catalogs_from_chunker.py <versions...>
```

Then `core/build.rs` bakes the result at compile time. The runtime never parses
JSON.

`gen_je_catalogs_from_chunker.py` reads the local Chunker data, which is the
offline counterpart to `fetch_je_catalogs.py` (misode/mcmeta over the network) and
the only path for versions mcmeta does not summarise. Three releases (1.13, 1.13.1,
1.13.2) have no `version.json` in Chunker's tree; their DataVersions come from a
small table in the generator.

## Why one blob

59 Bedrock catalogs share almost all of their block, property and value names.
Storing each separately paid for that vocabulary once per catalog, and LZ4 cannot
compress across blobs. `build.rs` interns every string once into a pool, references
them by index, and compresses the single resulting stream: 1.32 MiB of per-catalog
data becomes 116 KiB embedded.

The format is documented above `bake_bedrock` in `core/build.rs`. It is an internal
artifact of one build, so it carries a version byte and a magic header but no
compatibility promise beyond the build that wrote it.

## What this replaced

The Bedrock side used to read the block schema `bedrock-world` embeds. That is a
single snapshot of one release, captured from one reference world, so it:

- could not describe any release other than the one it came from, which made every
  older save read against a catalog that never contained its ids;
- listed only the states that reference world happened to contain, so whole blocks
  were missing from it (wool and concrete slabs among them).

The Java side still uses `mappings/data/java/`; both editions now read generated
catalogs and nothing reads `bedrock-world`'s block table.

## Coverage

`coverage_baseline.json` pins the conversion result for every catalog pair.
`core/tests/palette_coverage.rs` fails on a regression and passes an improvement,
so a refresh that would lose blocks fails the scheduled job instead of landing.
