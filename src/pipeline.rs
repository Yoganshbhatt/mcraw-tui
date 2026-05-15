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
    ProResProfile, Vp9Profile,
};
use crate::file::McrawFileInfo;
use anyhow::{anyhow, Result};
use rayon::prelude::*;
use std::fs;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;
use std::sync::atomic::{AtomicBool, Ordering};

/// Build FFmpeg codec arguments.
/// Delegates to `CodecFamily::to_ffmpeg_args` which independently resolves
/// the codec family, the user-chosen profile, and the runtime-detected
/// HEVC hardware encoder.
pub fn build_ffmpeg_codec_args(
    family: CodecFamily,
    hevc_encoder: &str,
    prores: ProResProfile,
    dnxhr: DnxhrProfile,
    hevc: HevcProfile,
    h264: H264Profile,
    av1: Av1Profile,
    vp9: Vp9Profile,
) -> (&'static str, &'static str, Vec<&'static str>) {
    family.to_ffmpeg_args(hevc_encoder, prores, dnxhr, hevc, h264, av1, vp9)
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
    // eprintln!("[NAKED] Starting raw bayer dump to: {}", output_path);

    let decoder = Decoder::new(&info.path)?;
    let timestamps = decoder.timestamps()?;

    if timestamps.is_empty() {
        return Err(anyhow!("No frames found in file"));
    }

    // eprintln!("[NAKED] Raw dimensions: {}x{} (stride={})", info.width, info.height, info.width);
    // eprintln!("[NAKED] Active area: {}x{} @ ({},{})",
    //     info.active_width, info.active_height, info.active_offset_x, info.active_offset_y);
    // eprintln!("[NAKED] Frame count: {}", timestamps.len());
    // eprintln!("[NAKED] Bayer pattern: {:?}", info.bayer_pattern);

    let mut out_file = fs::File::create(output_path)?;

    for (_i, ts) in timestamps.iter().enumerate() {
        // if (_i + 1) % 10 == 0 || _i == timestamps.len() - 1 {
        //     eprintln!("[NAKED] Dumping frame {}/{}", _i + 1, timestamps.len());
        // }

        let (bayer_bytes, _meta) = decoder.load_frame(*ts)?;

        let bayer_u16: Vec<u16> = bayer_bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();

        for &v in &bayer_u16 {
            out_file.write_all(&v.to_le_bytes())?;
        }
    }

    // eprintln!("[NAKED] Raw dump complete: {} frames -> {}", timestamps.len(), output_path);
    Ok(())
}

pub fn run(info: &McrawFileInfo, output_path: &str) -> Result<()> {
    let never_cancel = Arc::new(AtomicBool::new(false));
    run_export(info, output_path, &|_| {}, &never_cancel, &ColorSpace::Rec709, &TransferFunction::Rec709,
        CodecFamily::ProRes, ProResProfile::HQ, DnxhrProfile::HQX,
        HevcProfile::Main10_420, H264Profile::Main_8bit, Av1Profile::Profile0_420_10bit, Vp9Profile::Profile2_420_10bit, "libx265")
}

#[allow(clippy::too_many_arguments)]
pub fn run_export(
    info: &McrawFileInfo,
    output_path: &str,
    on_progress: &dyn Fn(f64),
    cancelled: &AtomicBool,
    export_cs: &ColorSpace,
    export_tf: &TransferFunction,
    codec_family: CodecFamily,
    prores_profile: ProResProfile,
    dnxhr_profile: DnxhrProfile,
    hevc_profile: HevcProfile,
    h264_profile: H264Profile,
    av1_profile: Av1Profile,
    vp9_profile: Vp9Profile,
    hevc_encoder: &str,
) -> Result<()> {
    // eprintln!("Starting video export to: {}", output_path);
    // eprintln!("Export settings: {} / {}", export_cs.name(), export_tf.name());

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

    // eprintln!("File info: {}x{} active area at ({},{}), stride {}",
    //     active_width, active_height, offset_x, offset_y, stride_width);
    // eprintln!("White level: {}, Black level: {}", info.white_level, info.black_level);
    // eprintln!("Bayer pattern: {:?}", info.bayer_pattern);

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

    // eprintln!("Camera→XYZ matrix: {:?}", cam_to_xyz);

    let is_log = export_tf.is_log_bypass();
    let target_xyz_to_rgb = export_cs.get_xyz_to_rgb_matrix();

    let pattern = info.bayer_pattern;
    let black_level = info.black_level;
    let white_level = info.white_level;
    let total_frames = timestamps.len();

    let (codec_name, pix_fmt, mut extra_args) = build_ffmpeg_codec_args(
        codec_family, hevc_encoder,
        prores_profile, dnxhr_profile, hevc_profile,
        h264_profile, av1_profile, vp9_profile,
    );

    let vui_tags = get_ffmpeg_vui_tags(export_cs, export_tf);
    extra_args.extend(vui_tags);

    let demosaic = BilinearDemosaic::new(pattern);
    let xyz_to_rec = xyz_to_rec709();

    let mut encoder = VideoEncoder::new(
        output_path, active_width, active_height, fps,
        codec_name, pix_fmt, &extra_args,
    )?;

    // Pre-allocate byte buffer once (6 bytes per pixel = 3 channels × u16)
    let pixel_count = (active_width * active_height) as usize;
    let bytes_per_frame = pixel_count * 6;
    let mut frame_bytes = vec![0u8; bytes_per_frame];

    // Lightweight profiling accumulators (nanoseconds)
    let mut p1_load: u64 = 0;
    let mut p2_demosaic: u64 = 0;
    let mut p3_process: u64 = 0;
    let mut p4_push: u64 = 0;

    for (i, ts) in timestamps.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            encoder.finish()?;
            log_profile(p1_load, p2_demosaic, p3_process, p4_push, i);
            return Err(anyhow!("Export cancelled by user"));
        }

        // --- Phase 1: load frame from disk & byte-swap ---
        let t0 = Instant::now();
        let (bayer_bytes, frame_meta) = decoder.load_frame(*ts)?;
        let bayer_u16: Vec<u16> = bayer_bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        let t1 = Instant::now();
        p1_load += (t1 - t0).as_nanos() as u64;

        // --- Phase 2: demosaic + normalize ---
        let mut linear_rgb = demosaic.process_par(&bayer_u16, stride_width, offset_x, offset_y, active_width, active_height, &pattern)?;
        normalize_linear_f32(&mut linear_rgb, black_level as f32, white_level as f32);
        let t2 = Instant::now();
        p2_demosaic += (t2 - t1).as_nanos() as u64;

        // --- Per-frame white balance & fused matrix ---
        let as_shot = frame_meta.as_shot_neutral;
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

        // Fuse camera→XYZ, CAT16 adaptation, and output RGB matrix into one 3×3
        let output_matrix = if is_log { &target_xyz_to_rgb } else { &xyz_to_rec };
        let fused = build_cat16_output_matrix(&cam_to_xyz, &scene_white_xyz, &D65_XYZ, output_matrix);

        // --- Phase 3: all pixel processing (fused pass + OETF + byte conversion) ---
        // Temporal WB → highlight clip → revert WB → fused matrix → clamp negatives
        linear_rgb.par_chunks_exact_mut(3).for_each(|chunk| {
            let r = chunk[0] * r_gain;
            let g = chunk[1];
            let b = chunk[2] * b_gain;

            // Highlight clip in WB-space
            let max_val = r.max(g).max(b);
            let (r, g, b) = if max_val > 0.95f32 {
                let t = ((max_val - 0.95) / 0.05).min(1.0);
                (r + (max_val - r) * t,
                 g + (max_val - g) * t,
                 b + (max_val - b) * t)
            } else {
                (r, g, b)
            };

            // Revert WB so the fused matrix sees native sensor ratios
            let rr = r / r_gain;
            let bb = b / b_gain;

            // Single 3×3 multiply: native RGB → adapted XYZ → output RGB
            let out = mat_mul_vec3(&fused, &[rr, g, bb]);
            chunk[0] = out[0].max(0.0);
            chunk[1] = out[1].max(0.0);
            chunk[2] = out[2].max(0.0);
        });

        // Apply OETF / log encoding
        if is_log {
            export_tf.process(&mut linear_rgb);
        } else {
            linear_rgb.par_iter_mut().for_each(|v| {
                if *v < 0.0 { *v = 0.0; }
                *v = if *v < 0.018 { 4.5 * *v } else { 1.099 * v.powf(0.45) - 0.099 };
            });
        }

        // Convert f32 → u16 bytes in parallel (no intermediate Vec<u16>)
        frame_bytes.par_chunks_exact_mut(6).enumerate().for_each(|(pi, out)| {
            let base = pi * 3;
            let ru = (linear_rgb[base].clamp(0.0, 1.0) * 65535.0) as u16;
            let gu = (linear_rgb[base + 1].clamp(0.0, 1.0) * 65535.0) as u16;
            let bu = (linear_rgb[base + 2].clamp(0.0, 1.0) * 65535.0) as u16;
            out[0] = ru as u8;
            out[1] = (ru >> 8) as u8;
            out[2] = gu as u8;
            out[3] = (gu >> 8) as u8;
            out[4] = bu as u8;
            out[5] = (bu >> 8) as u8;
        });
        let t3 = Instant::now();
        p3_process += (t3 - t2).as_nanos() as u64;

        // --- Phase 4: push to ffmpeg pipe ---
        encoder.push_frame(&frame_bytes)?;
        let t4 = Instant::now();
        p4_push += (t4 - t3).as_nanos() as u64;

        on_progress((i + 1) as f64 / total_frames as f64 * 100.0);
    }

    encoder.finish()?;
    log_profile(p1_load, p2_demosaic, p3_process, p4_push, timestamps.len());
    Ok(())
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
