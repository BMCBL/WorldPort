//! WorldPort native module.
//!
//! The module implements BMCBL's `bmcbl_native_entry` C ABI
//! (`docs/PLUGIN_NATIVE_ABI.md` in the launcher repository): the `.bmcblx`
//! plugin talks to it through `submit_command`/`read_events` jobs, which keeps
//! the WebAssembly sandbox out of the data path.
//!
//! Milestone M6 wires the ABI, the worker thread and the command loop; the
//! conversion engine itself lives in `worldport-core`.

/// ABI revision this module is built against.
pub const ABI_MAJOR: u32 = 1;
/// ABI minor revision.
pub const ABI_MINOR: u32 = 0;
/// Protocol revision spoken over the command/event channel.
pub const PROTOCOL_VERSION: u32 = worldport_protocol::PROTOCOL_VERSION;
