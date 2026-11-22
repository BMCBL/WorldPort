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

Re-sync after the launcher's crates change:

```powershell
./scripts/sync-vendor.ps1 -BmcblRoot <path-to-BMCBL>
```

Conversion fidelity follows the launcher's world stack on purpose: the same
`bedrock-world`/`bedrock-leveldb` code the launcher uses to read and edit worlds
also backs WorldPort, so both tools agree on Bedrock storage semantics.
