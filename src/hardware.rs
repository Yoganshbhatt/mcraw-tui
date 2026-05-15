use std::process::Command;

/// Runtime-detected hardware capabilities for video encoding.
pub struct HardwareCaps {
    /// Best available HEVC encoder name
    pub best_hevc_encoder: String,
    /// True if `best_hevc_encoder` is a hardware accelerator
    pub hevc_is_hw: bool,
}

/// Silently probe available FFmpeg HW encoders in priority order.
/// Falls back to `libx265` (software) if none is found or ffmpeg is missing.
pub fn probe_hardware() -> HardwareCaps {
    let output = match Command::new("ffmpeg").arg("-encoders").output() {
        Ok(o) => o.stdout,
        Err(_) => {
            return HardwareCaps {
                best_hevc_encoder: "libx265".to_string(),
                hevc_is_hw: false,
            };
        }
    };
    let stdout = String::from_utf8_lossy(&output);
    for name in &["hevc_nvenc", "hevc_amf", "hevc_qsv", "hevc_videotoolbox"] {
        if stdout.contains(name) {
            return HardwareCaps {
                best_hevc_encoder: name.to_string(),
                hevc_is_hw: true,
            };
        }
    }
    HardwareCaps {
        best_hevc_encoder: "libx265".to_string(),
        hevc_is_hw: false,
    }
}
