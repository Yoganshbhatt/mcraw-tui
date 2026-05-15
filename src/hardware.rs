use std::process::Command;

/// Runtime-detected hardware capabilities for video encoding.
pub struct HardwareCaps {
    /// Best available HEVC encoder name
    pub best_hevc_encoder: String,
    /// True if `best_hevc_encoder` is a hardware accelerator
    pub hevc_is_hw: bool,

    /// Best available H.264 encoder name
    pub best_h264_encoder: String,
    /// True if `best_h264_encoder` is a hardware accelerator
    pub h264_is_hw: bool,

    /// Best available AV1 encoder name
    pub best_av1_encoder: String,
    /// True if `best_av1_encoder` is a hardware accelerator
    pub av1_is_hw: bool,
}

/// Silently probe available FFmpeg HW encoders in priority order.
/// Falls back to the matching software encoder if none is found
/// or ffmpeg is missing.
pub fn probe_hardware() -> HardwareCaps {
    let output = match Command::new("ffmpeg").arg("-encoders").output() {
        Ok(o) => o.stdout,
        Err(_) => {
            return HardwareCaps {
                best_hevc_encoder: "libx265".to_string(),
                hevc_is_hw: false,
                best_h264_encoder: "libx264".to_string(),
                h264_is_hw: false,
                best_av1_encoder: "libaom-av1".to_string(),
                av1_is_hw: false,
            };
        }
    };
    let stdout = String::from_utf8_lossy(&output);

    let (hevc_enc, hevc_hw) = probe_one(
        &stdout,
        &["hevc_nvenc", "hevc_amf", "hevc_qsv", "hevc_videotoolbox"],
        "libx265",
    );

    let (h264_enc, h264_hw) = probe_one(
        &stdout,
        &["h264_nvenc", "h264_amf", "h264_qsv", "h264_videotoolbox"],
        "libx264",
    );

    let (av1_enc, av1_hw) = probe_one(
        &stdout,
        &["av1_nvenc", "av1_amf", "av1_qsv", "libsvtav1"],
        "libaom-av1",
    );

    HardwareCaps {
        best_hevc_encoder: hevc_enc,
        hevc_is_hw: hevc_hw,
        best_h264_encoder: h264_enc,
        h264_is_hw: h264_hw,
        best_av1_encoder: av1_enc,
        av1_is_hw: av1_hw,
    }
}

/// Walk `priority` in order; return the first encoder name found in `ffmpeg_output`.
/// If none match, return `fallback`. An encoder is considered hardware when its
/// name does **not** start with `"lib"`.
fn probe_one(ffmpeg_output: &str, priority: &[&str], fallback: &str) -> (String, bool) {
    for name in priority {
        if ffmpeg_output.contains(name) {
            let is_hw = !name.starts_with("lib");
            return (name.to_string(), is_hw);
        }
    }
    (fallback.to_string(), false)
}
