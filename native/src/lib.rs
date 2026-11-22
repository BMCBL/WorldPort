//! WorldPort native module.
//!
//! The module implements BMCBL's `bmcbl_native_entry` C ABI
//! (`docs/PLUGIN_NATIVE_ABI.md` in the launcher repository): the `.bmcblx`
//! plugin talks to it through `submit_command`/`read_events` jobs, which keeps
//! the WebAssembly sandbox out of the data path.
//!
//! The conversion engine lives in `worldport-core`; this crate owns the ABI,
//! the worker thread, the event ring and cancellation.

pub mod ffi;
mod info;
mod session;

/// ABI revision this module is built against.
pub const ABI_MAJOR: u32 = bmcbl_plugin_api::native::abi::MAJOR;
/// ABI minor revision.
pub const ABI_MINOR: u32 = bmcbl_plugin_api::native::abi::MINOR;
/// Protocol revision spoken over the command/event channel.
pub const PROTOCOL_VERSION: u32 = worldport_protocol::PROTOCOL_VERSION;
