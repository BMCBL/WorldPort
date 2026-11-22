//! C ABI entry points for BMCBL's native module loader.
//!
//! The shape follows `bmcbl_plugin_api::native::abi`: one static function table
//! behind `bmcbl_native_entry`, caller-owned buffers, and status codes. Panics
//! are caught at the boundary because unwinding into the host is undefined.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use bmcbl_plugin_api::native::abi::{self, AbiHeader, EventWake, NativeModuleApi};

use crate::session::Session;

static SESSION: Mutex<Option<(u64, Session)>> = Mutex::new(None);
static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);
static API: NativeModuleApi = NativeModuleApi {
    header: NativeModuleApi::HEADER,
    create_instance: Some(create_instance),
    submit_command: Some(submit_command),
    read_events: Some(read_events),
    cancel_command: Some(cancel_command),
    destroy_instance: Some(destroy_instance),
};

/// Returns the ABI table; this is the only exported symbol.
#[unsafe(no_mangle)]
pub extern "C" fn bmcbl_native_entry() -> *const AbiHeader {
    &API.header
}

unsafe extern "C" fn create_instance(
    _config: *const u8,
    config_len: u32,
    wake: EventWake,
    out: *mut u64,
) -> i32 {
    ffi(|| {
        if out.is_null() || config_len != 0 || wake.notify.is_none() {
            return abi::INVALID_ARGUMENT;
        }
        let mut slot = SESSION.lock().unwrap_or_else(|error| error.into_inner());
        if slot.is_some() {
            return abi::BUSY;
        }
        let Ok(session) = Session::start(wake) else {
            return abi::INTERNAL;
        };
        let id = NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed);
        *slot = Some((id, session));
        // SAFETY: the caller provides one writable instance handle for this call.
        unsafe { out.write(id) };
        abi::OK
    })
}

unsafe extern "C" fn submit_command(id: u64, request: u64, source: *const u8, len: u32) -> i32 {
    ffi(|| {
        if request == 0 || len == 0 || len as usize > abi::MAX_COMMAND || source.is_null() {
            return abi::INVALID_ARGUMENT;
        }
        // SAFETY: the caller leases len readable bytes for this call; submit copies them.
        let command = unsafe { std::slice::from_raw_parts(source, len as usize) };
        instance(id, |session| session.submit(request, command))
    })
}

unsafe extern "C" fn read_events(
    id: u64,
    output: *mut u8,
    capacity: u32,
    written: *mut u32,
) -> i32 {
    ffi(|| {
        if written.is_null() || (output.is_null() && capacity != 0) {
            return abi::INVALID_ARGUMENT;
        }
        instance(id, |session| {
            match session.events.read_batch(capacity as usize) {
                Ok(batch) => {
                    // SAFETY: the leased output is large enough for the batch; zero bytes need no buffer.
                    unsafe {
                        if !batch.is_empty() {
                            std::ptr::copy_nonoverlapping(batch.as_ptr(), output, batch.len());
                        }
                        written.write(u32::try_from(batch.len()).unwrap_or(u32::MAX));
                    }
                    abi::OK
                }
                Err(needed) => {
                    // SAFETY: written points to the caller-owned required-length field.
                    unsafe { written.write(u32::try_from(needed).unwrap_or(u32::MAX)) };
                    abi::BUFFER_TOO_SMALL
                }
            }
        })
    })
}

unsafe extern "C" fn cancel_command(id: u64, request: u64) -> i32 {
    ffi(|| instance(id, |session| session.cancel(request)))
}

unsafe extern "C" fn destroy_instance(id: u64) -> i32 {
    ffi(|| {
        let mut slot = SESSION.lock().unwrap_or_else(|error| error.into_inner());
        if !slot
            .as_ref()
            .is_some_and(|(current_id, _)| id == *current_id)
        {
            return abi::INVALID_HANDLE;
        }
        slot.take()
            .map_or(abi::INVALID_HANDLE, |(_, session)| session.stop())
    })
}

fn instance(id: u64, call: impl FnOnce(&Session) -> i32) -> i32 {
    let slot = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    match slot.as_ref() {
        Some((current, session)) if *current == id => call(session),
        _ => abi::INVALID_HANDLE,
    }
}

fn ffi(call: impl FnOnce() -> i32) -> i32 {
    catch_unwind(AssertUnwindSafe(call)).unwrap_or(abi::INTERNAL)
}
