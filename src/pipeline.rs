use crate::agx::{AgxConfig, AgxPipeline, Gamut, OutputTransfer, Transfer};
use crate::color::{
    normalize_linear_f32, identity_ccm,
    mat_mul_vec3, mat_mul_3x3, camera_to_xyz_matrix, interpolate_matrix,
    build_cat16_output_matrix, xyz_from_chromaticities,
    D65_XYZ, xyz_to_rec709,
    BilinearDemosaic,
    ColorSpace, TransferFunction,
};
use crate::decoder::Decoder;
use crate::encoder::VideoEncoder;
use crate::export::{
    Av1Profile, CodecFamily, DnxhrProfile, H264Profile, HevcProfile,
    ProResProfile, RateControl, Vp9Profile,
};
use crate::file::McrawFileInfo;
use crate::gpu;
use anyhow::{anyhow, Result};
use crossbeam_channel::bounded;
use rayon::prelude::*;
use std::fs;
use std::io::{BufWriter, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Number of frame slots in the producer-consumer pool.
const PIPELINE_DEPTH: usize = 3;

/// A pre-allocated buffer slot that circulates through the pipeline stages.
struct FrameSlot {
    bayer: Vec<u16>,
    frame_bytes: Vec<u8>,
    as_shot_neutral: [f32; 3],
}

/// Build FFmpeg codec arguments.
/// Delegates to `CodecFamily::to_ffmpeg_args` which independently resolves
/// the codec family, the user-chosen profile, and the runtime-detected
/// encoder names.
pub fn build_ffmpeg_codec_args(
    family: CodecFamily,
    hevc_encoder: &str,
    h264_encoder: &str,
    av1_encoder: &str,
    prores_encoder: &str,
    prores: ProResProfile,
    dnxhr: DnxhrProfile,
    hevc: HevcProfile,
    h264: H264Profile,
    av1: Av1Profile,
    vp9: Vp9Profile,
    rate_control: &RateControl,
) -> (String, String, Vec<String>) {
    family.to_ffmpeg_args(hevc_encoder, h264_encoder, av1_encoder, prores_encoder, prores, dnxhr, hevc, h264, av1, vp9, rate_control)
}

pub fn get_ffmpeg_vui_tags(color_space: &ColorSpace, transfer: &TransferFunction) -> Vec<&'static str> {
    let (primaries, matrix) = match color_space {
        ColorSpace::Rec709 | ColorSpace::Srgb => ("bt709", "bt709"),
        ColorSpace::Rec2020 => ("bt2020", "bt2020nc"),
        ColorSpace::DciP3 | ColorSpace::DisplayP3 => ("smpte432", "bt2020nc"),
        ColorSpace::SGamut3Cine | ColorSpace::SGamut3
        | ColorSpace::ARRIWideGamut3 | ColorSpace::ARRIWideGamut4
        | ColorSpace::CanonCinemaGamut | ColorSpace::PanasonicVGamut
        | ColorSpace::FGamut | ColorSpace::FGamutC
        | ColorSpace::DaVinciWideGamut | ColorSpace::ACESAP1 => ("bt2020", "bt2020nc"),
    };

    let trc = match transfer {
        TransferFunction::Rec709 => "bt709",
        TransferFunction::HLG => "arib-std-b67",
        TransferFunction::PQ => "smpte2084",
        TransferFunction::Linear => "linear",
        TransferFunction::SLog3 | TransferFunction::VLog
        | TransferFunction::ARRIlog3 | TransferFunction::CLog3
        | TransferFunction::FLog2 | TransferFunction::AppleLog
        | TransferFunction::AppleLog2 | TransferFunction::ACESCCT
        | TransferFunction::DaVinciIntermediate => "bt709",
        TransferFunction::Gamma24 => "bt709",
    };

    vec!["-color_primaries", primaries, "-color_trc", trc, "-colorspace", matrix]
}

// ---------------------------------------------------------------------------
// CinemaDNG export — COMING SOON
// Future: export RAW video sequence with LJ92 lossless compression fully
// compatible with DaVinci Resolve. Will add ProjFS (Windows), FUSE (Linux),
// and the macOS equivalent for folder mounting alongside the export.
// ---------------------------------------------------------------------------

pub fn run_naked(info: &McrawFileInfo, output_path: &str) -> Result<()> {
    tracing::info!("run_naked: input={} output={}", info.path, output_path);
    let decoder = Decoder::new(&info.path)?;
    let timestamps = decoder.timestamps()?;

    if timestamps.is_empty() {
        return Err(anyhow!("No frames found in file"));
    }

    tracing::debug!("run_naked: {} frames to dump", timestamps.len());

    let mut out_file = fs::File::create(output_path)?;

    for ts in &timestamps {
        let (bayer, _meta) = decoder.load_frame(*ts)?;

        for &v in &bayer {
            out_file.write_all(&v.to_le_bytes())?;
        }
    }

    Ok(())
}

pub fn run(info: &McrawFileInfo, output_path: &str) -> Result<()> {
    let never_cancel = Arc::new(AtomicBool::new(false));
    run_export(
        info.clone(),
        output_path.to_string(),
        Arc::new(|_| {}),
        never_cancel,
        ColorSpace::Rec709,
        TransferFunction::Rec709,
        CodecFamily::ProRes,
        ProResProfile::HQ,
        DnxhrProfile::HQX,
        HevcProfile::Main10_420,
        H264Profile::Main_8bit,
        Av1Profile::Profile0_420_10bit,
        Vp9Profile::Profile2_420_10bit,
        "libx265".to_string(),
        "libx264".to_string(),
        "libaom-av1".to_string(),
        "prores_ks".to_string(),
        RateControl::Lossless,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_export(
    info: McrawFileInfo,
    output_path: String,
    on_progress: Arc<dyn Fn(f64) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
    export_cs: ColorSpace,
    export_tf: TransferFunction,
    codec_family: CodecFamily,
    prores_profile: ProResProfile,
    dnxhr_profile: DnxhrProfile,
    hevc_profile: HevcProfile,
    h264_profile: H264Profile,
    av1_profile: Av1Profile,
    vp9_profile: Vp9Profile,
    hevc_encoder: String,
    h264_encoder: String,
    av1_encoder: String,
    prores_encoder: String,
    rate_control: RateControl,
) -> Result<()> {
    tracing::info!("run_export: input={} output={} codec={} cs={} tf={}",
        info.path, output_path, codec_family.name(), export_cs.name(), export_tf.name());
    let decoder = Decoder::new(&info.path)?;
    let timestamps = decoder.timestamps()?;

    if timestamps.is_empty() {
        return Err(anyhow!("No frames found in file"));
    }

    let stride_width = info.width as u32;
    let offset_x = info.active_offset_x as u32;
    let offset_y = info.active_offset_y as u32;
    let active_width = if info.active_width > 0 { info.active_width as u32 } else { stride_width };
    let active_height = if info.active_height > 0 { info.active_height as u32 } else { info.height as u32 };

    if active_width == 0 || active_height == 0 {
        return Err(anyhow!("Invalid active dimensions: {}x{}", active_width, active_height));
    }

    let fps = if info.fps > 0.0 { info.fps } else { 25.0 };
    tracing::info!("export config: {}x{} @ {}fps, {} frames, bayer={}",
        active_width, active_height, fps, timestamps.len(), info.bayer_pattern.name());

    // --- Build Forward Matrix (Camera→XYZ) from available DNG matrices ---
    // ForwardMatrix is the correct way to convert camera RGB to XYZ
    let fm1_f32: Option<[f32; 9]> = info.camera_metadata.forward_matrix1.map(|fm| {
        let mut f = [0.0f32; 9];
        for (i, v) in fm.iter().enumerate() { f[i] = *v as f32; }
        f
    });

    let fm2_f32: Option<[f32; 9]> = info.camera_metadata.forward_matrix2.map(|fm| {
        let mut f = [0.0f32; 9];
        for (i, v) in fm.iter().enumerate() { f[i] = *v as f32; }
        f
    });

    let cal1_f32: Option<[f32; 9]> = info.camera_metadata.calibration_matrix1.map(|cm| {
        let mut ccm = [0.0f32; 9];
        for (i, v) in cm.iter().enumerate() { ccm[i] = *v as f32; }
        ccm
    });
    let cal2_f32: Option<[f32; 9]> = info.camera_metadata.calibration_matrix2.map(|cm| {
        let mut ccm = [0.0f32; 9];
        for (i, v) in cm.iter().enumerate() { ccm[i] = *v as f32; }
        ccm
    });

    // Build Camera→XYZ matrix using ForwardMatrix approach
    // NOTE: ForwardMatrix1 is already D50-adapted and includes calibration baked in.
    // DO NOT multiply by calibration matrices again - that would be double-calibration.
    // See DNG spec: ForwardMatrix1 = ColorMatrix1^(-1) × CalMatrix1^(-1)
    let cam_to_xyz: [f32; 9] = match (fm1_f32, fm2_f32, cal1_f32, cal2_f32) {
        (Some(ref fm1), Some(ref fm2), Some(ref c1), Some(ref c2)) => {
            // Apply calibration FIRST (cal * fm), not fm * cal
            let eff_fm1 = mat_mul_3x3(c1, fm1);
            let eff_fm2 = mat_mul_3x3(c2, fm2);
            interpolate_matrix(&eff_fm1, &eff_fm2, 0.5)
        }
        (Some(ref fm1), Some(ref fm2), _, _) => {
            interpolate_matrix(fm1, fm2, 0.5)
        }
        (Some(ref fm1), None, Some(ref c1), None) => {
            // Apply calibration FIRST
            mat_mul_3x3(c1, fm1)
        }
        (Some(ref fm1), None, None, None) => {
            *fm1
        }
        _ => {
            // Fallback to ColorMatrix if no ForwardMatrix available
            let cm1_f32: [f32; 9] = info.camera_metadata.color_matrix
                .map(|cm| {
                    let mut ccm = [0.0f32; 9];
                    for (i, v) in cm.iter().enumerate() { ccm[i] = *v as f32; }
                    ccm
                })
                .unwrap_or_else(identity_ccm);
            camera_to_xyz_matrix(&cm1_f32, None)
        }
    };

    // For scene-referred professional codecs (ProRes, DNxHR, HEVC) that will be graded later,
    // we use a SIMPLE Camera→Output matrix WITHOUT CAT16 adaptation.
    // CAT16 should be applied during GRADING in Davinci Resolve, not baked into the encode.
    // This preserves the scene colorimetry for maximum flexibility in post.
    let xyz_to_output = export_cs.get_xyz_to_rgb_matrix();
    let cam_to_output = mat_mul_3x3(&xyz_to_output, &cam_to_xyz);

    let is_log = export_tf.is_log_bypass();

    let pattern = info.bayer_pattern;
    let black_level = info.black_level;
    let white_level = info.white_level;
    let total_frames = timestamps.len();

    let (codec_name, pix_fmt, mut extra_args) = build_ffmpeg_codec_args(
        codec_family, &hevc_encoder, &h264_encoder, &av1_encoder, &prores_encoder,
        prores_profile, dnxhr_profile, hevc_profile,
        h264_profile, av1_profile, vp9_profile,
        &rate_control,
    );

    let vui_tags = get_ffmpeg_vui_tags(&export_cs, &export_tf);
    extra_args.extend(vui_tags.into_iter().map(String::from));

    let xyz_to_rec = xyz_to_rec709();

    // -----------------------------------------------------------------
    // Audio pipeline
    //
    // Data flow:
    //   1. decoder.write_audio_to() writes each audio chunk as raw s16le
    //      to a temp file on disk via BufWriter (never holds all samples
    //      in memory — safe for hours-long recordings).
    //   2. Temp file path passed to VideoEncoder, which adds a second
    //      -i input to FFmpeg:  -f s16le -ar RATE -ac CH -i <tempfile>
    //   3. FFmpeg muxes audio alongside video; encoder Drop cleans up
    //
    // Future: add a user toggle (App::export_audio_enabled) gating the
    // entire block below.  The toggle lives at App level so it can be
    // rendered in the Export screen.  When disabled, skip to
    //   let audio_temp_path: Option<PathBuf> = None;
    // -----------------------------------------------------------------
    let audio_temp_path = if info.has_audio
        && info.audio_sample_rate > 0
        && info.audio_channels > 0
    {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp_path = std::env::temp_dir().join(format!("mcraw_audio_{}.raw", ts));
        match std::fs::File::create(&temp_path) {
            Ok(file) => {
                let mut writer = BufWriter::new(file);
                match decoder.write_audio_to(&mut writer) {
                    Ok(()) => {
                        let _ = writer.flush();
                        tracing::info!(
                            "audio streamed to temp file: {} Hz, {} ch -> {}",
                            info.audio_sample_rate, info.audio_channels,
                            temp_path.display()
                        );
                        Some(temp_path)
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&temp_path);
                        tracing::warn!("failed to write audio (export continues without audio): {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!("failed to create audio temp file (export continues without audio): {}", e);
                None
            }
        }
    } else {
        None
    };

    let mut encoder = VideoEncoder::new(
        &output_path, active_width, active_height, fps,
        &codec_name, &pix_fmt, &extra_args,
        audio_temp_path.as_deref(),
        info.audio_sample_rate,
        info.audio_channels,
    )?;

    // ----------------------------------------------------------------
    // Pre-allocate the buffer pool (eliminates per-frame heap churn)
    // ----------------------------------------------------------------
    let stride_pixels = stride_width as usize * info.height as usize;
    let pixel_count = (active_width * active_height) as usize;
    let bytes_per_frame = pixel_count * 6;

    let (free_tx, free_rx) = bounded::<FrameSlot>(PIPELINE_DEPTH);
    let (loaded_tx, loaded_rx) = bounded::<FrameSlot>(PIPELINE_DEPTH);
    let (processed_tx, processed_rx) = bounded::<FrameSlot>(PIPELINE_DEPTH);

    for _ in 0..PIPELINE_DEPTH {
        free_tx.send(FrameSlot {
            bayer: vec![0u16; stride_pixels],
            frame_bytes: vec![0u8; bytes_per_frame],
            as_shot_neutral: [0.0; 3],
        })?;
    }

    let free_tx_writer = free_tx.clone();

    // ----------------------------------------------------------------
    // Initialize GPU RCD pipeline (best-effort, fallback to CPU on failure)
    // ----------------------------------------------------------------
    let filters = pattern.to_dcraw_filters();
    let mut rcd_pipeline: Option<gpu::RcdPipeline> = None;
    // For debug diagnostics
    // Diagnostics: write path info to file alongside export output
    use std::io::Write;
    let diag_dir = std::path::Path::new(&output_path).parent().unwrap_or(std::path::Path::new(".")).to_owned();
    let diag_path = diag_dir.join("rcd_diag.txt");
    let mut diag_file = std::fs::File::create(&diag_path).ok();
    macro_rules! diag { ($($arg:tt)*) => { if let Some(ref mut f) = diag_file { let _ = writeln!(f, $($arg)*); } }; }
    diag!("active={}x{} stride={} offset={},{} bl={} wl={}", active_width, active_height, stride_width, offset_x, offset_y, black_level, white_level);
    diag!("pattern={:?} filters=0x{:08x}", pattern, filters);
    diag!("GPU init...");
    match pollster::block_on(gpu::GpuContext::new()) {
        Ok(ctx) => {
            let ctx = std::sync::Arc::new(ctx);
            match gpu::RcdPipeline::new(ctx, active_width, active_height) {
                Ok(pipeline) => {
                    diag!("GPU RCD pipeline OK");
                    tracing::info!("GPU RCD pipeline initialized ({}x{})", active_width, active_height);
                    rcd_pipeline = Some(pipeline);
                }
                Err(e) => {
                    diag!("GPU RCD FAILED: {}", e);
                    tracing::warn!("Failed to create RCD pipeline: {} — falling back to CPU demosaic", e);
                }
            }
        }
        Err(e) => {
            diag!("No GPU adapter: {}", e);
            tracing::warn!("No GPU adapter found: {} — falling back to CPU demosaic", e);
        }
    }

    // ==================================================================
    // Stage 1 — Loader thread: raw frame I/O
    // ==================================================================
    let loader_handle = std::thread::Builder::new()
        .name("loader".into())
        .spawn({
            let cancelled = cancelled.clone();
            move || -> Result<()> {
                let mut t_load_us: f64 = 0.0;
                let mut frame_count: u64 = 0;

                for ts in &timestamps {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }

                    let mut slot = free_rx
                        .recv()
                        .map_err(|_| anyhow!("Loader: free pool closed prematurely"))?;

                    let t0 = std::time::Instant::now();
                    let (bayer, frame_meta) = decoder.load_frame(*ts)?;
                    t_load_us += t0.elapsed().as_secs_f64() * 1e6;

                    slot.bayer = bayer;
                    slot.as_shot_neutral = frame_meta.as_shot_neutral;

                    loaded_tx
                        .send(slot)
                        .map_err(|_| anyhow!("Loader: processor channel closed"))?;

                    frame_count += 1;
                }

                if frame_count > 0 {
                    let avg_load = t_load_us / frame_count as f64;
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let msg = format!("[{}] loader  {:>4} frames: load={:.1}us  per-frame\n", ts, frame_count, avg_load);
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open("timing_results.txt")
                    {
                        let _ = std::io::Write::write_all(&mut f, msg.as_bytes());
                    }
                }

                drop(loaded_tx);
                Ok(())
            }
        })?;

    // Pre-build AgX pipeline — disabled for now; will be re-added as a
    // dedicated AgX section with full parameter control when preview is ready.
    let use_agx = false;
    let agx_pipeline = if use_agx {
        let mut cfg = AgxConfig::default();
        cfg.in_gamut = Gamut::Rec709;
        cfg.in_transfer = Transfer::Linear;
        cfg.working_curve = Transfer::AgxLogKraken;
        cfg.out_gamut = Gamut::Rec709;
        cfg.out_transfer = OutputTransfer::Bt1886InverseEotf;
        cfg.log_output = false;
        Some(AgxPipeline::new(cfg))
    } else {
        None
    };

    // ==================================================================
    // Stage 2 — Processor thread: GPU RCD → CPU fallback → color → OETF
    // ==================================================================
    let processor_handle = std::thread::Builder::new()
        .name("processor".into())
        .spawn({
            let cancelled = cancelled.clone();
            move || -> Result<()> {
                let mut rgb = vec![0.0f32; pixel_count * 3];
                let demosaic = BilinearDemosaic::new(pattern);

                let mut prev_as_shot: Option<[f32; 3]> = None;
                let mut wb_change_count = 0u64;

                // Accumulated microsecond counters for benchmarking
                let mut t_demosaic_us: f64 = 0.0;
                let mut t_normalize_us: f64 = 0.0;
                let mut t_color_us: f64 = 0.0;
                let mut t_convert_us: f64 = 0.0;
                let mut t_proc_total_us: f64 = 0.0;
                let mut frame_count: u64 = 0;

                for mut slot in loaded_rx {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }

                    let t_frame_start = std::time::Instant::now();
                    frame_count += 1;
                    
                    // Clear frame buffer to prevent stale data from previous frames
                    slot.frame_bytes.fill(0);
                    
                    let as_shot = slot.as_shot_neutral;
                    let wb_r_debug = if as_shot[0] > 1e-6 && as_shot[1] > 1e-6 {
                        as_shot[1] / as_shot[0]
                    } else {
                        1.0
                    };
                    let wb_b_debug = if as_shot[2] > 1e-6 && as_shot[1] > 1e-6 {
                        as_shot[1] / as_shot[2]
                    } else {
                        1.0
                    };
                    if let Some(prev) = prev_as_shot {
                        let delta_r = (as_shot[0] - prev[0]).abs();
                        let delta_b = (as_shot[2] - prev[2]).abs();
                        if delta_r > 0.05 || delta_b > 0.05 {
                            wb_change_count += 1;
                            tracing::warn!(
                                "Frame {}: WB shift detected - ΔR={:.3}, ΔB={:.3} (may cause color flicker)",
                                frame_count, delta_r, delta_b
                            );
                        }
                    }
                    prev_as_shot = Some(as_shot);

                    // Use the pre-computed Camera→Output matrix with CAT16 adaptation
                    // WB multipliers will be applied in the shader
                    let fused = cam_to_output;

                    if frame_count == 1 {
                        let _ = std::fs::write("wb_debug.txt",
                            format!("Frame 1 WB: as_shot=[{:.4}, {:.4}, {:.4}] wb_r={:.4} wb_b={:.4}\n\
 cam_to_output matrix:\n  [{:.4}, {:.4}, {:.4}]\n  [{:.4}, {:.4}, {:.4}]\n  [{:.5}, {:.5}, {:.5}]\n",
                                as_shot[0], as_shot[1], as_shot[2], wb_r_debug, wb_b_debug,
                                fused[0], fused[1], fused[2],
                                fused[3], fused[4], fused[5],
                                fused[6], fused[7], fused[8]));
                    }

                    // Try GPU RCD (full color pipeline on-GPU, returns packed u8 RGB)
                    let t0 = std::time::Instant::now();
                    let gpu_ok = if let Some(ref mut pipeline) = rcd_pipeline {
                        match pipeline.process(&slot.bayer, filters, black_level as f32, white_level as f32, stride_width, offset_x, offset_y, &fused, &slot.as_shot_neutral) {
                            Ok(packed) => {
                                t_demosaic_us += t0.elapsed().as_secs_f64() * 1e6;
                                // Unpack packed u32 → u16 LE frame_bytes
                                slot.frame_bytes.par_chunks_exact_mut(6).enumerate().for_each(|(pi, out)| {
                                    let p = packed[pi];
                                    let r = (p & 0xFF) as u16 * 257;
                                    let g = ((p >> 8) & 0xFF) as u16 * 257;
                                    let b = ((p >> 16) & 0xFF) as u16 * 257;
                                    out[0] = r as u8;
                                    out[1] = (r >> 8) as u8;
                                    out[2] = g as u8;
                                    out[3] = (g >> 8) as u8;
                                    out[4] = b as u8;
                                    out[5] = (b >> 8) as u8;
                                });
                                true
                            }
                            Err(e) => {
                                diag!("GPU RCD process FAILED: {} — falling back to CPU", e);
                                tracing::warn!("GPU RCD failed for frame: {} — falling back to CPU", e);
                                false
                            }
                        }
                    } else {
                        diag!("GPU RCD not available — using CPU bilinear");
                        false
                    };

                    if !gpu_ok {
                        let t_cpu = std::time::Instant::now();
                        demosaic.process_par_into(
                            &slot.bayer, stride_width, offset_x, offset_y,
                            active_width, active_height, &pattern, &mut rgb,
                        )?;
                        t_demosaic_us += t_cpu.elapsed().as_secs_f64() * 1e6;
                        let t1 = std::time::Instant::now();
                        normalize_linear_f32(&mut rgb, black_level as f32, white_level as f32);
                        t_normalize_us += t1.elapsed().as_secs_f64() * 1e6;

                        let wb_r = if as_shot[0] > 1e-6 && as_shot[1] > 1e-6 {
                            as_shot[1] / as_shot[0]
                        } else {
                            1.0
                        };
                        let wb_b = if as_shot[2] > 1e-6 && as_shot[1] > 1e-6 {
                            as_shot[1] / as_shot[2]
                        } else {
                            1.0
                        };

                        let use_agx_frame = use_agx;
                        let t2 = std::time::Instant::now();
                        rgb.par_chunks_exact_mut(3).for_each(|chunk| {
                            if use_agx_frame {
                                let rr = chunk[0];
                                let gg = chunk[1];
                                let bb = chunk[2];
                                let out = mat_mul_vec3(&fused, &[rr, gg, bb]);
                                chunk[0] = out[0].max(0.0);
                                chunk[1] = out[1].max(0.0);
                                chunk[2] = out[2].max(0.0);
                            } else {
                                let raw_r = chunk[0];
                                let raw_g = chunk[1];
                                let raw_b = chunk[2];

                                // Apply White Balance gains
                                let rw = raw_r * wb_r;
                                let gw = raw_g;
                                let bw = raw_b * wb_b;

                                // Apply combined matrix (ForwardMatrix + CAT16 + OutputMatrix)
                                let out = mat_mul_vec3(&fused, &[rw, gw, bw]);
                                chunk[0] = out[0].max(0.0);
                                chunk[1] = out[1].max(0.0);
                                chunk[2] = out[2].max(0.0);
                            }
                        });

                        if let Some(ref agx) = agx_pipeline {
                            agx.process_frame(&mut rgb);
                        } else if is_log {
                            export_tf.process(&mut rgb);
                        } else {
                            rgb.par_iter_mut().for_each(|v| {
                                if *v < 0.0 { *v = 0.0; }
                                *v = if *v < 0.018 { 4.5 * *v } else { 1.099 * v.powf(0.45) - 0.099 };
                            });
                        }
                        t_color_us += t2.elapsed().as_secs_f64() * 1e6;

                        let t3 = std::time::Instant::now();
                        slot.frame_bytes.par_chunks_exact_mut(6).enumerate().for_each(|(pi, out)| {
                            let base = pi * 3;
                            let ru = (rgb[base].clamp(0.0, 1.0) * 65535.0) as u16;
                            let gu = (rgb[base + 1].clamp(0.0, 1.0) * 65535.0) as u16;
                            let bu = (rgb[base + 2].clamp(0.0, 1.0) * 65535.0) as u16;
                            out[0] = ru as u8;
                            out[1] = (ru >> 8) as u8;
                            out[2] = gu as u8;
                            out[3] = (gu >> 8) as u8;
                            out[4] = bu as u8;
                            out[5] = (bu >> 8) as u8;
                        });
                        t_convert_us += t3.elapsed().as_secs_f64() * 1e6;
                    }

                    t_proc_total_us += t_frame_start.elapsed().as_secs_f64() * 1e6;

                    processed_tx
                        .send(slot)
                        .map_err(|_| anyhow!("Processor: writer channel closed"))?;
                }

                if frame_count > 0 {
                    let avg_demosaic = t_demosaic_us / frame_count as f64;
                    let avg_normalize = t_normalize_us / frame_count as f64;
                    let avg_color = t_color_us / frame_count as f64;
                    let avg_convert = t_convert_us / frame_count as f64;
                    let avg_proc = t_proc_total_us / frame_count as f64;
                    let avg_sub = avg_demosaic + avg_normalize + avg_color + avg_convert;
                    let fps = 1e6 / avg_proc.max(1.0);
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let msg = format!(
                        "[{}] proc     {:>4} frames: demosaic={:.1}us  normalize={:.1}us  color={:.1}us  convert={:.1}us  subtotal={:.1}us  wall={:.1}us ({:.1} fps)\n",
                        ts, frame_count, avg_demosaic, avg_normalize, avg_color, avg_convert, avg_sub, avg_proc, fps,
                    );
                    tracing::info!("{}", msg.trim());
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open("timing_results.txt")
                    {
                        let _ = std::io::Write::write_all(&mut f, msg.as_bytes());
                    }
                    
                    if wb_change_count > 0 {
                        tracing::warn!(
                            "Color stability report: {} frames had WB shifts >5% - this may cause visible color flicker during playback",
                            wb_change_count
                        );
                    }
                }

                drop(processed_tx);
                Ok(())
            }
        })?;

    // ==================================================================
    // Stage 3 — Writer thread: drain processed frames to FFmpeg stdin
    // ==================================================================
    let writer_cancelled = cancelled.clone();

    // Capture FFmpeg PID so a monitor thread can kill it on cancellation.
    // This unblocks the writer thread if it is stuck inside push_frame().
    let ffmpeg_pid = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    ffmpeg_pid.store(encoder.pid(), Ordering::Relaxed);

    // Spawn a cancellation monitor that force-kills FFmpeg when the user
    // cancels.  Without this the writer thread can hang indefinitely on
    // macOS (and occasionally Linux/Windows) because stdin.write_all() is
    // a blocking call that never reaches the cancel-flag check.
    let monitor_cancelled = cancelled.clone();
    let monitor_pid = ffmpeg_pid.clone();
    let _monitor_handle = std::thread::Builder::new()
        .name("cancel_monitor".into())
        .spawn(move || {
            while !monitor_cancelled.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // Cancel flag was set — kill FFmpeg to unblock the writer.
            let pid = monitor_pid.load(Ordering::Relaxed);
            if pid > 0 {
                #[cfg(target_os = "windows")]
                {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/F", "/PID", &pid.to_string()])
                        .output();
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = std::process::Command::new("kill")
                        .args(["-TERM", &pid.to_string()])
                        .output();
                }
            }
        })
        .ok();

    let writer_handle = std::thread::Builder::new()
        .name("writer".into())
        .spawn(move || -> Result<()> {
            let mut frames_written: usize = 0;
            let mut t_write_us: f64 = 0.0;

            for slot in processed_rx {
                if writer_cancelled.load(Ordering::Relaxed) {
                    // Return slot to pool before exiting
                    let _ = free_tx_writer.send(slot);
                    // Skip finish() — encoder drops here, killing FFmpeg
                    return Ok(());
                }

                let t0 = std::time::Instant::now();
                encoder.push_frame(&slot.frame_bytes)?;
                t_write_us += t0.elapsed().as_secs_f64() * 1e6;

                frames_written += 1;
                on_progress(frames_written as f64 / total_frames as f64 * 100.0);

                let _ = free_tx_writer.send(slot);
            }

            if frames_written > 0 {
                let avg_write = t_write_us / frames_written as f64;
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let msg = format!("[{}] writer  {:>4} frames: push={:.1}us  per-frame\n",
                    ts, frames_written, avg_write);
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("timing_results.txt")
                {
                    let _ = std::io::Write::write_all(&mut f, msg.as_bytes());
                }
            }

            // Double-check cancel flag: processed_rx may have been dropped
            // by the processor after the processor detected cancellation.
            if writer_cancelled.load(Ordering::Relaxed) {
                return Ok(());
            }

            encoder.finish()?;
            Ok(())
        })?;

    // ==================================================================
    // Join all stages & propagate the first error
    // ==================================================================
    let mut export_error: Option<anyhow::Error> = None;

    match loader_handle.join() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            cancelled.store(true, Ordering::Relaxed);
            export_error = Some(e);
        }
        Err(_) => {
            cancelled.store(true, Ordering::Relaxed);
            export_error = Some(anyhow!("Loader thread panicked"));
        }
    }

    match processor_handle.join() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            cancelled.store(true, Ordering::Relaxed);
            export_error.get_or_insert(e);
        }
        Err(_) => {
            cancelled.store(true, Ordering::Relaxed);
            export_error.get_or_insert(anyhow!("Processor thread panicked"));
        }
    }

    match writer_handle.join() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            export_error.get_or_insert(e);
        }
        Err(_) => {
            export_error.get_or_insert(anyhow!("Writer thread panicked"));
        }
    }

    // Clean up audio temp file (belt-and-suspenders with encoder Drop)
    if let Some(ref audio_path) = audio_temp_path {
        let _ = std::fs::remove_file(audio_path);
    }

    match export_error {
        Some(_) if cancelled.load(Ordering::Relaxed) => {
            Err(anyhow!("Export cancelled by user"))
        }
        Some(e) => Err(e),
        None => Ok(()),
    }
}
