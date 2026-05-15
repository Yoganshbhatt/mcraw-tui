use crate::color::{
    normalize_linear_f32, identity_ccm,
    mat_mul_vec3, camera_to_xyz_matrix, interpolate_matrix,
    build_cat16_output_matrix,
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
use anyhow::{anyhow, Result};
use crossbeam_channel::bounded;
use rayon::prelude::*;
use std::fs;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Number of frame slots in the producer-consumer pool.
const PIPELINE_DEPTH: usize = 3;

/// A pre-allocated buffer slot that circulates through the pipeline stages.
struct FrameSlot {
    bayer: Vec<u16>,
    frame_bytes: Vec<u8>,
    as_shot_neutral: [f32; 3],
}

/// Per-stage nanosecond accumulators (shared across threads via atomics).
struct ProfileTimers {
    p1_load: AtomicU64,
    p2_demosaic: AtomicU64,
    p3_process: AtomicU64,
    p4_push: AtomicU64,
}

impl ProfileTimers {
    fn new() -> Self {
        Self {
            p1_load: AtomicU64::new(0),
            p2_demosaic: AtomicU64::new(0),
            p3_process: AtomicU64::new(0),
            p4_push: AtomicU64::new(0),
        }
    }
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
    prores: ProResProfile,
    dnxhr: DnxhrProfile,
    hevc: HevcProfile,
    h264: H264Profile,
    av1: Av1Profile,
    vp9: Vp9Profile,
    rate_control: &RateControl,
) -> (&'static str, &'static str, Vec<String>) {
    family.to_ffmpeg_args(hevc_encoder, h264_encoder, av1_encoder, prores, dnxhr, hevc, h264, av1, vp9, rate_control)
}

pub fn get_ffmpeg_vui_tags(color_space: &ColorSpace, transfer: &TransferFunction) -> Vec<&'static str> {
    let (primaries, matrix) = match color_space {
        ColorSpace::Rec709 | ColorSpace::Srgb => ("bt709", "bt709"),
        ColorSpace::Rec2020 => ("bt2020", "bt2020nc"),
        ColorSpace::DciP3 | ColorSpace::AppleDisplayP3 => ("smpte432", "bt2020nc"),
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
        | TransferFunction::FLog2 | TransferFunction::ACESCCT => "bt709",
    };

    vec!["-color_primaries", primaries, "-color_trc", trc, "-colorspace", matrix]
}

pub fn run_naked(info: &McrawFileInfo, output_path: &str) -> Result<()> {
    let decoder = Decoder::new(&info.path)?;
    let timestamps = decoder.timestamps()?;

    if timestamps.is_empty() {
        return Err(anyhow!("No frames found in file"));
    }

    let mut out_file = fs::File::create(output_path)?;

    for ts in &timestamps {
        let (bayer_bytes, _meta) = decoder.load_frame(*ts)?;
        let bayer_u16: Vec<u16> = bayer_bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();

        for &v in &bayer_u16 {
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
    rate_control: RateControl,
) -> Result<()> {
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

    // --- Build Camera→XYZ matrix from available DNG matrices ---
    let cm1_f32: [f32; 9] = info.camera_metadata.color_matrix
        .map(|cm| {
            let mut ccm = [0.0f32; 9];
            for (i, v) in cm.iter().enumerate() { ccm[i] = *v as f32; }
            ccm
        })
        .unwrap_or_else(identity_ccm);

    let cm2_f32: Option<[f32; 9]> = info.camera_metadata.color_matrix2.map(|cm| {
        let mut ccm = [0.0f32; 9];
        for (i, v) in cm.iter().enumerate() { ccm[i] = *v as f32; }
        ccm
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

    let cam_to_xyz: [f32; 9] = match (cm2_f32, cal1_f32, cal2_f32) {
        (Some(ref cm2), Some(ref c1), Some(ref c2)) => {
            let raw_cm1 = camera_to_xyz_matrix(&cm1_f32, Some(c1));
            let raw_cm2 = camera_to_xyz_matrix(cm2, Some(c2));
            interpolate_matrix(&raw_cm1, &raw_cm2, 0.5)
        }
        (Some(ref cm2), _, _) => {
            let raw_cm1 = camera_to_xyz_matrix(&cm1_f32, None);
            let raw_cm2 = camera_to_xyz_matrix(cm2, None);
            interpolate_matrix(&raw_cm1, &raw_cm2, 0.5)
        }
        _ => {
            camera_to_xyz_matrix(&cm1_f32, None)
        }
    };

    let is_log = export_tf.is_log_bypass();
    let target_xyz_to_rgb = export_cs.get_xyz_to_rgb_matrix();

    let pattern = info.bayer_pattern;
    let black_level = info.black_level;
    let white_level = info.white_level;
    let total_frames = timestamps.len();

    let (codec_name, pix_fmt, mut extra_args) = build_ffmpeg_codec_args(
        codec_family, &hevc_encoder, &h264_encoder, &av1_encoder,
        prores_profile, dnxhr_profile, hevc_profile,
        h264_profile, av1_profile, vp9_profile,
        &rate_control,
    );

    let vui_tags = get_ffmpeg_vui_tags(&export_cs, &export_tf);
    extra_args.extend(vui_tags.into_iter().map(String::from));

    let xyz_to_rec = xyz_to_rec709();

    let mut encoder = VideoEncoder::new(
        &output_path, active_width, active_height, fps,
        codec_name, pix_fmt, &extra_args,
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

    let timers = Arc::new(ProfileTimers::new());
    let free_tx_writer = free_tx.clone();

    // ==================================================================
    // Stage 1 — Loader thread: raw frame I/O + byte-swap
    // ==================================================================
    let loader_handle = std::thread::Builder::new()
        .name("loader".into())
        .spawn({
            let cancelled = cancelled.clone();
            let timers = timers.clone();
            move || -> Result<()> {
                for ts in &timestamps {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }

                    let t0 = Instant::now();

                    let mut slot = free_rx
                        .recv()
                        .map_err(|_| anyhow!("Loader: free pool closed prematurely"))?;

                    let (bayer_bytes, frame_meta) = decoder.load_frame(*ts)?;

                    // Byte-swap directly into the pre-allocated buffer
                    let dst = &mut slot.bayer[..bayer_bytes.len() / 2];
                    for (i, chunk) in bayer_bytes.chunks_exact(2).enumerate() {
                        dst[i] = u16::from_le_bytes([chunk[0], chunk[1]]);
                    }
                    slot.as_shot_neutral = frame_meta.as_shot_neutral;

                    let t1 = t0.elapsed().as_nanos() as u64;
                    timers.p1_load.fetch_add(t1, Ordering::Relaxed);

                    loaded_tx
                        .send(slot)
                        .map_err(|_| anyhow!("Loader: processor channel closed"))?;
                }

                drop(loaded_tx);
                Ok(())
            }
        })?;

    // ==================================================================
    // Stage 2 — Processor thread: demosaic → color pipeline → OETF → u16
    // ==================================================================
    let processor_handle = std::thread::Builder::new()
        .name("processor".into())
        .spawn({
            let cancelled = cancelled.clone();
            let timers = timers.clone();
            move || -> Result<()> {
                // Pre-allocate the RGB working buffer ONCE for all frames
                let mut rgb = vec![0.0f32; pixel_count * 3];
                let demosaic = BilinearDemosaic::new(pattern);

                for mut slot in loaded_rx {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }

                    // --- Per-frame white balance from metadata ---
                    let as_shot = slot.as_shot_neutral;
                    let (r_gain, b_gain) = if as_shot[0] > 1e-6 && as_shot[1] > 1e-6 && as_shot[2] > 1e-6 {
                        (as_shot[1] / as_shot[0], as_shot[1] / as_shot[2])
                    } else {
                        (1.0, 1.0)
                    };

                    let scene_white_xyz: [f32; 3] = if as_shot[0] > 1e-6 && as_shot[1] > 1e-6 && as_shot[2] > 1e-6 {
                        mat_mul_vec3(&cam_to_xyz, &as_shot)
                    } else {
                        [D65_XYZ[0], D65_XYZ[1], D65_XYZ[2]]
                    };

                    let output_matrix = if is_log { &target_xyz_to_rgb } else { &xyz_to_rec };
                    let fused = build_cat16_output_matrix(&cam_to_xyz, &scene_white_xyz, &D65_XYZ, output_matrix);

                    // -- Phase 2: demosaic + normalize (into pre-allocated rgb) --
                    let t1 = Instant::now();
                    demosaic.process_par_into(
                        &slot.bayer, stride_width, offset_x, offset_y,
                        active_width, active_height, &pattern, &mut rgb,
                    )?;
                    normalize_linear_f32(&mut rgb, black_level as f32, white_level as f32);
                    let t2 = t1.elapsed().as_nanos() as u64;
                    timers.p2_demosaic.fetch_add(t2, Ordering::Relaxed);

                    // -- Phase 3: pixel processing (fused pass + OETF + byte conversion) --
                    let t2_total = Instant::now();

                    // WB gain → highlight clip → revert WB → single 3×3 matrix multiply
                    rgb.par_chunks_exact_mut(3).for_each(|chunk| {
                        let r = chunk[0] * r_gain;
                        let g = chunk[1];
                        let b = chunk[2] * b_gain;

                        let max_val = r.max(g).max(b);
                        let (r, g, b) = if max_val > 0.95f32 {
                            let t = ((max_val - 0.95) / 0.05).min(1.0);
                            (r + (max_val - r) * t,
                             g + (max_val - g) * t,
                             b + (max_val - b) * t)
                        } else {
                            (r, g, b)
                        };

                        let rr = r / r_gain;
                        let bb = b / b_gain;

                        let out = mat_mul_vec3(&fused, &[rr, g, bb]);
                        chunk[0] = out[0].max(0.0);
                        chunk[1] = out[1].max(0.0);
                        chunk[2] = out[2].max(0.0);
                    });

                    // Apply OETF / log encoding
                    if is_log {
                        export_tf.process(&mut rgb);
                    } else {
                        rgb.par_iter_mut().for_each(|v| {
                            if *v < 0.0 { *v = 0.0; }
                            *v = if *v < 0.018 { 4.5 * *v } else { 1.099 * v.powf(0.45) - 0.099 };
                        });
                    }

                    // Convert f32 → u16 bytes (into pre-allocated frame_bytes)
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

                    let t3 = t2_total.elapsed().as_nanos() as u64;
                    timers.p3_process.fetch_add(t3, Ordering::Relaxed);

                    processed_tx
                        .send(slot)
                        .map_err(|_| anyhow!("Processor: writer channel closed"))?;
                }

                drop(processed_tx);
                Ok(())
            }
        })?;

    // ==================================================================
    // Stage 3 — Writer thread: drain processed frames to FFmpeg stdin
    // ==================================================================
    let writer_cancelled = cancelled.clone();
    let writer_timers = timers.clone();
    let writer_handle = std::thread::Builder::new()
        .name("writer".into())
        .spawn(move || -> Result<()> {
            let mut frames_written: usize = 0;

            for slot in processed_rx {
                if writer_cancelled.load(Ordering::Relaxed) {
                    break;
                }

                let t3 = Instant::now();
                encoder.push_frame(&slot.frame_bytes)?;
                let t4 = t3.elapsed().as_nanos() as u64;
                writer_timers.p4_push.fetch_add(t4, Ordering::Relaxed);

                frames_written += 1;
                on_progress(frames_written as f64 / total_frames as f64 * 100.0);

                // Return slot to the free pool for reuse by the loader
                let _ = free_tx_writer.send(slot);
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

    // Log profile data
    let p1 = timers.p1_load.load(Ordering::Relaxed);
    let p2 = timers.p2_demosaic.load(Ordering::Relaxed);
    let p3 = timers.p3_process.load(Ordering::Relaxed);
    let p4 = timers.p4_push.load(Ordering::Relaxed);
    log_profile(p1, p2, p3, p4, total_frames);

    match export_error {
        Some(_) if cancelled.load(Ordering::Relaxed) => {
            Err(anyhow!("Export cancelled by user"))
        }
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn log_profile(p1: u64, p2: u64, p3: u64, p4: u64, frames: usize) {
    let fmt_ns = |ns: u64| -> String {
        if ns >= 1_000_000_000 {
            format!("{:.3}s", ns as f64 / 1_000_000_000.0)
        } else if ns >= 1_000_000 {
            format!("{:.1}ms", ns as f64 / 1_000_000.0)
        } else if ns >= 1_000 {
            format!("{:.1}µs", ns as f64 / 1_000.0)
        } else {
            format!("{ns}ns")
        }
    };
    let total = p1 + p2 + p3 + p4;
    if let Ok(mut f) = fs::File::create("ffmpeg_debug.txt") {
        let _ = writeln!(f, "=== Pipeline profile ({} frames) ===", frames);
        let _ = writeln!(f, "Phase 1  load_frame + byte-swap : {:>12}  {:>5.0}%", fmt_ns(p1), p1 as f64 / total as f64 * 100.0);
        let _ = writeln!(f, "Phase 2  demosaic + normalize   : {:>12}  {:>5.0}%", fmt_ns(p2), p2 as f64 / total as f64 * 100.0);
        let _ = writeln!(f, "Phase 3  pixel processing       : {:>12}  {:>5.0}%", fmt_ns(p3), p3 as f64 / total as f64 * 100.0);
        let _ = writeln!(f, "Phase 4  encoder.push_frame     : {:>12}  {:>5.0}%", fmt_ns(p4), p4 as f64 / total as f64 * 100.0);
        let _ = writeln!(f, "──────────────────────────────────────────────");
        let _ = writeln!(f, "Total                          : {:>12}", fmt_ns(total));
        let _ = writeln!(f, "Per-frame (1/{} frames)         : {:>12}", frames, fmt_ns(total / frames as u64));
    }
}
