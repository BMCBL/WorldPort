# Vendored Bedrock crates

These three crates are **copies** of the versions used by the launcher, taken from
`BMCBL/crates` (they are not published to crates.io at these versions):

- `bedrock-nbt`
- `bedrock-leveldb`
- `bedrock-world`

Source: the BMCBL repository (`crates/`) at commit `e00e019d4`.

Local edits applied to the copies:

- `fearless_simd.workspace = true` → `fearless_simd = "=1.1.0"` in
  `bedrock-nbt` and `bedrock-world` (the workspace root is not copied).
- `bedrock-world/src/world/query.rs`: `World::chunk` reads per-chunk records with a
  sequential scan. The table-parallel pipeline spawns a scope per chunk; many reader
  threads scoped the shared scan pool concurrently and could deadlock, and a chunk
  prefix is a handful of records where the pipeline only adds overhead.
- `bedrock-world/src/world/query.rs`: added `World::chunk_snapshot`, which fetches the
  fixed chunk tag set plus the 1.18+ subchunk range with one `get_many` call and parses
  the biome record from the same batch.
- `bedrock-world/src/scan/mod.rs`: `parse_data3d_with_min_y` is public so the snapshot
  path can decode `Data3D` without a second storage lookup.
- `bedrock-leveldb/src/table_scan.rs`: the parallel table scan keeps one decoded run
  resident per lane, so its footprint was `table_count * 1 MiB`. A 443 MB world has
  200+ SST tables, which pinned ~1.4 GB of decoded blocks just to list chunk keys.
  The run target now shrinks with the table count to stay inside a 32 MiB resident
  budget, and the result window is capped at the scheduler's in-flight target.
  Peak RSS for a full BE->JE conversion dropped 2518 -> 276 MiB; wall time moved
  49.8 s -> 38-40 s (it also stopped thrashing).

Re-sync after the launcher's crates change:

```powershell
./scripts/sync-vendor.ps1 -BmcblRoot <path-to-BMCBL>
```

Conversion fidelity follows the launcher's world stack on purpose: the same
`bedrock-world`/`bedrock-leveldb` code the launcher uses to read and edit worlds
also backs WorldPort, so both tools agree on Bedrock storage semantics.
