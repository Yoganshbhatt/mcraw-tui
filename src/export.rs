#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecFamily {
    ProRes,
    DNxHR,
    HEVC,
    H264,
    AV1,
    VP9,
}

impl CodecFamily {
    pub fn name(&self) -> &'static str {
        match self {
            CodecFamily::ProRes => "ProRes",
            CodecFamily::DNxHR => "DNxHR",
            CodecFamily::HEVC => "HEVC",
            CodecFamily::H264 => "H.264",
            CodecFamily::AV1 => "AV1",
            CodecFamily::VP9 => "VP9",
        }
    }

    pub fn all() -> &'static [CodecFamily] {
        &[
            CodecFamily::ProRes,
            CodecFamily::DNxHR,
            CodecFamily::HEVC,
            CodecFamily::H264,
            CodecFamily::AV1,
            CodecFamily::VP9,
        ]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }

    /// Build FFmpeg arguments:
    /// - Codec and pixel format are determined by the family and the
    ///   runtime-detected encoder names.
    /// - Profile is resolved independently so the user's choice is preserved.
    /// - Rate-control flags are appended for HEVC / H.264 / AV1.
    pub fn to_ffmpeg_args(
        &self,
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
        let mut base_codec_name: &str = "";
        let mut base_pix_fmt: &str = "";
        let mut base_extra: Vec<&'static str> = Vec::new();

        match self {
            CodecFamily::ProRes => {
                let (profile_v, pix_fmt) = match prores {
                    ProResProfile::Proxy => ("0", "yuv422p10le"),
                    ProResProfile::LT => ("1", "yuv422p10le"),
                    ProResProfile::Standard => ("2", "yuv422p10le"),
                    ProResProfile::HQ => ("3", "yuv422p10le"),
                    ProResProfile::P4444 => ("4", "yuva444p10le"),
                    ProResProfile::XQ4444 => ("5", "yuva444p12le"),
                };
                base_codec_name = "prores_ks";
                base_pix_fmt = pix_fmt;
                base_extra = vec!["-profile:v", profile_v];
            }
            CodecFamily::DNxHR => {
                let (profile_str, pix_fmt) = match dnxhr {
                    DnxhrProfile::SQ => ("dnxhr_sq", "yuv422p10le"),
                    DnxhrProfile::HD => ("dnxhr_hd", "yuv422p10le"),
                    DnxhrProfile::HDX => ("dnxhr_hdx", "yuv422p10le"),
                    DnxhrProfile::HQX => ("dnxhr_hqx", "yuv422p10le"),
                    DnxhrProfile::P444 => ("dnxhr_444", "yuv444p10le"),
                };
                base_codec_name = "dnxhd";
                base_pix_fmt = pix_fmt;
                base_extra = vec!["-profile:v", profile_str];
            }
            CodecFamily::HEVC => {
                match hevc_encoder {
                    "hevc_nvenc" => {
                        base_codec_name = "hevc_nvenc";
                        base_pix_fmt = "p010le";
                        base_extra = vec!["-preset", "p6"];
                    }
                    "hevc_amf" => {
                        base_codec_name = "hevc_amf";
                        base_pix_fmt = "p010le";
                        base_extra = vec!["-quality", "quality"];
                    }
                    "hevc_qsv" => {
                        base_codec_name = "hevc_qsv";
                        base_pix_fmt = "p010le";
                    }
                    "hevc_videotoolbox" => {
                        base_codec_name = "hevc_videotoolbox";
                        base_pix_fmt = "p010le";
                    }
                    _ => {
                        let pix_fmt = match hevc {
                            HevcProfile::Main10_420 => "yuv420p10le",
                            HevcProfile::Main10_444 => "yuv444p10le",
                        };
                        base_codec_name = "libx265";
                        base_pix_fmt = pix_fmt;
                        base_extra = vec!["-pix_fmt", pix_fmt];
                    }
                }
            }
            CodecFamily::H264 => {
                match h264_encoder {
                    "h264_nvenc" => {
                        let (pf, ext) = match h264 {
                            H264Profile::High_10bit => ("p010le", vec!["-preset", "p6", "-profile:v", "high10"]),
                            H264Profile::Main_8bit => ("yuv420p", vec!["-preset", "p6"]),
                        };
                        base_codec_name = "h264_nvenc";
                        base_pix_fmt = pf;
                        base_extra = ext;
                    }
                    "h264_amf" => {
                        let (pf, ext) = match h264 {
                            H264Profile::High_10bit => ("p010le", vec!["-quality", "quality"]),
                            H264Profile::Main_8bit => ("yuv420p", vec!["-quality", "quality"]),
                        };
                        base_codec_name = "h264_amf";
                        base_pix_fmt = pf;
                        base_extra = ext;
                    }
                    "h264_qsv" => {
                        let pf = match h264 {
                            H264Profile::High_10bit => "p010le",
                            H264Profile::Main_8bit => "yuv420p",
                        };
                        base_codec_name = "h264_qsv";
                        base_pix_fmt = pf;
                    }
                    "h264_videotoolbox" => {
                        let pf = match h264 {
                            H264Profile::High_10bit => "p010le",
                            H264Profile::Main_8bit => "yuv420p",
                        };
                        base_codec_name = "h264_videotoolbox";
                        base_pix_fmt = pf;
                    }
                    _ => {
                        let (pf, ext) = match h264 {
                            H264Profile::Main_8bit => ("yuv420p", vec!["-preset", "slow"]),
                            H264Profile::High_10bit => ("yuv422p10le", vec!["-preset", "slow"]),
                        };
                        base_codec_name = "libx264";
                        base_pix_fmt = pf;
                        base_extra = ext;
                    }
                }
            }
            CodecFamily::AV1 => {
                match av1_encoder {
                    "av1_nvenc" => {
                        base_codec_name = "av1_nvenc";
                        base_pix_fmt = "p010le";
                        base_extra = vec!["-preset", "p6"];
                    }
                    "av1_amf" => {
                        base_codec_name = "av1_amf";
                        base_pix_fmt = "p010le";
                        base_extra = vec!["-quality", "quality"];
                    }
                    "av1_qsv" => {
                        base_codec_name = "av1_qsv";
                        base_pix_fmt = "p010le";
                    }
                    "libsvtav1" => {
                        base_codec_name = "libsvtav1";
                        base_pix_fmt = "yuv420p10le";
                        base_extra = vec!["-preset", "8"];
                    }
                    _ => {
                        base_codec_name = "libaom-av1";
                        base_pix_fmt = "yuv420p10le";
                        base_extra = vec!["-cpu-used", "4"];
                    }
                }
            }
            CodecFamily::VP9 => {
                let crf = match vp9 {
                    Vp9Profile::Profile2_420_10bit => "30",
                    Vp9Profile::Profile3_444_10bit => "30",
                };
                base_codec_name = "libvpx-vp9";
                base_pix_fmt = "yuv420p10le";
                base_extra = vec!["-crf", crf, "-b:v", "0"];
            }
        }

        // Convert static extra args to owned Strings
        let mut extra: Vec<String> = base_extra.iter().map(|&s| s.to_string()).collect();

        // Append rate-control flags for HEVC / H.264 / AV1 (not ProRes / DNxHR / VP9)
        match self {
            CodecFamily::HEVC => {
                extra.extend(rate_control_args(rate_control, hevc_encoder));
            }
            CodecFamily::H264 => {
                extra.extend(rate_control_args(rate_control, h264_encoder));
            }
            CodecFamily::AV1 => {
                extra.extend(rate_control_args(rate_control, av1_encoder));
            }
            _ => {}
        }

        (base_codec_name, base_pix_fmt, extra)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProResProfile {
    Proxy,
    LT,
    Standard,
    HQ,
    P4444,
    XQ4444,
}

impl ProResProfile {
    pub fn name(&self) -> &'static str {
        match self {
            ProResProfile::Proxy => "Proxy",
            ProResProfile::LT => "LT",
            ProResProfile::Standard => "Standard",
            ProResProfile::HQ => "HQ",
            ProResProfile::P4444 => "4444",
            ProResProfile::XQ4444 => "4444 XQ",
        }
    }

    pub fn all() -> &'static [ProResProfile] {
        &[
            ProResProfile::Proxy,
            ProResProfile::LT,
            ProResProfile::Standard,
            ProResProfile::HQ,
            ProResProfile::P4444,
            ProResProfile::XQ4444,
        ]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnxhrProfile {
    SQ,
    HD,
    HDX,
    HQX,
    P444,
}

impl DnxhrProfile {
    pub fn name(&self) -> &'static str {
        match self {
            DnxhrProfile::SQ => "SQ",
            DnxhrProfile::HD => "HD",
            DnxhrProfile::HDX => "HDX",
            DnxhrProfile::HQX => "HQX",
            DnxhrProfile::P444 => "444",
        }
    }

    pub fn all() -> &'static [DnxhrProfile] {
        &[DnxhrProfile::SQ, DnxhrProfile::HD, DnxhrProfile::HDX, DnxhrProfile::HQX, DnxhrProfile::P444]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HevcProfile {
    Main10_420,
    Main10_444,
}

impl HevcProfile {
    pub fn name(&self) -> &'static str {
        match self {
            HevcProfile::Main10_420 => "Main 10 4:2:0",
            HevcProfile::Main10_444 => "Main 10 4:4:4",
        }
    }

    pub fn is_8bit(&self) -> bool {
        false
    }

    pub fn all() -> &'static [HevcProfile] {
        &[HevcProfile::Main10_420, HevcProfile::Main10_444]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264Profile {
    Main_8bit,
    High_10bit,
}

impl H264Profile {
    pub fn name(&self) -> &'static str {
        match self {
            H264Profile::Main_8bit => "Main 8-bit",
            H264Profile::High_10bit => "High 10-bit",
        }
    }

    pub fn is_8bit(&self) -> bool {
        matches!(self, H264Profile::Main_8bit)
    }

    pub fn all() -> &'static [H264Profile] {
        &[H264Profile::Main_8bit, H264Profile::High_10bit]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Av1Profile {
    Profile0_420_10bit,
    Profile1_444_10bit,
}

impl Av1Profile {
    pub fn name(&self) -> &'static str {
        match self {
            Av1Profile::Profile0_420_10bit => "Profile 0 4:2:0 10-bit",
            Av1Profile::Profile1_444_10bit => "Profile 1 4:4:4 10-bit",
        }
    }

    pub fn is_8bit(&self) -> bool {
        false
    }

    pub fn all() -> &'static [Av1Profile] {
        &[Av1Profile::Profile0_420_10bit, Av1Profile::Profile1_444_10bit]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vp9Profile {
    Profile2_420_10bit,
    Profile3_444_10bit,
}

impl Vp9Profile {
    pub fn name(&self) -> &'static str {
        match self {
            Vp9Profile::Profile2_420_10bit => "Profile 2 4:2:0 10-bit",
            Vp9Profile::Profile3_444_10bit => "Profile 3 4:4:4 10-bit",
        }
    }

    pub fn is_8bit(&self) -> bool {
        false
    }

    pub fn all() -> &'static [Vp9Profile] {
        &[Vp9Profile::Profile2_420_10bit, Vp9Profile::Profile3_444_10bit]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

// ---------------------------------------------------------------------------
// Rate Control
// ---------------------------------------------------------------------------

/// A hybrid rate-control / constant-quality preset.
///
/// Quality presets (`Lossless` / `High` / `Standard`) map to `-cq` (HW) or
/// `-crf` (SW).  Bitrate presets (`Master400M` / `Standard150M`) map to
/// `-b:v` / `-maxrate`.  The `Custom` variant lets the user type an arbitrary
/// FFmpeg rate-control argument.
#[derive(Debug, Clone)]
pub enum RateControl {
    Lossless,
    High,
    Standard,
    Master400M,
    Standard150M,
    Custom(String),
}

impl RateControl {
    pub fn name(&self) -> String {
        match self {
            RateControl::Lossless => "Lossless".to_string(),
            RateControl::High => "High Quality".to_string(),
            RateControl::Standard => "Standard".to_string(),
            RateControl::Master400M => "Master 400M".to_string(),
            RateControl::Standard150M => "Standard 150M".to_string(),
            RateControl::Custom(v) => {
                if v.is_empty() {
                    "Custom: []".to_string()
                } else {
                    format!("Custom: [{}]", v)
                }
            }
        }
    }

    pub fn next(&self) -> Self {
        match self {
            RateControl::Lossless => RateControl::High,
            RateControl::High => RateControl::Standard,
            RateControl::Standard => RateControl::Master400M,
            RateControl::Master400M => RateControl::Standard150M,
            RateControl::Standard150M => RateControl::Custom(String::new()),
            RateControl::Custom(_) => RateControl::Lossless,
        }
    }
}

/// Build the FFmpeg rate-control / quality arguments for a given encoder.
///
/// * `is_hw` — `true` for GPU-backed encoders (nvenc / amf / qsv / videotoolbox).
/// * `codec_name` — the FFmpeg encoder name (used only for default fallback).
pub fn rate_control_args(rc: &RateControl, encoder_name: &str) -> Vec<String> {
    let is_hw = !encoder_name.starts_with("lib");
    match rc {
        RateControl::Lossless => {
            if is_hw {
                vec!["-cq".into(), "16".into()]
            } else {
                vec!["-crf".into(), "16".into()]
            }
        }
        RateControl::High => {
            if is_hw {
                vec!["-cq".into(), "20".into()]
            } else {
                vec!["-crf".into(), "20".into()]
            }
        }
        RateControl::Standard => {
            if is_hw {
                vec!["-cq".into(), "24".into()]
            } else {
                vec!["-crf".into(), "24".into()]
            }
        }
        RateControl::Master400M => {
            vec![
                "-b:v".into(),
                "400M".into(),
                "-maxrate".into(),
                "400M".into(),
            ]
        }
        RateControl::Standard150M => {
            vec![
                "-b:v".into(),
                "150M".into(),
                "-maxrate".into(),
                "150M".into(),
            ]
        }
        RateControl::Custom(val) => {
            if val.is_empty() {
                return vec![];
            }
            let upper = val.to_uppercase();
            if upper.ends_with('M') || upper.ends_with('K') {
                vec![
                    "-b:v".into(),
                    val.clone(),
                    "-maxrate".into(),
                    val.clone(),
                ]
            } else if val.parse::<f64>().is_ok() {
                if is_hw {
                    vec!["-cq".into(), val.clone()]
                } else {
                    vec!["-crf".into(), val.clone()]
                }
            } else {
                // Pass the raw string directly — FFmpeg validates it.
                vec![val.clone()]
            }
        }
    }
}
