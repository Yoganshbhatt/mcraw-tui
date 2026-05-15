use crate::color::{
    pipeline_convert_to_u16, normalize_linear, identity_ccm,
    mat_mul_vec3, camera_to_xyz_matrix, interpolate_matrix,
    cat16_adapt, D65_XYZ, xyz_to_rec709,
    BilinearDemosaic, Rec709TransferFunction,
    Demosaic, TransferFunctionProcessor, ColorSpace, TransferFunction,
};
use crate::decoder::Decoder;
use crate::encoder::VideoEncoder;
use crate::export::{CodecFamily, ProResProfile, DnxhrProfile, HevcProfile, H264Profile, Av1Profile, Vp9Profile};
use crate::file::McrawFileInfo;
use anyhow::{anyhow, Result};
use std::fs;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Build FFmpeg codec arguments from the selected codec family + profile.
/// Returns (codec_name, pixel_format, extra_args).
pub fn build_ffmpeg_codec_args(
    family: CodecFamily,
    prores: ProResProfile,
    dnxhr: DnxhrProfile,
    hevc: HevcProfile,
    h264: H264Profile,
    av1: Av1Profile,
    vp9: Vp9Profile,
) -> (&'static str, &'static str, Vec<&'static str>) {
    match family {
        CodecFamily::ProRes => {
            let (profile_v, pix_fmt) = match prores {
                ProResProfile::Proxy => ("0", "yuv422p10le"),
                ProResProfile::LT => ("1", "yuv422p10le"),
                ProResProfile::Standard => ("2", "yuv422p10le"),
                ProResProfile::HQ => ("3", "yuv422p10le"),
                ProResProfile::P4444 => ("4", "yuva444p10le"),
                ProResProfile::XQ4444 => ("5", "yuva444p12le"),
            };
            ("prores_ks", pix_fmt, vec!["-profile:v", profile_v])
        }
        CodecFamily::DNxHR => {
            let (profile_str, pix_fmt) = match dnxhr {
                DnxhrProfile::SQ => ("dnxhr_sq", "yuv422p10le"),
                DnxhrProfile::HD => ("dnxhr_hd", "yuv422p10le"),
                DnxhrProfile::HDX => ("dnxhr_hdx", "yuv422p10le"),
                DnxhrProfile::HQX => ("dnxhr_hqx", "yuv422p10le"),
                DnxhrProfile::P444 => ("dnxhr_444", "yuv444p10le"),
            };
            ("dnxhd", pix_fmt, vec!["-profile:v", profile_str])
        }
        CodecFamily::HEVC => {
            let (pix_fmt, extra_crf) = match hevc {
                HevcProfile::Main10_420 => ("yuv420p10le", "16"),
                HevcProfile::Main10_444 => ("yuv444p10le", "14"),
            };
            ("libx265", pix_fmt, vec!["-crf", extra_crf, "-pix_fmt", pix_fmt])
        }
        CodecFamily::H264 => {
            match h264 {
                H264Profile::Main_8bit => {
                    ("libx264", "yuv420p", vec!["-preset", "slow", "-crf", "18"])
                }
                H264Profile::High_10bit => {
                    ("libx264", "yuv422p10le", vec!["-preset", "slow", "-crf", "18"])
                }
            }
        }
        CodecFamily::AV1 => {
            let crf = match av1 {
                Av1Profile::Profile0_420_10bit => "30",
                Av1Profile::Profile1_444_10bit => "30",
            };
            ("libaom-av1", "yuv420p10le", vec!["-crf", crf, "-cpu-used", "4"])
        }
        CodecFamily::VP9 => {
            let crf = match vp9 {
                Vp9Profile::Profile2_420_10bit => "30",
                Vp9Profile::Profile3_444_10bit => "30",
            };
            ("libvpx-vp9", "yuv420p10le", vec!["-crf", crf, "-b:v", "0"])
        }
        CodecFamily::CinemaDNG => {
            // cDNG is handled via TIFF/DNG writer, not FFmpeg
            ("", "", vec![])
        }
    }
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
    eprintln!("[NAKED] Starting raw bayer dump to: {}", output_path);

    let decoder = Decoder::new(&info.path)?;
    let timestamps = decoder.timestamps()?;

    if timestamps.is_empty() {
        return Err(anyhow!("No frames found in file"));
    }

    let w = info.width as u32;
    let h = info.height as u32;

    eprintln!("[NAKED] Raw dimensions: {}x{} (stride={})", w, h, w);
    eprintln!("[NAKED] Active area: {}x{} @ ({},{})",
        info.active_width, info.active_height, info.active_offset_x, info.active_offset_y);
    eprintln!("[NAKED] Frame count: {}", timestamps.len());
    eprintln!("[NAKED] Bayer pattern: {:?}", info.bayer_pattern);

    let mut out_file = fs::File::create(output_path)?;

    for (i, ts) in timestamps.iter().enumerate() {
        if (i + 1) % 10 == 0 || i == timestamps.len() - 1 {
            eprintln!("[NAKED] Dumping frame {}/{}", i + 1, timestamps.len());
        }

        let (bayer_bytes, _meta) = decoder.load_frame(*ts)?;

        let bayer_u16: Vec<u16> = bayer_bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();

        for &v in &bayer_u16 {
            out_file.write_all(&v.to_le_bytes())?;
        }
    }

    eprintln!("[NAKED] Raw dump complete: {} frames -> {}", timestamps.len(), output_path);
    Ok(())
}

pub fn run(info: &McrawFileInfo, output_path: &str) -> Result<()> {
    let never_cancel = Arc::new(AtomicBool::new(false));
    run_export(info, output_path, &|_| {}, &never_cancel, &ColorSpace::Rec709, &TransferFunction::Rec709,
        CodecFamily::ProRes, ProResProfile::HQ, DnxhrProfile::HQX,
        HevcProfile::Main10_420, H264Profile::Main_8bit, Av1Profile::Profile0_420_10bit, Vp9Profile::Profile2_420_10bit)
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
) -> Result<()> {
    eprintln!("Starting video export to: {}", output_path);
    eprintln!("Export settings: {} / {}", export_cs.name(), export_tf.name());

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

    eprintln!("File info: {}x{} active area at ({},{}), stride {}",
        active_width, active_height, offset_x, offset_y, stride_width);
    eprintln!("White level: {}, Black level: {}", info.white_level, info.black_level);
    eprintln!("Bayer pattern: {:?}", info.bayer_pattern);

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

    eprintln!("Camera→XYZ matrix: {:?}", cam_to_xyz);

    let is_log = export_tf.is_log_bypass();
    let target_xyz_to_rgb = export_cs.get_xyz_to_rgb_matrix();

    let pattern = info.bayer_pattern;
    let black_level = info.black_level;
    let white_level = info.white_level;
    let total_frames = timestamps.len();

    let (codec_name, pix_fmt, mut extra_args) = build_ffmpeg_codec_args(
        codec_family, prores_profile, dnxhr_profile, hevc_profile,
        h264_profile, av1_profile, vp9_profile,
    );

    let vui_tags = get_ffmpeg_vui_tags(export_cs, export_tf);
    extra_args.extend(vui_tags);

    let demosaic = BilinearDemosaic::new(pattern);
    let rec709_oetf = Rec709TransferFunction::new();

    let mut encoder = VideoEncoder::new(
        output_path, active_width, active_height, fps,
        codec_name, pix_fmt, &extra_args,
    )?;

    for (i, ts) in timestamps.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            eprintln!("Export cancelled at frame {}/{}", i + 1, total_frames);
            encoder.finish()?;
            return Err(anyhow!("Export cancelled by user"));
        }

        if i % 10 == 0 || i == total_frames - 1 {
            eprintln!("Processing frame {}/{}", i + 1, total_frames);
        }

        let (bayer_bytes, frame_meta) = decoder.load_frame(*ts)?;
        let bayer_u16: Vec<u16> = bayer_bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();

        let mut linear_rgb = demosaic.process(&bayer_u16, stride_width, offset_x, offset_y, active_width, active_height, &pattern)?;
        normalize_linear(&mut linear_rgb, black_level, white_level);

        // --- Per-frame chromatic adaptation with CAT16: camera raw → D65 XYZ ---
        let as_shot = frame_meta.as_shot_neutral;
        let scene_white_xyz: [f32; 3] = if as_shot[0] > 1e-6 && as_shot[1] > 1e-6 && as_shot[2] > 1e-6 {
            mat_mul_vec3(&cam_to_xyz, &as_shot)
        } else {
            [D65_XYZ[0], D65_XYZ[1], D65_XYZ[2]]
        };

        // Convert camera raw → XYZ D65
        let mut frame_xyz: Vec<f32> = Vec::with_capacity(linear_rgb.len());
        for chunk in linear_rgb.chunks_exact(3) {
            let xyz = mat_mul_vec3(&cam_to_xyz, &[chunk[0], chunk[1], chunk[2]]);
            let adapted = cat16_adapt(&xyz, &scene_white_xyz, &D65_XYZ);
            frame_xyz.push(adapted[0]);
            frame_xyz.push(adapted[1]);
            frame_xyz.push(adapted[2]);
        }

        if is_log {
            // --- LOG BYPASS: Scene-referred log export, no tonemapping ---
            for chunk in frame_xyz.chunks_exact_mut(3) {
                let rgb = mat_mul_vec3(&target_xyz_to_rgb, &[chunk[0], chunk[1], chunk[2]]);
                chunk[0] = rgb[0].max(0.0);
                chunk[1] = rgb[1].max(0.0);
                chunk[2] = rgb[2].max(0.0);
            }
            export_tf.process(&mut frame_xyz);
        } else {
            // --- STANDARD PATH: XYZ → Rec709 + OETF (no AgX for now) ---
            for chunk in frame_xyz.chunks_exact_mut(3) {
                let rgb = mat_mul_vec3(&xyz_to_rec709(), &[chunk[0], chunk[1], chunk[2]]);
                chunk[0] = rgb[0].max(0.0).min(1.0);
                chunk[1] = rgb[1].max(0.0).min(1.0);
                chunk[2] = rgb[2].max(0.0).min(1.0);
            }
            // TODO: When TransferFunction::Rec709 is selected, optionally apply AgX
            // tonemapping here before the OETF.
            rec709_oetf.process(&mut frame_xyz);
        }

        let u16_data = pipeline_convert_to_u16(&frame_xyz);
        let bytes: Vec<u8> = u16_data.iter().flat_map(|&v| v.to_le_bytes()).collect();
        encoder.push_frame(&bytes)?;

        on_progress((i + 1) as f64 / total_frames as f64 * 100.0);
    }

    encoder.finish()?;
    eprintln!("Video export complete: {}", output_path);
    Ok(())
}
