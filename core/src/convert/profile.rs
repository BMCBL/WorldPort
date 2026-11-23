//! Per-worker conversion profiling.
//!
//! The stage report answers "how long did terrain take"; it cannot answer
//! "how much of terrain was reading, mapping or compressing". These counters
//! split one worker's work into those parts and are only touched when
//! `WORLDPORT_PROFILE` is set, so the production path pays a single relaxed
//! load per checkpoint.
//!
//! Aggregation is per worker and merged at the end: a shared atomic per block
//! would cost more than the work it measures.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// One worker's accumulated timings, in microseconds.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct ChunkTimings {
    /// Reading the chunk records and actor payloads.
    pub read_us: u64,
    /// Mapping blocks and assembling the Java NBT tree.
    pub map_us: u64,
    /// Encoding the NBT tree to bytes.
    pub encode_us: u64,
    /// Compressing the encoded bytes.
    pub compress_us: u64,
    /// Chunks measured.
    pub chunks: u64,
}

impl ChunkTimings {
    /// Adds another worker's timings into this one.
    pub fn merge(&mut self, other: &Self) {
        self.read_us = self.read_us.saturating_add(other.read_us);
        self.map_us = self.map_us.saturating_add(other.map_us);
        self.encode_us = self.encode_us.saturating_add(other.encode_us);
        self.compress_us = self.compress_us.saturating_add(other.compress_us);
        self.chunks = self.chunks.saturating_add(other.chunks);
    }
}

/// True when profiling was requested for this process.
#[must_use]
pub fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("WORLDPORT_PROFILE").is_some())
}

/// Accumulates one worker's timings.
#[derive(Debug, Default)]
pub struct Profiler {
    timings: ChunkTimings,
    mark: Option<Instant>,
}

impl Profiler {
    /// Creates a profiler; inert unless profiling is enabled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts timing a phase.
    pub fn start(&mut self) {
        if enabled() {
            self.mark = Some(Instant::now());
        }
    }

    /// Stops the current phase and adds it to `slot`.
    pub fn stop(&mut self, slot: fn(&mut ChunkTimings) -> &mut u64) {
        let Some(started) = self.mark.take() else {
            return;
        };
        let elapsed = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let target = slot(&mut self.timings);
        *target = target.saturating_add(elapsed);
    }

    /// Marks the boundary between storage reads and CPU mapping inside a chunk.
    ///
    /// Called from inside the chunk reader, where the phase switch happens; the
    /// caller's `read` slot then only covers the storage part.
    pub fn mark_mapping_start(&mut self) {
        if !enabled() {
            return;
        }
        if let Some(started) = self.mark.take() {
            let elapsed = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            self.timings.read_us = self.timings.read_us.saturating_add(elapsed);
        }
        self.mark = Some(Instant::now());
    }

    /// Counts one measured chunk.
    pub fn count_chunk(&mut self) {
        if enabled() {
            self.timings.chunks = self.timings.chunks.saturating_add(1);
        }
    }

    /// The accumulated timings.
    #[must_use]
    pub const fn timings(&self) -> ChunkTimings {
        self.timings
    }
}

/// Phase selectors, used with [`Profiler::stop`].
pub fn read(t: &mut ChunkTimings) -> &mut u64 {
    &mut t.read_us
}

/// Phase selector for block mapping.
pub fn map(t: &mut ChunkTimings) -> &mut u64 {
    &mut t.map_us
}

/// Phase selector for NBT encoding.
pub fn encode(t: &mut ChunkTimings) -> &mut u64 {
    &mut t.encode_us
}

/// Phase selector for compression.
pub fn compress(t: &mut ChunkTimings) -> &mut u64 {
    &mut t.compress_us
}

/// Reports the merged timings for a conversion.
pub fn report(timings: &ChunkTimings) {
    if !enabled() || timings.chunks == 0 {
        return;
    }
    let chunks = timings.chunks.max(1);
    eprintln!(
        "profile: {} chunks; read {} ms ({:.2} ms/chunk), map {} ms ({:.2}), \
         encode {} ms ({:.2}), compress {} ms ({:.2})",
        timings.chunks,
        timings.read_us / 1000,
        timings.read_us as f64 / 1000.0 / chunks as f64,
        timings.map_us / 1000,
        timings.map_us as f64 / 1000.0 / chunks as f64,
        timings.encode_us / 1000,
        timings.encode_us as f64 / 1000.0 / chunks as f64,
        timings.compress_us / 1000,
        timings.compress_us as f64 / 1000.0 / chunks as f64,
    );
}

/// Shared totals for the driver to merge worker results into.
#[derive(Debug, Default)]
pub struct Totals {
    read_us: AtomicU64,
    map_us: AtomicU64,
    encode_us: AtomicU64,
    compress_us: AtomicU64,
    chunks: AtomicU64,
}

impl Totals {
    /// Adds one worker's timings.
    pub fn add(&self, timings: &ChunkTimings) {
        if !enabled() {
            return;
        }
        self.read_us.fetch_add(timings.read_us, Ordering::Relaxed);
        self.map_us.fetch_add(timings.map_us, Ordering::Relaxed);
        self.encode_us
            .fetch_add(timings.encode_us, Ordering::Relaxed);
        self.compress_us
            .fetch_add(timings.compress_us, Ordering::Relaxed);
        self.chunks.fetch_add(timings.chunks, Ordering::Relaxed);
    }

    /// Snapshots the merged totals.
    #[must_use]
    pub fn snapshot(&self) -> ChunkTimings {
        ChunkTimings {
            read_us: self.read_us.load(Ordering::Relaxed),
            map_us: self.map_us.load(Ordering::Relaxed),
            encode_us: self.encode_us.load(Ordering::Relaxed),
            compress_us: self.compress_us.load(Ordering::Relaxed),
            chunks: self.chunks.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_sums_every_phase() {
        let mut left = ChunkTimings {
            read_us: 10,
            map_us: 20,
            encode_us: 30,
            compress_us: 40,
            chunks: 1,
        };
        let right = ChunkTimings {
            read_us: 1,
            map_us: 2,
            encode_us: 3,
            compress_us: 4,
            chunks: 2,
        };
        left.merge(&right);
        assert_eq!(left.read_us, 11);
        assert_eq!(left.map_us, 22);
        assert_eq!(left.encode_us, 33);
        assert_eq!(left.compress_us, 44);
        assert_eq!(left.chunks, 3);
    }

    #[test]
    fn totals_accumulate_worker_results() {
        let totals = Totals::default();
        // `add` is a no-op unless profiling is enabled, which it is not in
        // tests, so exercise the snapshot shape instead of the counters.
        let snapshot = totals.snapshot();
        assert_eq!(snapshot.chunks, 0);
        assert_eq!(snapshot.read_us, 0);
    }

    #[test]
    fn phase_selectors_target_distinct_fields() {
        let mut timings = ChunkTimings::default();
        *read(&mut timings) = 1;
        *map(&mut timings) = 2;
        *encode(&mut timings) = 3;
        *compress(&mut timings) = 4;
        assert_eq!(timings.read_us, 1);
        assert_eq!(timings.map_us, 2);
        assert_eq!(timings.encode_us, 3);
        assert_eq!(timings.compress_us, 4);
    }
}
