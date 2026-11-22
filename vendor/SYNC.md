# Vendored Bedrock crates

These three crates are **copies** of the versions used by the launcher, taken from
`BMCBL/crates` (they are not published to crates.io at these versions):

- `bedrock-nbt`
- `bedrock-leveldb`
- `bedrock-world`

Source: the BMCBL repository (`crates/`) at commit `b14e08d1c`.

## Do not patch these copies

Every change WorldPort needs belongs in the launcher's crates first and is then
copied here by `scripts/sync-vendor.ps1`. A local edit is silently lost the next
time the script runs, and the launcher and the converter would otherwise drift
apart on Bedrock storage semantics.

The changes that used to live only in this copy are now in `BMCBL/crates`:

- `bedrock-world`: `World::chunk` scans per-chunk records sequentially (the
  table-parallel pipeline spawns a scope per chunk, and many reader threads
  scoping the shared scan pool concurrently could deadlock), and
  `World::chunk_snapshot` fetches the fixed chunk tag set plus the 1.18+
  subchunk range in one `get_many` call. `parse_data3d_with_min_y` is public so
  the snapshot path can decode `Data3D` without a second storage lookup.
- `bedrock-leveldb`: the parallel table scan keeps its resident decoded runs
  inside a fixed 32 MiB budget instead of `table_count * 1 MiB` (a 443 MB world
  has 200+ SST tables), and caps the result window at the scheduler's in-flight
  target. Full BE→JE conversion: peak RSS 2518 → 276 MiB.
- `bedrock-world::village`: the point-of-interest model (`InterestKind`,
  `InterestInstance`, `VillagerInterests`, `parse_poi_record`,
  `java_interest_type`), shared by the launcher and WorldPort.

Local edits still applied by the sync script:

- `fearless_simd.workspace = true` → `fearless_simd = "=1.1.0"` in
  `bedrock-nbt` and `bedrock-world` (the workspace root is not copied).

Re-sync after the launcher's crates change:

```powershell
./scripts/sync-vendor.ps1 -BmcblRoot <path-to-BMCBL>
```

Conversion fidelity follows the launcher's world stack on purpose: the same
`bedrock-world`/`bedrock-leveldb` code the launcher uses to read and edit worlds
also backs WorldPort, so both tools agree on Bedrock storage semantics.
