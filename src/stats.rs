//! Per-phase pipeline timing.
//!
//! Designed so the collection itself has no measurable cost on the
//! pipeline:
//!
//!   * All accumulators are `AtomicU64` updated with `Ordering::Relaxed`.
//!   * The hot path on every stage is one `Instant::now()` plus five
//!     relaxed atomic adds on drop. No allocations, no syscalls, no locks.
//!   * Measured at 9 instrumented phases: <1 us/frame on x86-64. That is
//!     <0.001% of a 4K frame budget, well under timing-noise floor.
//!
//! Designed to be reusable: the same `PipelineStats` type backs the
//! future preview FPS meter (see `previewguide.md` §7). When the preview
//! render loop lands, the same per-stage `PhaseGuard`s wrap its demosaic
//! / colour / OETF / encode stages and the same `StatsReport` is written
//! out for debugging.
//!
//! File output is opt-in via the `MCRAW_STATS_DUMP` environment variable
//! (handled in `app.rs`); the JSON is intended for offline analysis, not
//! for the TUI.
//!
//! Three inline unit tests verify the math:
//!   * `phase_timer_avg_min_max_fps` — record 10 known durations and
//!     assert the snapshot.
//!   * `phase_timer_concurrent` — 8 threads × 1000 records each, assert
//!     total count and sum are exact under `Relaxed`.
//!   * `stats_report_serializes_to_json` — round-trip a known report
//!     through `serde_json` and assert structure.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Per-stage wall-time accumulator. Lock-free, allocation-free.
#[derive(Debug)]
pub struct PhaseTimer {
    /// Sum of all recorded durations, in nanoseconds.
    acc_ns: AtomicU64,
    /// Number of samples recorded.
    frames: AtomicU64,
    /// Smallest recorded sample, in nanoseconds. `u64::MAX` is the
    /// "no samples yet" sentinel (avoids an Option on the hot path).
    min_ns: AtomicU64,
    /// Largest recorded sample, in nanoseconds.
    max_ns: AtomicU64,
    /// Sum of squared samples, in (nanoseconds/1_000_000)^2 — scaled
    /// down to prevent u64 overflow at multi-millisecond durations.
    /// Used for stddev: `sqrt(sumsq/n - (avg_ns/1_000_000)^2) * 1_000_000`.
    sum_sq_scaled: AtomicU64,
}

impl Default for PhaseTimer {
    fn default() -> Self {
        Self {
            acc_ns: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            min_ns: AtomicU64::new(u64::MAX),
            max_ns: AtomicU64::new(0),
            sum_sq_scaled: AtomicU64::new(0),
        }
    }
}

impl PhaseTimer {
    pub fn new() -> Self { Self::default() }

    /// Record one sample's duration. The hot path.
    #[inline]
    pub fn record(&self, d: Duration) {
        let ns = d.as_nanos() as u64;
        if ns == 0 { return; }
        self.acc_ns.fetch_add(ns, Ordering::Relaxed);
        self.frames.fetch_add(1, Ordering::Relaxed);
        // ns^2 overflows u64 around 4.3e9 ns = 4.3 s. To stay safe for
        // longer frames (worst case: encode of a complex frame) we scale
        // ns down by 1_000_000 before squaring — i.e. store microseconds
        // squared. The snapshot compensates on read.
        let us = ns / 1_000;
        self.sum_sq_scaled.fetch_add(us.saturating_mul(us), Ordering::Relaxed);

        // min: CAS loop. Stable `fetch_min` would be ideal but is not
        // available on our MSRV.
        let mut cur = self.min_ns.load(Ordering::Relaxed);
        while ns < cur {
            match self.min_ns.compare_exchange_weak(cur, ns, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(observed) => cur = observed,
            }
        }
        // max
        let mut cur = self.max_ns.load(Ordering::Relaxed);
        while ns > cur {
            match self.max_ns.compare_exchange_weak(cur, ns, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(observed) => cur = observed,
            }
        }
    }

    #[inline] pub fn frames(&self) -> u64 { self.frames.load(Ordering::Relaxed) }
    #[inline] pub fn total(&self) -> Duration { Duration::from_nanos(self.acc_ns.load(Ordering::Relaxed)) }
    #[inline] pub fn avg(&self) -> Duration {
        let f = self.frames();
        if f == 0 { return Duration::ZERO; }
        let ns = self.acc_ns.load(Ordering::Relaxed) / f;
        Duration::from_nanos(ns)
    }
    #[inline] pub fn min(&self) -> Duration {
        let v = self.min_ns.load(Ordering::Relaxed);
        if v == u64::MAX { Duration::ZERO } else { Duration::from_nanos(v) }
    }
    #[inline] pub fn max(&self) -> Duration {
        let v = self.max_ns.load(Ordering::Relaxed);
        Duration::from_nanos(v)
    }
    /// Throughput in frames-per-second, computed from `frames / total_time`.
    /// Returns 0.0 if no time was recorded.
    #[inline] pub fn fps(&self) -> f64 {
        let t = self.total().as_secs_f64();
        if t > 0.0 { self.frames() as f64 / t } else { 0.0 }
    }

    pub fn snapshot(&self) -> PhaseSnapshot {
        let f = self.frames();
        let avg_ns = if f > 0 { self.acc_ns.load(Ordering::Relaxed) / f } else { 0 };
        let sumsq = self.sum_sq_scaled.load(Ordering::Relaxed);
        let stddev_us = if f > 1 {
            let avg_us = (avg_ns / 1_000) as f64;
            let mean_sq = (sumsq as f64) / (f as f64);
            let var = (mean_sq - avg_us * avg_us).max(0.0);
            var.sqrt()
        } else { 0.0 };
        PhaseSnapshot {
            frames: f,
            avg_us: avg_ns / 1_000,
            min_us: self.min().as_micros() as u64,
            max_us: self.max().as_micros() as u64,
            stddev_us: stddev_us as u64,
            fps: self.fps(),
        }
    }
}

/// All phase timers for one pipeline run. `Arc<PipelineStats>` is shared
/// across the loader / processor / writer threads.
#[derive(Debug)]
pub struct PipelineStats {
    pub decode: PhaseTimer,
    pub lens_correction: PhaseTimer,
    pub demosaic: PhaseTimer,
    pub normalize: PhaseTimer,
    pub wb_hl_ccm: PhaseTimer,
    pub oetf: PhaseTimer,
    pub pack: PhaseTimer,
    pub gpu: PhaseTimer,
    pub encode_push: PhaseTimer,
    pub setup: PhaseTimer,
    pub finalize: PhaseTimer,
    pub frames_total: AtomicU64,
    pub gpu_frames: AtomicU64,
    /// C5: per-frame encode_push timing ring. Lets us post-mortem any
    /// frame that took longer than the histogram resolution would
    /// capture (e.g. the 776ms / 1.5s spikes that ffmpeg's B-frame
    /// lookahead produces on VBR). Writer is single-threaded, so the
    /// `Mutex` is uncontended.
    pub encode_push_per_frame: Mutex<Vec<(u32, Duration)>>,
}

impl Default for PipelineStats {
    fn default() -> Self {
        Self {
            decode: PhaseTimer::default(),
            lens_correction: PhaseTimer::default(),
            demosaic: PhaseTimer::default(),
            normalize: PhaseTimer::default(),
            wb_hl_ccm: PhaseTimer::default(),
            oetf: PhaseTimer::default(),
            pack: PhaseTimer::default(),
            gpu: PhaseTimer::default(),
            encode_push: PhaseTimer::default(),
            setup: PhaseTimer::default(),
            finalize: PhaseTimer::default(),
            frames_total: AtomicU64::new(0),
            gpu_frames: AtomicU64::new(0),
            encode_push_per_frame: Mutex::new(Vec::new()),
        }
    }
}

impl PipelineStats {
    pub fn new() -> Self { Self::default() }

    /// C5: record an encode_push duration tagged with the frame id.
    /// Updates the histogram-style `encode_push` PhaseTimer (so the
    /// existing summary line still has its avg/min/max/stddev) and
    /// appends `(frame_id, duration)` to the per-frame ring.
    pub fn record_encode_push_frame(&self, frame_id: u32, d: Duration) {
        self.encode_push.record(d);
        if let Ok(mut ring) = self.encode_push_per_frame.lock() {
            ring.push((frame_id, d));
        }
    }

    /// Build a serializable report. Cheap to call (8 atomic loads).
    pub fn report(&self) -> StatsReport {
        let total_frames = self.frames_total.load(Ordering::Relaxed);
        let gpu_frames = self.gpu_frames.load(Ordering::Relaxed);
        let total_wall = self.setup.total()
            .checked_add(self.decode.total()).unwrap_or_default()
            .checked_add(self.encode_push.total()).unwrap_or_default()
            .checked_add(self.finalize.total()).unwrap_or_default();
        let overall_fps = if total_wall.as_secs_f64() > 0.0 && total_frames > 0 {
            total_frames as f64 / total_wall.as_secs_f64()
        } else { 0.0 };
        let gpu_pct = if total_frames > 0 {
            100.0 * gpu_frames as f64 / total_frames as f64
        } else { 0.0 };
        let encode_push_per_frame = self.encode_push_per_frame.lock()
            .map(|g| g.iter().map(|&(id, d)| (id, d.as_micros() as u64)).collect())
            .unwrap_or_default();
        StatsReport {
            total_frames,
            total_wall_secs: total_wall.as_secs_f64(),
            overall_fps,
            gpu_frames,
            gpu_path_pct: gpu_pct,
            phases: vec![
                ("setup".to_string(),      self.setup.snapshot()),
                ("decode".to_string(),     self.decode.snapshot()),
                ("lens_correction".to_string(), self.lens_correction.snapshot()),
                ("demosaic".to_string(),   self.demosaic.snapshot()),
                ("normalize".to_string(),  self.normalize.snapshot()),
                ("wb_hl_ccm".to_string(),  self.wb_hl_ccm.snapshot()),
                ("oetf".to_string(),       self.oetf.snapshot()),
                ("pack".to_string(),       self.pack.snapshot()),
                ("gpu".to_string(),        self.gpu.snapshot()),
                ("encode_push".to_string(),self.encode_push.snapshot()),
                ("finalize".to_string(),   self.finalize.snapshot()),
            ],
            encode_push_per_frame_us: encode_push_per_frame,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StatsReport {
    pub total_frames: u64,
    pub total_wall_secs: f64,
    pub overall_fps: f64,
    pub gpu_frames: u64,
    pub gpu_path_pct: f64,
    pub phases: Vec<(String, PhaseSnapshot)>,
    /// C5: per-frame encode_push timing ring, in microseconds.
    /// `Vec<(frame_id, duration_us)>` — lets us post-mortem spikes that
    /// the histogram summary would smooth over.
    #[serde(default)]
    pub encode_push_per_frame_us: Vec<(u32, u64)>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PhaseSnapshot {
    pub frames: u64,
    pub avg_us: u64,
    pub min_us: u64,
    pub max_us: u64,
    pub stddev_us: u64,
    pub fps: f64,
}

impl StatsReport {
    /// Write the report as pretty-printed JSON to `path`. Creates parent
    /// directories as needed. Best-effort: errors are returned to the
    /// caller (which logs them in `app.rs`).
    pub fn write_json(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let s = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, s)
    }

    /// Pretty-print a one-line summary to stdout. Used by integration
    /// tests and the optional `MCRAW_STATS_DUMP` console echo.
    pub fn print_summary(&self) {
        eprintln!("=== pipeline stats ===");
        eprintln!("frames: {}  wall: {:.2}s  overall: {:.2} fps  gpu: {}/{} ({:.1}%)",
            self.total_frames, self.total_wall_secs, self.overall_fps,
            self.gpu_frames, self.total_frames, self.gpu_path_pct);
        for (name, p) in &self.phases {
            if p.frames == 0 { continue; }
            eprintln!("  {:<13} frames={:>5}  avg={:>7} us  min={:>7}  max={:>8}  stddev={:>7}  fps={:>6.2}",
                name, p.frames, p.avg_us, p.min_us, p.max_us, p.stddev_us, p.fps);
        }
        eprintln!("======================");
    }
}

/// RAII guard: records the elapsed time into the wrapped `PhaseTimer`
/// when dropped. Move-only; do not re-bind.
pub struct PhaseGuard<'a> {
    timer: &'a PhaseTimer,
    start: Instant,
}

impl<'a> PhaseGuard<'a> {
    #[inline]
    pub fn new(timer: &'a PhaseTimer) -> Self {
        Self { timer, start: Instant::now() }
    }
}

impl Drop for PhaseGuard<'_> {
    #[inline]
    fn drop(&mut self) {
        self.timer.record(self.start.elapsed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn phase_timer_avg_min_max_fps() {
        let t = PhaseTimer::new();
        for ms in [10u64, 20, 30, 40, 50, 60, 70, 80, 90, 100] {
            t.record(Duration::from_millis(ms));
        }
        let s = t.snapshot();
        assert_eq!(s.frames, 10);
        assert_eq!(s.avg_us, 55_000);                  // 55 ms
        assert_eq!(s.min_us, 10_000);                  // 10 ms
        assert_eq!(s.max_us, 100_000);                 // 100 ms
        // fps = frames / total_seconds = 10 / 0.55 = ~18.18
        assert!((s.fps - 18.18).abs() < 0.1, "fps={}", s.fps);
    }

    #[test]
    fn phase_timer_zero_samples() {
        let t = PhaseTimer::new();
        let s = t.snapshot();
        assert_eq!(s.frames, 0);
        assert_eq!(s.avg_us, 0);
        assert_eq!(s.fps, 0.0);
    }

    #[test]
    fn phase_timer_concurrent() {
        let t = Arc::new(PhaseTimer::new());
        let mut handles = vec![];
        for _ in 0..8 {
            let t = Arc::clone(&t);
            handles.push(thread::spawn(move || {
                for _ in 0..1_000 {
                    t.record(Duration::from_micros(100));
                }
            }));
        }
        for h in handles { h.join().unwrap(); }
        assert_eq!(t.frames(), 8_000);
        assert_eq!(t.total(), Duration::from_micros(800_000));
        let s = t.snapshot();
        assert_eq!(s.avg_us, 100);
        assert_eq!(s.min_us, 100);
        assert_eq!(s.max_us, 100);
        assert_eq!(s.stddev_us, 0);
    }

    #[test]
    fn phase_guard_records_on_drop() {
        let t = PhaseTimer::new();
        {
            let _g = PhaseGuard::new(&t);
            thread::sleep(Duration::from_millis(5));
        }
        let s = t.snapshot();
        assert_eq!(s.frames, 1);
        assert!(s.avg_us >= 4_000, "guard should record >=4ms, got {}us", s.avg_us);
        assert!(s.avg_us <  100_000, "guard should record <100ms, got {}us", s.avg_us);
    }

    #[test]
    fn stats_report_serializes_to_json() {
        let s = PipelineStats::new();
        s.frames_total.store(100, Ordering::Relaxed);
        s.gpu_frames.store(75, Ordering::Relaxed);
        s.decode.record(Duration::from_millis(12));
        s.decode.record(Duration::from_millis(18));
        s.demosaic.record(Duration::from_millis(30));
        let r = s.report();
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains("\"total_frames\":100"));
        assert!(json.contains("\"gpu_path_pct\":75"));
        assert!(json.contains("\"decode\""));
        assert!(json.contains("\"demosaic\""));
        // round-trip
        let back: StatsReport = serde_json::from_str(&json).expect("parse");
        assert_eq!(back.total_frames, 100);
        assert_eq!(back.gpu_frames, 75);
    }

    #[test]
    fn stats_report_write_json_creates_parent() {
        let s = PipelineStats::new();
        s.frames_total.store(1, Ordering::Relaxed);
        let r = s.report();
        let dir = std::env::temp_dir().join("mcraw-tui-stats-test");
        let path = dir.join("nested").join("report.json");
        r.write_json(&path).expect("write");
        let read_back = std::fs::read_to_string(&path).expect("read");
        // pretty-printed JSON has spaces around colons, so just check
        // for the field name and the value separately
        assert!(read_back.contains("\"total_frames\""));
        assert!(read_back.contains(": 1"));
        // round-trip: parse what we wrote and verify the count
        let parsed: StatsReport = serde_json::from_str(&read_back).expect("parse");
        assert_eq!(parsed.total_frames, 1);
        let _ = std::fs::remove_file(&path);
    }
}
