use crate::color::{
    normalize_linear_per_channel, identity_ccm, mat_mul_vec3, mat_mul_3x3,
    camera_to_xyz_matrix, interpolate_matrix,
    BilinearDemosaic, ColorSpace, TransferFunction, build_bradford_matrix, D65_XYZ,
    detect_camera_to_xyz,
    compute_color_only_map,
    apply_lens_correction_cpu_with_map,
    apply_display_rolloff,
    apply_basic_highlight_desat,
    apply_on_highlight_imbalance_hedge,
    apply_exposure_gain,
};
use crate::decoder::Decoder;
use crate::encoder::VideoEncoder;
use crate::export::{Av1Profile, CodecFamily, DnxhrProfile, H264Profile, HevcProfile, ProResProfile, RateControl, Vp9Profile, yuv_conversion_for};
use crate::file::McrawFileInfo;
use crate::gpu;
use crate::hl;
use crate::stats::{PhaseGuard, PipelineStats};
use anyhow::{anyhow, Result};
use crossbeam_channel::bounded;
use rayon::prelude::*;
use std::fs;
use std::io::{BufWriter, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const PIPELINE_DEPTH: usize = 3;

/// Black level / white level mode for normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlWlMode {
    Dynamic,
    Static,
    Preset1023_64,
    Preset4095_256,
    Preset16383_1024,
    Preset65535_4096,
    Preset4095_64,
    Preset16383_64,
    Preset16383_0,
}

impl BlWlMode {
    pub fn name(&self) -> &'static str {
        match self {
            BlWlMode::Dynamic => "Dynamic",
            BlWlMode::Static => "Static",
            BlWlMode::Preset1023_64 => "1023/64",
            BlWlMode::Preset4095_256 => "4095/256",
            BlWlMode::Preset16383_1024 => "16383/1024",
            BlWlMode::Preset65535_4096 => "65535/4096",
            BlWlMode::Preset4095_64 => "4095/64",
            BlWlMode::Preset16383_64 => "16383/64",
            BlWlMode::Preset16383_0 => "16383/0",
        }
    }
}

/// Lens correction mode for shading map correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensCorrectionMode {
    Off,
    Full,
    ColorOnly,
}

impl LensCorrectionMode {
    pub fn name(&self) -> &'static str {
        match self {
            LensCorrectionMode::Off => "Off",
            LensCorrectionMode::Full => "Full",
            LensCorrectionMode::ColorOnly => "Color Only",
        }
    }
}

struct FrameSlot {
    bayer: Vec<u16>,
    frame_bytes: Vec<u8>,
    as_shot_neutral: [f32; 3],
    dynamic_black_level: Option<[f32; 4]>,
    dynamic_white_level: Option<f32>,
}

pub fn build_ffmpeg_codec_args(family: CodecFamily, hevc_encoder: &str, h264_encoder: &str, av1_encoder: &str, prores_encoder: &str, prores: ProResProfile, dnxhr: DnxhrProfile, hevc: HevcProfile, h264: H264Profile, av1: Av1Profile, vp9: Vp9Profile, rate_control: &RateControl, cs: ColorSpace) -> (String, String, Vec<String>) {
    family.to_ffmpeg_args(hevc_encoder, h264_encoder, av1_encoder, prores_encoder, prores, dnxhr, hevc, h264, av1, vp9, rate_control, cs)
}

/// Build the bitstream colour signalling for one export.
/// True for planar-RGB pixel formats, where an RGB→YCbCr conversion does
/// not exist and a matrix tag must therefore never be asserted.
pub fn is_planar_rgb_fmt(pix_fmt: &str) -> bool {
    pix_fmt.starts_with("gbrp") || pix_fmt.starts_with("rgb")
}

/// Build the bitstream colour signalling for one export.
///
/// Two things this must get right, both measured on this toolchain:
///
/// 1. **Mechanism.** FFmpeg's `-color_primaries` / `-color_trc` /
///    `-colorspace` / `-color_range` output options are silently dropped by
///    `libx265` — verified for BT.709 and BT.2020, for RGB and YUV pixel
///    formats alike (every deliverable read back `unknown`). Only
///    `-x265-params colorprim=…:transfer=…:colormatrix=…:range=…` actually
///    reaches the HEVC VUI. Other encoders in the family are driven through
///    the generic options, which they do honour, so the argument form is
///    selected per encoder rather than per codec family.
///
/// 2. **Pairing.** The matrix and range written here MUST equal the values
///    `export::yuv_conversion_for` put in the scale filter. Asserting a
///    YCbCr matrix for RGB planes (or a wrong one for YUV planes) is exactly
///    the green/magenta decode failure: flipping only the matrix tag on a
///    bit-identical stream reproduced it (full-frame green-gap −46, sky
///    +40). RGB output claims no matrix at all.
///
/// Primaries/transfer stay `unspecified` for camera-vendor gamuts and log
/// curves: ITU-R/SMPTE define no code point for them, and writing a
/// *different* standard's value would be a mislabel that a colour-managed
/// NLE would act on.
pub fn get_ffmpeg_vui_tags(
    color_space: &ColorSpace,
    transfer: &TransferFunction,
    pix_fmt: &str,
    encoder: &str,
) -> Vec<String> {
    let primaries = match color_space {
        ColorSpace::Rec709 | ColorSpace::Srgb => "bt709",
        ColorSpace::Rec2020 => "bt2020",
        ColorSpace::DciP3 => "smpte431",
        ColorSpace::DisplayP3 => "smpte432",
        // Camera-vendor gamuts (F-Gamut, S-Gamut3, AWG3/4, Canon Cinema
        // Gamut, V-Gamut, DWG, AP1, Apple Wide Gamut): no standard code
        // point describes them, and no standard gamut is a superset
        // (AWG3's red primary is outside BT.2020). `unspecified` is the
        // truthful answer and also the safest: an NLE is then obliged to
        // use the operator's input assignment instead of guessing.
        _ => "unspecified",
    };
    let trc = match transfer {
        TransferFunction::Rec709 => "bt709",
        // Display gamma 2.4 has no dedicated FFmpeg code; bt709 is the
        // standard display-referred tag and is the safest choice.
        TransferFunction::Gamma24 => "bt709",
        TransferFunction::HLG => "arib-std-b67",
        TransferFunction::PQ => "smpte2084",
        TransferFunction::Linear => "linear",
        // Camera log curves (S-Log3, V-Log, ARRI LogC3, C-Log3, F-Log2,
        // Apple Log, ACEScct, DaVinci Intermediate) have no standard
        // FFmpeg/ITU TRC code. `unknown` tells decoders not to attempt
        // any inverse-OETF — the filename / NLE input assignment
        // identifies the curve.
        _ => "unknown",
    };

    // Planar RGB: identity conversion. Claiming a YCbCr matrix here is the
    // bug, so no matrix or range is asserted at all.
    if is_planar_rgb_fmt(pix_fmt) {
        return match encoder {
            "libx265" => vec!["-x265-params".into(), format!("colorprim={primaries}:transfer={trc}")],
            _ => vec!["-color_primaries".into(), primaries.into(), "-color_trc".into(), trc.into()],
        };
    }

    let conv = yuv_conversion_for(*color_space);
    let range_kw = if conv.range == "full" { "full" } else { "limited" };
    match encoder {
        // x265's own spelling for the non-constant-luminance BT.2020
        // matrix is `bt2020nc` (verified in the VUI); `bt2020ncl` is
        // rejected and silently drops the tag.
        "libx265" => vec![
            "-x265-params".into(),
            format!("colorprim={primaries}:transfer={trc}:colormatrix={}:range={range_kw}", conv.matrix),
        ],
        _ => vec![
            "-color_primaries".into(), primaries.into(),
            "-color_trc".into(), trc.into(),
            "-colorspace".into(), conv.matrix.into(),
            "-color_range".into(), range_kw.into(),
        ],
    }
}

pub fn run_naked(info: &McrawFileInfo, output_path: &str) -> Result<()> {
    tracing::info!("run_naked: input={} output={}", info.path, output_path);
    let _stats = Arc::new(PipelineStats::new());
    let decoder = Decoder::new(&info.path)?; let timestamps = decoder.timestamps()?;
    if timestamps.is_empty() { return Err(anyhow!("No frames found in file")); }
    let mut out_file = fs::File::create(output_path)?;
    for ts in &timestamps { let (bayer, _meta) = decoder.load_frame(*ts)?; for &v in &bayer { out_file.write_all(&v.to_le_bytes())?; } }
    Ok(())
}

pub fn run(info: &McrawFileInfo, output_path: &str) -> Result<()> {
    let never_cancel = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(PipelineStats::new());
    run_export(info.clone(), output_path.to_string(), Arc::new(|_| {}), never_cancel, stats, ColorSpace::Rec709, TransferFunction::Rec709, CodecFamily::ProRes, ProResProfile::HQ, DnxhrProfile::HQX, HevcProfile::Main10_420, H264Profile::Main8bit, Av1Profile::Profile0_420_10bit, Vp9Profile::Profile2_420_10bit, "libx265".to_string(), "libx264".to_string(), "libaom-av1".to_string(), "prores_ks".to_string(), RateControl::Lossless, None, LensCorrectionMode::Full, BlWlMode::Dynamic, true, 0.0)
}

#[allow(clippy::too_many_arguments)]
pub fn run_export(info: McrawFileInfo, output_path: String, on_progress: Arc<dyn Fn(f64) + Send + Sync>, cancelled: Arc<AtomicBool>, stats: Arc<PipelineStats>, export_cs: ColorSpace, export_tf: TransferFunction, codec_family: CodecFamily, prores_profile: ProResProfile, dnxhr_profile: DnxhrProfile, hevc_profile: HevcProfile, h264_profile: H264Profile, av1_profile: Av1Profile, vp9_profile: Vp9Profile, hevc_encoder: String, h264_encoder: String, av1_encoder: String, prores_encoder: String, rate_control: RateControl, custom_fps: Option<f64>, lens_mode: LensCorrectionMode, blwl_mode: BlWlMode, highlight_recovery: bool, exposure_ev: f32) -> Result<()> {
    tracing::info!("run_export: input={} output={} codec={} cs={} tf={}", info.path, output_path, codec_family.name(), export_cs.name(), export_tf.name());
    let setup_start = Instant::now();
    let decoder = Decoder::new(&info.path)?; let timestamps = decoder.timestamps()?;
    if timestamps.is_empty() { return Err(anyhow!("No frames found in file")); }
    let stride_width = info.width as u32; let offset_x = info.active_offset_x as u32; let offset_y = info.active_offset_y as u32;
    let active_width = if info.active_width > 0 { info.active_width as u32 } else { stride_width };
    let active_height = if info.active_height > 0 { info.active_height as u32 } else { info.height as u32 };
    if active_width == 0 || active_height == 0 { return Err(anyhow!("Invalid active dimensions")); }
    let fps = custom_fps.unwrap_or_else(|| if info.fps > 0.0 { info.fps } else { 25.0 });
    
    let cm1_f32: [f32; 9] = info.camera_metadata.color_matrix.map(|cm| { let mut ccm = [0.0f32; 9]; for (i, v) in cm.iter().enumerate() { ccm[i] = *v as f32; } ccm }).unwrap_or_else(identity_ccm);
    let cm2_f32: Option<[f32; 9]> = info.camera_metadata.color_matrix2.map(|cm| { let mut ccm = [0.0f32; 9]; for (i, v) in cm.iter().enumerate() { ccm[i] = *v as f32; } ccm });
    let fm1_f32: Option<[f32; 9]> = info.camera_metadata.forward_matrix1.map(|fm| { let mut ccm = [0.0f32; 9]; for (i, v) in fm.iter().enumerate() { ccm[i] = *v as f32; } ccm });
    let fm2_f32: Option<[f32; 9]> = info.camera_metadata.forward_matrix2.map(|fm| { let mut ccm = [0.0f32; 9]; for (i, v) in fm.iter().enumerate() { ccm[i] = *v as f32; } ccm });
    let cal1_f32: Option<[f32; 9]> = info.camera_metadata.calibration_matrix1.map(|cm| { let mut ccm = [0.0f32; 9]; for (i, v) in cm.iter().enumerate() { ccm[i] = *v as f32; } ccm });
    let cal2_f32: Option<[f32; 9]> = info.camera_metadata.calibration_matrix2.map(|cm| { let mut ccm = [0.0f32; 9]; for (i, v) in cm.iter().enumerate() { ccm[i] = *v as f32; } ccm });
    
    let cm_has_values = cm1_f32.iter().any(|&v| v.abs() > 0.01 && v.abs() < 10.0);
    let mut matrix_path = "";
    // The matrix that takes white-balanced camera RGB into XYZ under the
    // matrix's reference illuminant. Two DNG conventions:
    //   * ColorMatrix1: Camera-WB -> XYZ under illuminant1 (often D65 or
    //     Standard A, sometimes custom). Row sums equal that illuminant.
    //     Needs a Bradford CAT to D65 before applying xyz_to_Rec.709.
    //   * ForwardMatrix1: Camera-WB -> XYZ under D50 (assumes camera
    //     WB is applied via AsShotNeutral). Row sums equal D50
    //     [0.96422, 1.0, 0.82521]. Needs only a fixed D50->D65 CAT for
    //     a Rec.709 (D65) output.
    //
    // The DNG spec says to prefer ForwardMatrix1 over ColorMatrix1 when
    // both are present because fm1 already incorporates scene WB and
    // D50 adaptation. We therefore select fm1 if present, falling back
    // to cm1 (with a per-frame Bradford CAT computed from the actual
    // scene white in XYZ) only if no fm1 is available.
    let cam_to_xyz: [f32; 9] = if let (Some(ref fm1), Some(ref fm2)) = (fm1_f32, fm2_f32) {
        matrix_path = "ForwardMatrix1+2 → D50 (preferred)";
        let fm_avg = interpolate_matrix(fm1, fm2, 0.5);
        // fm1 is already Camera-WB -> D50; no orientation test needed.
        // Verify it's the right orientation: row sums should match D50.
        let rs = [fm_avg[0] + fm_avg[1] + fm_avg[2],
                  fm_avg[3] + fm_avg[4] + fm_avg[5],
                  fm_avg[6] + fm_avg[7] + fm_avg[8]];
        let d50 = crate::color::D50_XYZ;
        let d = (rs[0]-d50[0]).powi(2) + (rs[1]-d50[1]).powi(2) + (rs[2]-d50[2]).powi(2);
        if d < 0.05 {
            fm_avg
        } else {
            matrix_path = "ForwardMatrix1+2 (orientation-adjusted)";
            detect_camera_to_xyz(&fm_avg)
        }
    } else if let Some(ref fm1) = fm1_f32 {
        matrix_path = "ForwardMatrix1 → D50 (preferred)";
        let rs = [fm1[0] + fm1[1] + fm1[2],
                  fm1[3] + fm1[4] + fm1[5],
                  fm1[6] + fm1[7] + fm1[8]];
        let d50 = crate::color::D50_XYZ;
        let d = (rs[0]-d50[0]).powi(2) + (rs[1]-d50[1]).powi(2) + (rs[2]-d50[2]).powi(2);
        if d < 0.05 {
            *fm1
        } else {
            matrix_path = "ForwardMatrix1 (orientation-adjusted)";
            detect_camera_to_xyz(fm1)
        }
    } else if cm_has_values {
        matrix_path = "ColorMatrix1 → XYZ (no fm1 fallback)";
        let cal = cal1_f32.or(cal2_f32);
        match cm2_f32 {
            Some(ref cm2) => {
                let cm_avg = interpolate_matrix(&cm1_f32, cm2, 0.5);
                camera_to_xyz_matrix(&cm_avg, cal.as_ref())
            }
            None => camera_to_xyz_matrix(&cm1_f32, cal.as_ref()),
        }
    } else {
        matrix_path = "IDENTITY";
        identity_ccm()
    };
    tracing::info!("matrix path: {} | cam_to_xyz diag=[{:.3},{:.3},{:.3}]", matrix_path, cam_to_xyz[0], cam_to_xyz[4], cam_to_xyz[8]);
    // Suppress unused-import warning when nothing forwards to a transform that
    // we might still want to log. Kept for parity with detector heuristics.
    let _ = detect_camera_to_xyz(&cam_to_xyz);

    let xyz_to_output = export_cs.get_xyz_to_rgb_matrix();
    // The cam_to_xyz matrix maps Camera-WB -> XYZ under some reference
    // illuminant. For the GPU fused matrix we pre-bake a constant Bradford
    // CAT from that illuminant to D65 (so the output is D65-based, e.g.
    // Rec.709). The per-frame CPU path additionally re-applies the CAT
    // using the actual scene white in XYZ for the case where the
    // illuminant is unknown (cm1 path) — see `fused` construction below.
    let rs = [cam_to_xyz[0] + cam_to_xyz[1] + cam_to_xyz[2],
              cam_to_xyz[3] + cam_to_xyz[4] + cam_to_xyz[5],
              cam_to_xyz[6] + cam_to_xyz[7] + cam_to_xyz[8]];
    let cam_illuminant_xyz = if matrix_path.starts_with("ForwardMatrix") {
        // fm1 maps to D50; row sums ARE D50 (within 0.05).
        crate::color::D50_XYZ
    } else {
        // cm1 (or identity) — use the row sums as the implied illuminant.
        // If row sums look degenerate (e.g. all near zero or far from any
        // known illuminant), fall back to D50.
        let l = rs[0].max(rs[1]).max(rs[2]);
        if l < 0.1 || l > 5.0 { crate::color::D50_XYZ } else { rs }
    };
    let bradford_static = build_bradford_matrix(&cam_illuminant_xyz, &D65_XYZ);
    let cam_to_xyz_d65 = mat_mul_3x3(&bradford_static, &cam_to_xyz);
    let fused = mat_mul_3x3(&xyz_to_output, &cam_to_xyz_d65);
    tracing::info!("fused matrix cs={}: [{:.6},{:.6},{:.6} | {:.6},{:.6},{:.6} | {:.6},{:.6},{:.6}]",
        export_cs.name(),
        fused[0], fused[1], fused[2],
        fused[3], fused[4], fused[5],
        fused[6], fused[7], fused[8],
    );
    let pattern = info.bayer_pattern; let white_level = info.white_level; let total_frames = timestamps.len();
    // The censored-photosite completion expresses "neutral" as the file's own
    // as-shot neutral triple (src/hl.rs). That is exact on the ForwardMatrix
    // path — fm1 maps the as-shot-WB'd neutral to D50, the CAT ends at the
    // output white, and the output matrix is D65 — and only approximate on the
    // ColorMatrix1 path, whose per-frame Bradford CAT is built from the scene
    // white. Verify once per export and say so loudly if it does not hold.
    if highlight_recovery {
        let n = mat_mul_vec3(&crate::color::invert_3x3(&fused), &[1.0, 1.0, 1.0]);
        let g = if n[1].is_finite() && n[1].abs() > 1e-6 { n[1] } else { 1.0 };
        let dev = (n[0] / g - 1.0).abs().max((n[1] / g - 1.0).abs()).max((n[2] / g - 1.0).abs());
        if dev > 1e-2 {
            tracing::warn!("highlight recovery: fused matrix is not neutral-at-unity (deviation {:.4}, path {}); the completion will inherit the ColorMatrix1 approximation", dev, matrix_path);
        } else {
            tracing::info!("highlight recovery: neutral identity verified (deviation {deviation:.2e}, path {path})", deviation = dev, path = matrix_path);
        }
    }
    let bl_count = info.black_level_count;
    let bl_per_ch = info.black_level_per_channel;
    let bl_static_r = bl_per_ch[0];
    let bl_static_g = if bl_count >= 4 { (bl_per_ch[1] + bl_per_ch[2]) / 2.0 } else { bl_per_ch[0] };
    let bl_static_b = if bl_count >= 4 { bl_per_ch[3] } else { bl_per_ch[0] };

    // Lens correction data (captured by processor closure).
    // MOTION format files may not populate sensor_width/sensor_height
    // (they come from legacy TLV blocks only), so fall back to the
    // active frame dimensions when those values are zero.
    let sensor_w = if info.sensor_width > 0 { info.sensor_width as u32 } else { info.width as u32 + info.active_offset_x as u32 };
    let sensor_h = if info.sensor_height > 0 { info.sensor_height as u32 } else { info.height as u32 + info.active_offset_y as u32 };
    // Try the info's shading map first, fall back to decoder's container metadata
    let shading_map = info.lens_shading_map.clone().or_else(|| {
        decoder.container_metadata().ok().and_then(|cm| cm.lens_shading_map)
    });
    if let Some(ref sm) = shading_map {
        if lens_mode != LensCorrectionMode::Off {
            tracing::info!("lens shading map found: {}x{}", sm.width, sm.height);
        }
    } else if lens_mode != LensCorrectionMode::Off {
        tracing::warn!("lens correction enabled but no shading map found in file or decoder");
    }
    let (codec_name, pix_fmt, mut extra_args) = build_ffmpeg_codec_args(codec_family, &hevc_encoder, &h264_encoder, &av1_encoder, &prores_encoder, prores_profile, dnxhr_profile, hevc_profile, h264_profile, av1_profile, vp9_profile, &rate_control, export_cs);
    let vui_tags = get_ffmpeg_vui_tags(&export_cs, &export_tf, &pix_fmt, &codec_name);
    tracing::info!("export signalling: codec={} pix_fmt={} vui={:?}", codec_name, pix_fmt, vui_tags);
    extra_args.extend(vui_tags);
    
    let audio_temp_path = if info.has_audio && info.audio_sample_rate > 0 && info.audio_channels > 0 {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let temp_path = std::env::temp_dir().join(format!("mcraw_audio_{}.raw", ts));
        match std::fs::File::create(&temp_path) {
            Ok(file) => { let mut writer = BufWriter::new(file); match decoder.write_audio_to(&mut writer) { Ok(()) => { let _ = writer.flush(); Some(temp_path) } Err(e) => { let _ = std::fs::remove_file(&temp_path); None } } }
            Err(_) => None
        }
    } else { None };
    
    let mut encoder = VideoEncoder::new(&output_path, active_width, active_height, fps, &codec_name, &pix_fmt, &extra_args, audio_temp_path.as_deref(), info.audio_sample_rate, info.audio_channels)?;
    let stride_pixels = stride_width as usize * info.height as usize; let pixel_count = (active_width * active_height) as usize; let bytes_per_frame = pixel_count * 6;
    let (free_tx, free_rx) = bounded::<FrameSlot>(PIPELINE_DEPTH); let (loaded_tx, loaded_rx) = bounded::<FrameSlot>(PIPELINE_DEPTH); let (processed_tx, processed_rx) = bounded::<FrameSlot>(PIPELINE_DEPTH);
    for _ in 0..PIPELINE_DEPTH { free_tx.send(FrameSlot { bayer: vec![0u16; stride_pixels], frame_bytes: vec![0u8; bytes_per_frame], as_shot_neutral: [0.0; 3], dynamic_black_level: None, dynamic_white_level: None })?; }
    let free_tx_writer = free_tx.clone();
    
    let filters = pattern.to_dcraw_filters(); let mut rcd_pipeline: Option<gpu::RcdPipeline> = None;
    // MCRAW_FORCE_CPU=1 forces the CPU path (useful for A/B against the GPU).
    if std::env::var("MCRAW_FORCE_CPU").as_deref() == Ok("1") {
        rcd_pipeline = None;
    } else if let Ok(ctx) = pollster::block_on(gpu::GpuContext::new()) {
        let ctx = std::sync::Arc::new(ctx);
        if let Ok(pipeline) = gpu::RcdPipeline::new(ctx, active_width, active_height) {
            rcd_pipeline = Some(pipeline);
        }
    }
    // CFA phase assumption: `hl::color_at` maps active-region coords while
    // the demosaic and the RCD shaders map sensor coords + offset parity.
    // All reviewed files report offset (0,0), where the three agree. An odd
    // offset would complete the wrong colour's photosites — loud, not silent.
    if offset_x & 1 == 1 || offset_y & 1 == 1 {
        tracing::warn!("odd active offset ({offset_x},{offset_y}): HL completion phase differs from demosaic phase");
    }
    stats.setup.record(setup_start.elapsed());
    
    let loader_handle = std::thread::Builder::new().name("loader".into()).spawn({
        let cancelled = cancelled.clone();
        let stats = Arc::clone(&stats);
        move || -> Result<()> {
            for (i, ts) in timestamps.iter().enumerate() {
                if cancelled.load(Ordering::Relaxed) { break; }
                let mut slot = free_rx.recv().map_err(|_| anyhow!("Loader: free pool closed"))?;
                let as_shot_neutral = {
                    let _g = PhaseGuard::new(&stats.decode);
                    decoder.load_frame_into(*ts, &mut slot.bayer)?
                };
                slot.as_shot_neutral = as_shot_neutral;
                if let Ok(meta) = decoder.load_frame_metadata(*ts) {
                    slot.dynamic_black_level = meta.dynamic_black_level;
                    slot.dynamic_white_level = meta.dynamic_white_level;
                }
                // B4: hint the OS to prefetch the next frame's range.
                if let Some(next_ts) = timestamps.get(i + 1) {
                    decoder.prefetch(*next_ts);
                }
                loaded_tx.send(slot).map_err(|_| anyhow!("Loader: processor channel closed"))?;
            }
            drop(loaded_tx); Ok(())
        }
    })?;
    
    // AgX pipeline is intentionally disabled. It will be reintroduced
    // as a separate feature; for now the render path is scene-referred
    // raw → WB → CCM → OETF.

    let processor_handle = std::thread::Builder::new().name("processor".into()).spawn({
        let cancelled = cancelled.clone();
        let stats = Arc::clone(&stats);
        move || -> Result<()> {
            let mut rgb = vec![0.0f32; pixel_count * 3]; let demosaic = BilinearDemosaic::new(pattern);
            let mut hl_scratch = hl::HlScratch::new();
            let hl_geom = hl::HlGeometry::from_info(stride_width, offset_x, offset_y, active_width, active_height, pattern);

            // Lens correction pre-setup (computed once, applied per-frame)
            let (color_only_map, _lens_grid_w, _lens_grid_h) = match &shading_map {
                Some(sm) if lens_mode == LensCorrectionMode::ColorOnly => {
                    let cm = compute_color_only_map(&sm.channels, sm.width, sm.height);
                    (Some(cm), sm.width, sm.height)
                }
                Some(sm) => (None, sm.width, sm.height),
                None => (None, 0, 0),
            };
            let has_lens = !matches!(lens_mode, LensCorrectionMode::Off) && shading_map.is_some();

            let mut frame_index: u64 = 0;
            for mut slot in loaded_rx {
                if cancelled.load(Ordering::Relaxed) { break; }
                frame_index += 1;
                stats.frames_total.fetch_add(1, Ordering::Relaxed);
                slot.frame_bytes.fill(0); let as_shot = slot.as_shot_neutral;

                // Per-frame fused matrix. Three cases:
                //
                //   1. cam_to_xyz = fm1: fm1 already maps Camera-WB -> D50
                //      XYZ. The only CAT needed is a fixed D50 -> D65 (which
                //      is baked into the static `fused` above). Re-using
                //      the static `fused` here is the correct and only
                //      consistent choice; the per-frame as_shot does NOT
                //      re-enter the matrix.
                //
                //   2. cam_to_xyz = cm1: cm1 maps Camera-WB -> XYZ under
                //      some other illuminant (e.g. D65, A, custom). The
                //      per-frame as_shot gives the actual scene white in
                //      XYZ (under the matrix's reference illuminant) and
                //      the Bradford CAT is computed from that.
                //
                //   3. cam_to_xyz = identity: no characterization, just
                //      use the static fused (which is xyz_to_Rec709
                //      applied to identity — produces non-neutral gray
                //      for D65 scenes; this is a known limitation when
                //      no matrix is provided).
                let fused = if matrix_path.starts_with("ForwardMatrix") {
                    // Use the precomputed static fused (D50 -> D65 -> Rec.709
                    // CAT already baked in).
                    fused
                } else if matrix_path.starts_with("ColorMatrix") {
                    // Per-frame CAT from cm1-as-shot-white to D65.
                    let neutral_under_d65 =
                        (as_shot[0] - 1.0).abs() < 1e-3 &&
                        (as_shot[1] - 1.0).abs() < 1e-3 &&
                        (as_shot[2] - 1.0).abs() < 1e-3;
                    if neutral_under_d65 {
                        fused
                    } else {
                        let scene_white_xyz = if as_shot[0] > 1e-6 && as_shot[1] > 1e-6 && as_shot[2] > 1e-6 {
                            let mut v = mat_mul_vec3(&cam_to_xyz, &as_shot);
                            v[0] = v[0].clamp(0.3, 3.0);
                            v[1] = v[1].clamp(0.3, 3.0);
                            v[2] = v[2].clamp(0.3, 3.0);
                            v
                        } else {
                            D65_XYZ
                        };
                        let bradford_adapt = build_bradford_matrix(&scene_white_xyz, &D65_XYZ);
                        let cam_to_xyz_d65 = mat_mul_3x3(&bradford_adapt, &cam_to_xyz);
                        mat_mul_3x3(&xyz_to_output, &cam_to_xyz_d65)
                    }
                } else {
                    fused
                };

                // Resolve black/white levels from the selected BL/WL mode.
                // These are the "src" values used by the lens correction formula
                // (`(raw - src_bl) / (src_wl - src_bl)`), matching motioncam-fs.
                let (src_bl_r, src_bl_g, src_bl_b, src_wl) = match blwl_mode {
                    BlWlMode::Dynamic => {
                        if let Some(dbl) = slot.dynamic_black_level {
                            let g_bl = if dbl[1] > 0.0 && dbl[2] > 0.0 {
                                (dbl[1] + dbl[2]) / 2.0
                            } else {
                                dbl[1].max(dbl[2])
                            };
                            let wl = slot.dynamic_white_level
                                .map(|w| w as f64)
                                .unwrap_or(white_level);
                            (dbl[0] as f64, g_bl as f64, dbl[3] as f64, wl)
                        } else {
                            (bl_static_r, bl_static_g, bl_static_b, white_level)
                        }
                    }
                    BlWlMode::Static => (bl_static_r, bl_static_g, bl_static_b, white_level),
                    BlWlMode::Preset1023_64 => (64.0, 64.0, 64.0, 1023.0),
                    BlWlMode::Preset4095_256 => (256.0, 256.0, 256.0, 4095.0),
                    BlWlMode::Preset16383_1024 => (1024.0, 1024.0, 1024.0, 16383.0),
                    BlWlMode::Preset65535_4096 => (4096.0, 4096.0, 4096.0, 65535.0),
                    BlWlMode::Preset4095_64 => (64.0, 64.0, 64.0, 4095.0),
                    BlWlMode::Preset16383_64 => (64.0, 64.0, 64.0, 16383.0),
                    BlWlMode::Preset16383_0 => (0.0, 0.0, 0.0, 16383.0),
                };

                // Compute extended white level and normalization black/white.
                // When lens correction is active we follow motioncam-fs:
                //   - Output range is extended by +2 bits for headroom
                //   - Black level becomes 0 (already subtracted in correction)
                let (norm_bl_r, norm_bl_g, norm_bl_b, norm_wl) = if has_lens {
                    let bits = (u16::BITS - (src_wl as u16).leading_zeros()).max(1);
                    let ext = ((1u64 << (bits + 2).min(16)) - 1) as f64;
                    (0.0, 0.0, 0.0, ext)
                } else {
                    (src_bl_r, src_bl_g, src_bl_b, src_wl)
                };

                // Censored-photosite completion, in the RAW domain, BEFORE lens
                // correction (the sensor railed at src_wl; the shading map is a
                // calibration gain that must be divided back out, not folded
                // into the estimate) and BEFORE demosaic. The resolved src
                // blacks and rail are the same values the lens formula uses.
                // `hp` is hoisted: the GPU path receives the same params for
                // its own on-device pass, so exactly one backend completes
                // each frame (mutual exclusion — idempotence would make
                // overlap harmless, but the redundant CPU pass costs ~15 ms).
                let hp = hl::HlParams::new(if highlight_recovery { hl::HlPolicy::Full } else { hl::HlPolicy::Sensor }, src_wl, [src_bl_r, src_bl_g, src_bl_b], as_shot);
                // GPU-HL is valid ONLY on raw-domain mosaic: `hp` is
                // calibrated for pre-lens DNs (rail src_wl, blacks src_bl).
                // With lens correction on, the device mosaic is
                // black-subtracted and rescaled to the 0..norm_wl domain
                // (4095 for 10-bit), where raw-domain params miscensor and
                // compress (measured: sun core 0.47x level). So lens-on
                // completion is owned by the CPU-HL pass below for BOTH
                // backends; the on-device pass serves lens-off exports.
                let gpu_hl = rcd_pipeline.is_some() && highlight_recovery && !has_lens;
                if highlight_recovery && !gpu_hl {
                    let _g = PhaseGuard::new(&stats.hl_complete);
                    let (_cens, rewritten) = hl_scratch.apply(&mut slot.bayer, &hl_geom, &hp);
                    if frame_index % 60 == 0 {
                        tracing::info!("hl-complete: censored={} rewritten={} {}", _cens, rewritten, hp.summary());
                    }
                } else {
                    // OFF (--no-highlight-recovery): bit-exact sensor truth.
                    // No raw photosite is written; the magenta is the sensor's
                    // own colour error, documented not hidden. The
                    // ratio-preserving display rolloff still applies
                    // downstream. HlPolicy::Prior (multi-censored blocks only)
                    // stays available as an internal middle arm for the
                    // hl_review harness, not as export behaviour.
                }

                // Lens correction: apply before both GPU and CPU paths to
                // ensure corrected bayer data is used regardless of backend.
                if has_lens {
                    if let Some(ref sm) = shading_map {
                        let channels = if lens_mode == LensCorrectionMode::ColorOnly {
                            color_only_map.as_ref().unwrap()
                        } else {
                            &sm.channels
                        };
                        let _g = PhaseGuard::new(&stats.lens_correction);
                        apply_lens_correction_cpu_with_map(
                            &mut slot.bayer, stride_width,
                            offset_x, offset_y, pattern,
                            channels, sm.width, sm.height,
                            sensor_w, sensor_h,
                            [src_bl_r as f32, src_bl_g as f32, src_bl_g as f32, src_bl_b as f32],
                            src_wl as f32,
                            norm_wl as u16,
                        );
                    }
                }

                // Diagnostic exposure shift (CLI --exposure-ev, default
                // 0 = off): linear gain on the mosaic after completion and
                // lens, before demosaic/upload, so both backends see
                // identically scaled data (GPU uploads this buffer). The
                // black pedestal scales with the gain — a documented
                // approximation, irrelevant for highlight inspection,
                // which is all this flag is for.
                if exposure_ev != 0.0 && exposure_ev.is_finite() {
                    apply_exposure_gain(&mut slot.bayer, exposure_ev);
                }

                // GPU gets the same per-channel blacks the CPU normalizes
                // with (norm_bl_*): 0 when lens correction already
                // subtracted them, else the resolved src blacks. The WGSL
                // normalize divides each plane by its own per-channel range,
                // matching `normalize_linear_per_channel` exactly.
                let (gpu_bl_r, gpu_bl_g, gpu_bl_b) = if has_lens {
                    (0.0f32, 0.0f32, 0.0f32)
                } else {
                    (src_bl_r as f32, src_bl_g as f32, src_bl_b as f32)
                };
                let gpu_wl = if has_lens { norm_wl as f32 } else { white_level as f32 };
                let gpu_ok = if let Some(ref mut pipeline) = rcd_pipeline {
                    let gpu_result = {
                        let _g = PhaseGuard::new(&stats.gpu);
                        pipeline.process(&slot.bayer, filters, gpu_bl_r, gpu_bl_g, gpu_bl_b, gpu_wl, stride_width, offset_x, offset_y, &fused, &slot.as_shot_neutral, &export_tf, &hp, gpu_hl, pattern)
                    };
                    match gpu_result {
                        Ok(rgb48le) => {
                            stats.gpu_frames.fetch_add(1, Ordering::Relaxed);
                            slot.frame_bytes.copy_from_slice(&rgb48le);
                            true
                        } Err(_) => false
                    }
                } else { false };

                if !gpu_ok {
                    if gpu_hl {
                        // The GPU owned this frame's completion but the
                        // dispatch failed: fall back to the CPU pass before
                        // the CPU demosaic, or the raw censored bayer would
                        // demosaic unrecovered (a silent regression vs the
                        // pre-GPU behaviour, where the bayer was completed).
                        let _g = PhaseGuard::new(&stats.hl_complete);
                        hl_scratch.apply(&mut slot.bayer, &hl_geom, &hp);
                    }
                    {
                        let _g = PhaseGuard::new(&stats.demosaic);
                        demosaic.process_par_into(&slot.bayer, stride_width, offset_x, offset_y, active_width, active_height, &pattern, &mut rgb)?;
                    }
                    {
                        let _g = PhaseGuard::new(&stats.normalize);
                        normalize_linear_per_channel(&mut rgb, norm_bl_r, norm_bl_g, norm_bl_b, norm_wl);
                    }

                    let raw_r_gain = if as_shot[0] > 1e-6 && as_shot[1] > 1e-6 { as_shot[1] / as_shot[0] } else { 1.0 };
                    let raw_b_gain = if as_shot[2] > 1e-6 && as_shot[1] > 1e-6 { as_shot[1] / as_shot[2] } else { 1.0 };
                    tracing::info!("as_shot=[{:.4},{:.4},{:.4}] raw_wb_r={:.4} raw_wb_b={:.4}", as_shot[0], as_shot[1], as_shot[2], raw_r_gain, raw_b_gain);
                    let r_gain = raw_r_gain.clamp(0.1, 10.0);
                    let b_gain = raw_b_gain.clamp(0.1, 10.0);
                    if (r_gain - raw_r_gain).abs() > 1e-3 || (b_gain - raw_b_gain).abs() > 1e-3 {
                        tracing::warn!(
                            "CPU WB gains clamped: as_shot={:?} raw=[{:.3},{:.3}] clamped=[{:.3},{:.3}]",
                            as_shot, raw_r_gain, raw_b_gain, r_gain, b_gain
                        );
                    }
                    {
                        let _g = PhaseGuard::new(&stats.wb_hl_ccm);
                        rgb.par_chunks_exact_mut(3).for_each(|chunk| {
                            // 1. Apply WB
                            let mut wb = [chunk[0] * r_gain, chunk[1], chunk[2] * b_gain];

                            // 1b. OFF/basic mode only: classic neutral desat
                            // of clipped highlights (no RAW reconstruction).
                            // ON/reconstruction must NOT run this — it would
                            // crush the completed 1.0-1.9 gradient (08).
                            if !highlight_recovery {
                                apply_basic_highlight_desat(&mut wb);
                            }

                            // 2. Apply CCM
                            let out = mat_mul_vec3(&fused, &wb);
                            chunk[0] = out[0].max(0.0);
                            chunk[1] = out[1].max(0.0);
                            chunk[2] = out[2].max(0.0);
                        });
                    }
                    // Display-boundary handling, display-referred transfers only.
                    // Hue-preserving (uniform scale) so it cannot introduce a
                    // channel-ratio error, and identity below 1.0 so it cannot
                    // temper anything a display-referred deliverable would have
                    // shown as not-white. Scene-referred (log) exports are
                    // untouched: their headroom belongs to the colourist.
                    //
                    // ON/reconstruction only: hedge imbalanced above-rail
                    // residuals (single-censored warm-sky bias) toward
                    // luminance BEFORE the rolloff compresses levels. Balanced
                    // completed-neutral data passes through bit-identically.
                    if export_tf.is_display_referred() {
                        if highlight_recovery {
                            apply_on_highlight_imbalance_hedge(&mut rgb);
                        }
                        apply_display_rolloff(&mut rgb);
                    }
                    {
                        let _g = PhaseGuard::new(&stats.oetf);
                        export_tf.process(&mut rgb);
                    }

                    {
                        let _g = PhaseGuard::new(&stats.pack);
                        slot.frame_bytes.par_chunks_exact_mut(6).enumerate().for_each(|(pi, out)| {
                            let base = pi * 3;
                            let ru = (rgb[base].max(0.0) * 65535.0).min(65535.0) as u16; let gu = (rgb[base + 1].max(0.0) * 65535.0).min(65535.0) as u16; let bu = (rgb[base + 2].max(0.0) * 65535.0).min(65535.0) as u16;
                            out[0] = ru as u8; out[1] = (ru >> 8) as u8; out[2] = gu as u8; out[3] = (gu >> 8) as u8; out[4] = bu as u8; out[5] = (bu >> 8) as u8;
                        });
                    }
                }
                processed_tx.send(slot).map_err(|_| anyhow!("Processor: writer channel closed"))?;
            }
            drop(processed_tx); Ok(())
        }
    })?;
    
    let writer_cancelled = cancelled.clone(); let ffmpeg_pid = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)); ffmpeg_pid.store(encoder.pid(), Ordering::Relaxed);
    let monitor_cancelled = cancelled.clone(); let monitor_pid = ffmpeg_pid.clone();
    let _monitor_handle = std::thread::Builder::new().name("cancel_monitor".into()).spawn(move || {
        while !monitor_cancelled.load(Ordering::Relaxed) { std::thread::sleep(std::time::Duration::from_millis(50)); }
        let pid = monitor_pid.load(Ordering::Relaxed);
        if pid > 0 { #[cfg(target_os = "windows")] { let _ = std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).output(); } #[cfg(not(target_os = "windows"))] { let _ = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).output(); } }
    }).ok();
    
    let writer_handle = std::thread::Builder::new().name("writer".into()).spawn({
        let stats = Arc::clone(&stats);
        move || -> Result<()> {
            let mut frames_written: usize = 0;
            for slot in processed_rx {
                if writer_cancelled.load(Ordering::Relaxed) { let _ = free_tx_writer.send(slot); return Ok(()); }
                // C5: explicit start/end so we can tag the per-frame ring
                // with the frame_id. The `encode_push` PhaseTimer is still
                // updated (via record_encode_push_frame) for the histogram.
                let encode_start = std::time::Instant::now();
                encoder.push_frame(&slot.frame_bytes)?;
                stats.record_encode_push_frame(frames_written as u32, encode_start.elapsed());
                frames_written += 1; on_progress(frames_written as f64 / total_frames as f64 * 100.0); let _ = free_tx_writer.send(slot);
            }
            if writer_cancelled.load(Ordering::Relaxed) { return Ok(()); }
            {
                let _g = PhaseGuard::new(&stats.finalize);
                encoder.finish()?;
            }
            Ok(())
        }
    })?;
    
    let mut export_error: Option<anyhow::Error> = None;
    match loader_handle.join() { Ok(Ok(())) => {} Ok(Err(e)) => { cancelled.store(true, Ordering::Relaxed); export_error = Some(e); } Err(_) => { cancelled.store(true, Ordering::Relaxed); export_error = Some(anyhow!("Loader panicked")); } }
    match processor_handle.join() { Ok(Ok(())) => {} Ok(Err(e)) => { cancelled.store(true, Ordering::Relaxed); export_error.get_or_insert(e); } Err(_) => { cancelled.store(true, Ordering::Relaxed); export_error.get_or_insert(anyhow!("Processor panicked")); } }
    match writer_handle.join() { Ok(Ok(())) => {} Ok(Err(e)) => { export_error.get_or_insert(e); } Err(_) => { export_error.get_or_insert(anyhow!("Writer panicked")); } }
    if let Some(ref audio_path) = audio_temp_path { let _ = std::fs::remove_file(audio_path); }
    match export_error { Some(_) if cancelled.load(Ordering::Relaxed) => Err(anyhow!("Export cancelled")), Some(e) => Err(e), None => Ok(()) }
}
