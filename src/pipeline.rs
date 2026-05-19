use crate::agx::{AgxConfig, AgxPipeline, Gamut, OutputTransfer, Transfer};
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
        | TransferFunction::FLog2 | TransferFunction::ACESCCT
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
    let decoder = Decoder::new(&info.path)?;
    let timestamps = decoder.timestamps()?;

    if timestamps.is_empty() {
        return Err(anyhow!("No frames found in file"));
    }

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
                        log::info!(
                            "Audio streamed to temp file: {} Hz, {} ch -> {}",
                            info.audio_sample_rate, info.audio_channels,
                            temp_path.display()
                        );
                        Some(temp_path)
                    }
                    Err(e) => {
                        // Clean up temp file on partial write
                        let _ = std::fs::remove_file(&temp_path);
                        log::warn!("Failed to write audio (export continues without audio): {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                log::warn!("Failed to create audio temp file (export continues without audio): {}", e);
                None
            }
        }
    } else {
        None
    };

    let mut encoder = VideoEncoder::new(
        &output_path, active_width, active_height, fps,
        codec_name, pix_fmt, &extra_args,
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

    // ==================================================================
    // Stage 1 — Loader thread: raw frame I/O
    // ==================================================================
    let loader_handle = std::thread::Builder::new()
        .name("loader".into())
        .spawn({
            let cancelled = cancelled.clone();
            move || -> Result<()> {
                for ts in &timestamps {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }

                    let mut slot = free_rx
                        .recv()
                        .map_err(|_| anyhow!("Loader: free pool closed prematurely"))?;

                    let (bayer, frame_meta) = decoder.load_frame(*ts)?;

                    slot.bayer = bayer;
                    slot.as_shot_neutral = frame_meta.as_shot_neutral;

                    loaded_tx
                        .send(slot)
                        .map_err(|_| anyhow!("Loader: processor channel closed"))?;
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
    // Stage 2 — Processor thread: demosaic → color pipeline → OETF → u16
    // ==================================================================
    let processor_handle = std::thread::Builder::new()
        .name("processor".into())
        .spawn({
            let cancelled = cancelled.clone();
            move || -> Result<()> {
                let mut rgb = vec![0.0f32; pixel_count * 3];
                let demosaic = BilinearDemosaic::new(pattern);

                for mut slot in loaded_rx {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }

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

                    demosaic.process_par_into(
                        &slot.bayer, stride_width, offset_x, offset_y,
                        active_width, active_height, &pattern, &mut rgb,
                    )?;
                    normalize_linear_f32(&mut rgb, black_level as f32, white_level as f32);

                    let use_agx_frame = use_agx;
                    rgb.par_chunks_exact_mut(3).for_each(|chunk| {
                        let r = chunk[0] * r_gain;
                        let g = chunk[1];
                        let b = chunk[2] * b_gain;

                        if use_agx_frame {
                            // AgX will handle highlight roll-off and tone mapping;
                            // just apply CAT16 white-balance adaptation and move on.
                            let rr = r / r_gain;
                            let bb = b / b_gain;
                            let out = mat_mul_vec3(&fused, &[rr, g, bb]);
                            chunk[0] = out[0].max(0.0);
                            chunk[1] = out[1].max(0.0);
                            chunk[2] = out[2].max(0.0);
                        } else {
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
                        }
                    });

                    if let Some(ref agx) = agx_pipeline {
                        // Full AgX tone mapping pipeline: tone scale, gamut
                        // compression, and BT.1886 gamma 2.4 output.
                        agx.process_frame(&mut rgb);
                    } else if is_log {
                        export_tf.process(&mut rgb);
                    } else {
                        rgb.par_iter_mut().for_each(|v| {
                            if *v < 0.0 { *v = 0.0; }
                            *v = if *v < 0.018 { 4.5 * *v } else { 1.099 * v.powf(0.45) - 0.099 };
                        });
                    }

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
    let writer_handle = std::thread::Builder::new()
        .name("writer".into())
        .spawn(move || -> Result<()> {
            let mut frames_written: usize = 0;

            for slot in processed_rx {
                if writer_cancelled.load(Ordering::Relaxed) {
                    // Return slot to pool before exiting
                    let _ = free_tx_writer.send(slot);
                    // Skip finish() — encoder drops here, killing FFmpeg
                    return Ok(());
                }

                encoder.push_frame(&slot.frame_bytes)?;

                frames_written += 1;
                on_progress(frames_written as f64 / total_frames as f64 * 100.0);

                let _ = free_tx_writer.send(slot);
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
