//! End-to-end smoke test for the export pipeline + per-phase stats.
//!
//! Gated behind the `MCRAW_TEST_EXPORT` environment variable so it does
//! not run on CI. Run with:
//!
//!   MCRAW_TEST_EXPORT=path/to/file.mcraw \
//!     cargo test --release --test export_stats_smoke -- --nocapture
//!
//! What it does:
//!   1. Loads the .mcraw via `McrawFileInfo::from_path`
//!   2. Runs the full export pipeline (`pipeline::run_export`) with
//!      default settings (ProRes HQ lossless, Rec.709/Rec.709)
//!   3. Reports per-phase and overall timing to stdout
//!   4. Writes the JSON dump to `./mcraw-stats-test.json`
//!   5. Verifies the output video file was created
//!
//! This is a debug-only test. The export can take minutes for 4K input
//! and is skipped unless the env var is set.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[test]
fn export_pipeline_emits_per_phase_stats() {
    let mcraw_path = match std::env::var("MCRAW_TEST_EXPORT") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => {
            eprintln!("MCRAW_TEST_EXPORT not set; skipping end-to-end export test.");
            eprintln!("Run with: MCRAW_TEST_EXPORT=path/to/file.mcraw \\");
            eprintln!("  cargo test --release --test export_stats_smoke -- --nocapture");
            return;
        }
    };

    if !mcraw_path.exists() {
        eprintln!("MCRAW_TEST_EXPORT={} does not exist; skipping.", mcraw_path.display());
        return;
    }

    eprintln!("[test] reading file: {}", mcraw_path.display());
    let mut info = match mcraw_tui::file::McrawFileInfo::from_path(&mcraw_path) {
        Ok(i) => i,
        Err(e) => panic!("McrawFileInfo::from_path failed: {e}"),
    };
    info.enhance_with_decoder();
    eprintln!("[test] file: {}x{} @ {:.2} fps, {} frames",
        info.width, info.height, info.fps, info.frame_count);

    let out_dir = std::env::temp_dir().join("mcraw-tui-export-stats");
    std::fs::create_dir_all(&out_dir).expect("create temp dir");
    let out_path = out_dir.join("smoke_export.mov");
    let _ = std::fs::remove_file(&out_path);

    let stats = Arc::new(mcraw_tui::stats::PipelineStats::new());
    let progress = Arc::new(|_pct: f64| {});
    let cancel = Arc::new(AtomicBool::new(false));

    eprintln!("[test] starting export to {}", out_path.display());
    let start = std::time::Instant::now();

    // Match the defaults of `pipeline::run` exactly: ProRes HQ, Rec.709,
    // Rec.709 transfer, software encoders, lossless rate control.
    use mcraw_tui::color::{ColorSpace, TransferFunction};
    use mcraw_tui::export::{CodecFamily, ProResProfile, RateControl};
    use mcraw_tui::pipeline;

    let result = pipeline::run_export(
        info,
        out_path.to_string_lossy().to_string(),
        progress,
        cancel,
        stats.clone(),
        ColorSpace::Rec709,
        TransferFunction::Rec709,
        CodecFamily::ProRes,
        ProResProfile::HQ,
        mcraw_tui::export::DnxhrProfile::HQX,
        mcraw_tui::export::HevcProfile::Main10_420,
        mcraw_tui::export::H264Profile::Main8bit,
        mcraw_tui::export::Av1Profile::Profile0_420_10bit,
        mcraw_tui::export::Vp9Profile::Profile2_420_10bit,
        "libx265".to_string(),
        "libx264".to_string(),
        "libaom-av1".to_string(),
        "prores_ks".to_string(),
        RateControl::Lossless,
        None,
    );

    let wall = start.elapsed();
    match &result {
        Ok(()) => eprintln!("[test] export OK in {:.1}s", wall.as_secs_f64()),
        Err(e) => eprintln!("[test] export failed: {e}"),
    }
    result.expect("export failed");

    assert!(out_path.exists(), "output video not created");
    let size = std::fs::metadata(&out_path).expect("stat").len();
    eprintln!("[test] output size: {:.1} MB", size as f64 / 1_048_576.0);

    let report = stats.report();
    report.print_summary();

    let json_path = std::env::current_dir().expect("cwd").join("mcraw-stats-test.json");
    report.write_json(&json_path).expect("write json");
    eprintln!("[test] stats JSON: {}", json_path.display());

    // Sanity: total_frames > 0, total_wall_secs > 0
    assert!(report.total_frames > 0, "no frames recorded");
    assert!(report.total_wall_secs > 0.0, "no wall time recorded");

    // Sanity: at least one phase recorded > 0 frames.
    let any_phase = report.phases.iter().any(|(_, p)| p.frames > 0);
    assert!(any_phase, "no phase recorded any frames");
}
