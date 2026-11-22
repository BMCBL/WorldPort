//! Drives the native module through its exported C ABI, the same path BMCBL
//! uses: create an instance, submit commands, read event batches.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bmcbl_plugin_api::native::abi::{self, EventWake, NativeModuleApi};
use worldport_protocol::{
    Command, ConversionRequest, Edition, Event, EventBatch, Features, PruningPolicy,
    decode_batch, encode_command,
};

static WAKES: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn wake(_context: *mut c_void) {
    WAKES.fetch_add(1, Ordering::Relaxed);
}

fn api() -> &'static NativeModuleApi {
    let header = worldport_native::ffi::bmcbl_native_entry();
    // The entry point returns the header at the start of the static table.
    unsafe { &*(header.cast::<NativeModuleApi>()) }
}

struct Instance {
    id: u64,
}

impl Instance {
    fn create() -> Self {
        let mut id = 0_u64;
        let wake = EventWake {
            context: std::ptr::null_mut(),
            notify: Some(wake),
        };
        let api = api();
        let status = unsafe {
            (api.create_instance.expect("create_instance"))(
                std::ptr::null(),
                0,
                wake,
                &mut id,
            )
        };
        assert_eq!(status, abi::OK, "create_instance status");
        assert_ne!(id, 0);
        Self { id }
    }

    fn submit(&self, request: u64, command: &Command) {
        let bytes = encode_command(command).expect("encode command");
        let api = api();
        let status = unsafe {
            (api.submit_command.expect("submit_command"))(
                self.id,
                request,
                bytes.as_ptr(),
                u32::try_from(bytes.len()).expect("command fits"),
            )
        };
        assert_eq!(status, abi::OK, "submit_command status");
    }

    fn read(&self) -> Vec<Event> {
        let mut buffer = vec![0_u8; abi::MAX_EVENT_BATCH];
        let mut written = 0_u32;
        let api = api();
        let status = unsafe {
            (api.read_events.expect("read_events"))(
                self.id,
                buffer.as_mut_ptr(),
                u32::try_from(buffer.len()).expect("capacity fits"),
                &mut written,
            )
        };
        assert_eq!(status, abi::OK, "read_events status");
        buffer.truncate(written as usize);
        if buffer.is_empty() {
            return Vec::new();
        }
        let batch: EventBatch = decode_batch(&buffer).expect("decode batch");
        batch.events.into_iter().map(|item| item.event).collect()
    }

    fn wait_for(&self, mut matches: impl FnMut(&Event) -> bool) -> Event {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            for event in self.read() {
                if matches(&event) {
                    return event;
                }
            }
            assert!(Instant::now() < deadline, "timed out waiting for an event");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let api = api();
        let status = unsafe { (api.destroy_instance.expect("destroy_instance"))(self.id) };
        assert_eq!(status, abi::OK, "destroy_instance status");
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("worldport-native-{name}"));
    let _ = std::fs::remove_dir_all(&path);
    path
}

fn request(source: &Path, target: &Path, edition: Edition) -> ConversionRequest {
    ConversionRequest {
        source: source.display().to_string(),
        target: target.display().to_string(),
        target_edition: Some(edition),
        target_version: None,
        features: Features::default(),
        pruning: PruningPolicy::default(),
        jobs: 0,
        compression_level: 6,
        overwrite: false,
    }
}

#[test]
fn abi_converts_a_fixture_world() {
    let source = temp_dir("je");
    worldport_core::fixtures::write_world(&source).expect("fixture world");
    let target = temp_dir("be");

    let instance = Instance::create();

    // A second instance would own the same process-wide worker slot.
    let mut second = 0_u64;
    let status = unsafe {
        (api().create_instance.expect("create_instance"))(
            std::ptr::null(),
            0,
            EventWake {
                context: std::ptr::null_mut(),
                notify: Some(wake),
            },
            &mut second,
        )
    };
    assert_eq!(status, abi::BUSY, "only one instance at a time");

    instance.submit(
        1,
        &Command::Detect {
            path: source.display().to_string(),
        },
    );
    let detected = instance.wait_for(|event| matches!(event, Event::Detected { .. }));
    let Event::Detected { info, path } = detected else {
        unreachable!()
    };
    assert_eq!(info.edition, "java");
    assert_eq!(path, source.display().to_string());
    assert!(info.total_chunks() >= 1, "fixture has a chunk");

    instance.submit(
        2,
        &Command::Start {
            request: request(&source, &target, Edition::Bedrock),
        },
    );
    let finished = instance.wait_for(|event| {
        matches!(event, Event::Finished { .. } | Event::Failed { .. })
    });
    let Event::Finished { report } = finished else {
        panic!("conversion failed: {finished:?}");
    };
    assert!(report.chunks >= 1, "report: {report:?}");
    assert!(report.blocks > 0, "report: {report:?}");
    assert_eq!(report.maps, 1);
    assert_eq!(report.target_path, target.display().to_string());

    instance.submit(3, &Command::Snapshot);
    let state = instance.wait_for(|event| matches!(event, Event::State { .. }));
    let Event::State {
        running,
        last_report,
        ..
    } = state
    else {
        unreachable!()
    };
    assert!(!running);
    assert!(last_report.is_some_and(|report| report.chunks >= 1));

    drop(instance);
    assert!(WAKES.load(Ordering::Relaxed) > 0, "events woke the host");
    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&target);
}
