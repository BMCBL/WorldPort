# Vendored BMCBL plugin SDK

`bmcbl-plugin-api` and `bmcbl-plugin-macros` are copies of the SDK shipped with
the BMCBL plugin examples (API `1.0.0`, WIT package `bmcbl:plugin@1.0.0`).

Copied from `PaperConnect/sdk` (same author, same API version) so this project
builds without a machine-specific path into the launcher repository.

Re-sync when the launcher's WIT contract changes:

```powershell
Copy-Item -Recurse -Force <BMCBL>\crates\bmcbl-plugin-api .
Copy-Item -Recurse -Force <BMCBL>\crates\bmcbl-plugin-macros .
```

A WIT change also requires bumping `api_version` in the generated `plugin.toml`
and rebuilding every plugin.
