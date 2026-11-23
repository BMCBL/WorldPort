use std::ffi::{OsString, c_void};
use std::path::Path;
use std::ptr::null;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use bmcbl_plugin_api::native::abi::{self, EventWake, NativeModuleApi};
use libloading::Library;
use worldport_protocol::{
    Command, ConversionRequest, Event, ReportSummary, decode_batch, encode_command,
};

#[derive(Default)]
struct Wake {
    pending: Mutex<bool>,
    changed: Condvar,
}

unsafe extern "C" fn notify(context: *mut c_void) {
    // SAFETY: the runner keeps the wake object alive through instance teardown.
    let wake = unsafe { &*context.cast::<Wake>() };
    if let Ok(mut pending) = wake.pending.lock() {
        *pending = true;
        wake.changed.notify_one();
    }
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [library, source, output, version, jobs, report] = args.as_slice() else {
        bail!(
            "usage: worldport-bench-runner <dll> <source> <output> <target-version> <jobs> <report>"
        );
    };
    let request = ConversionRequest {
        source: utf8(source)?.into(),
        target: utf8(output)?.into(),
        target_edition: None,
        target_version: Some(utf8(version)?.into()),
        features: Default::default(),
        pruning: Default::default(),
        jobs: utf8(jobs)?.parse()?,
        compression_level: 6,
        overwrite: false,
    };
    let report_data = run(Path::new(library), request)?;
    std::fs::write(report, serde_json::to_vec_pretty(&report_data)?)?;
    Ok(())
}

fn utf8(value: &OsString) -> Result<&str> {
    value.to_str().context("argument is not UTF-8")
}

fn run(path: &Path, request: ConversionRequest) -> Result<ReportSummary> {
    let wake = Box::new(Wake::default());
    // SAFETY: the benchmark caller selects the locally built WorldPort module.
    let library = unsafe { Library::new(path) }?;
    // SAFETY: WorldPort exports this symbol with the declared ABI signature.
    let entry = unsafe { library.get::<abi::Entry>(abi::ENTRY) }?;
    // SAFETY: the module owns a static ABI header for the lifetime of this library.
    let table = unsafe { entry() };
    // SAFETY: the preceding call returns a readable ABI header or null.
    let header = unsafe { table.as_ref() }.context("null ABI header")?;
    ensure!(
        header.abi_major == abi::MAJOR && header.struct_size >= size_of::<NativeModuleApi>() as u32,
        "incompatible native module ABI"
    );
    // SAFETY: the header's version and length validate the consumed ABI prefix.
    let api = unsafe { &*table.cast::<NativeModuleApi>() };
    let create = api.create_instance.context("ABI has no create_instance")?;
    let destroy = api
        .destroy_instance
        .context("ABI has no destroy_instance")?;
    let mut instance = 0;
    let signal = EventWake {
        context: std::ptr::from_ref(wake.as_ref()).cast_mut().cast(),
        notify: Some(notify),
    };
    // SAFETY: the empty config, wake callback and instance destination satisfy the ABI.
    status(
        unsafe { create(null(), 0, signal, &mut instance) },
        "create",
    )?;
    let result = convert(api, instance, request, &wake);
    // SAFETY: the valid handle was created above and has no outstanding host calls.
    if let Err(error) = status(unsafe { destroy(instance) }, "destroy") {
        // Teardown could not prove that no worker still uses the module or callback.
        std::mem::forget(wake);
        std::mem::forget(library);
        return Err(error);
    }
    result
}

fn convert(
    api: &NativeModuleApi,
    instance: u64,
    request: ConversionRequest,
    wake: &Wake,
) -> Result<ReportSummary> {
    let submit = api.submit_command.context("ABI has no submit_command")?;
    let read = api.read_events.context("ABI has no read_events")?;
    let bytes = encode_command(&Command::Start { request })?;
    ensure!(bytes.len() <= abi::MAX_COMMAND, "command exceeds ABI limit");
    // SAFETY: the command buffer stays alive throughout the call.
    status(
        unsafe { submit(instance, 1, bytes.as_ptr(), bytes.len() as u32) },
        "submit",
    )?;

    let start = Instant::now();
    let mut buffer = vec![0; abi::MAX_EVENT_BATCH];
    while start.elapsed() < Duration::from_secs(3600) {
        let pending = wake
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("wake lock poisoned"))?;
        let (mut pending, _) = wake
            .changed
            .wait_timeout_while(pending, Duration::from_millis(250), |pending| !*pending)
            .map_err(|_| anyhow::anyhow!("wake lock poisoned"))?;
        *pending = false;
        drop(pending);

        let mut written = 0;
        // SAFETY: the caller owns the output buffers and grants their exact capacities.
        status(
            unsafe {
                read(
                    instance,
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    &mut written,
                )
            },
            "read events",
        )?;
        if written == 0 {
            continue;
        }
        let bytes = buffer
            .get(..written as usize)
            .context("invalid native event buffer length")?;
        for event in decode_batch(bytes)?.events {
            match event.event {
                Event::Finished { report } => return Ok(report),
                Event::Failed { message } => bail!("{message}"),
                Event::Cancelled => bail!("conversion cancelled"),
                _ => {}
            }
        }
    }
    bail!("timed out waiting for DLL conversion")
}

fn status(code: i32, operation: &str) -> Result<()> {
    ensure!(code == abi::OK, "{operation} failed with ABI status {code}");
    Ok(())
}
