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
        prores_encoder: &str,
        prores: ProResProfile,
        dnxhr: DnxhrProfile,
        hevc: HevcProfile,
        h264: H264Profile,
        av1: Av1Profile,
        vp9: Vp9Profile,
        rate_control: &RateControl,
    ) -> (String, String, Vec<String>) {
        let mut base_codec_name: String = String::new();
        let mut base_pix_fmt: String = String::new();
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
                base_codec_name = prores_encoder.to_string();
                base_pix_fmt = pix_fmt.to_string();
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
                base_codec_name = "dnxhd".to_string();
                base_pix_fmt = pix_fmt.to_string();
                base_extra = vec!["-profile:v", profile_str];
            }
            CodecFamily::HEVC => {
                match hevc_encoder {
                    "hevc_nvenc" => {
                        base_codec_name = "hevc_nvenc".to_string();
                        base_pix_fmt = "p010le".to_string();
                        base_extra = vec!["-preset", "p6"];
                    }
                    "hevc_amf" => {
                        base_codec_name = "hevc_amf".to_string();
                        base_pix_fmt = "p010le".to_string();
                        base_extra = vec!["-quality", "quality"];
                    }
                    "hevc_qsv" => {
                        base_codec_name = "hevc_qsv".to_string();
                        base_pix_fmt = "p010le".to_string();
                    }
                    "hevc_videotoolbox" => {
                        base_codec_name = "hevc_videotoolbox".to_string();
                        base_pix_fmt = "p010le".to_string();
                        base_extra = vec!["-realtime", "true"];
                    }
                    _ => {
                        let pix_fmt = match hevc {
                            HevcProfile::Main10_420 => "yuv420p10le",
                            HevcProfile::Main10_444 => "yuv444p10le",
                        };
                        base_codec_name = "libx265".to_string();
                        base_pix_fmt = pix_fmt.to_string();
                        base_extra = vec!["-pix_fmt", pix_fmt];
                    }
                }
            }
            CodecFamily::H264 => {
                match h264_encoder {
                    "h264_nvenc" => {
                        let (pf, ext) = match h264 {
                            H264Profile::High10bit => ("p010le", vec!["-preset", "p6", "-profile:v", "high10"]),
                            H264Profile::Main8bit => ("yuv420p", vec!["-preset", "p6"]),
                        };
                        base_codec_name = "h264_nvenc".to_string();
                        base_pix_fmt = pf.to_string();
                        base_extra = ext;
                    }
                    "h264_amf" => {
                        let (pf, ext) = match h264 {
                            H264Profile::High10bit => ("p010le", vec!["-quality", "quality"]),
                            H264Profile::Main8bit => ("yuv420p", vec!["-quality", "quality"]),
                        };
                        base_codec_name = "h264_amf".to_string();
                        base_pix_fmt = pf.to_string();
                        base_extra = ext;
                    }
                    "h264_qsv" => {
                        let pf = match h264 {
                            H264Profile::High10bit => "p010le",
                            H264Profile::Main8bit => "yuv420p",
                        };
                        base_codec_name = "h264_qsv".to_string();
                        base_pix_fmt = pf.to_string();
                    }
                    "h264_videotoolbox" => {
                        let pf = match h264 {
                            H264Profile::High10bit => "p010le",
                            H264Profile::Main8bit => "yuv420p",
                        };
                        base_codec_name = "h264_videotoolbox".to_string();
                        base_pix_fmt = pf.to_string();
                        base_extra = vec!["-realtime", "true"];
                    }
                    _ => {
                        let (pf, ext) = match h264 {
                            H264Profile::Main8bit => ("yuv420p", vec!["-preset", "slow"]),
                            H264Profile::High10bit => ("yuv422p10le", vec!["-preset", "slow"]),
                        };
                        base_codec_name = "libx264".to_string();
                        base_pix_fmt = pf.to_string();
                        base_extra = ext;
                    }
                }
            }
            CodecFamily::AV1 => {
                match av1_encoder {
                    "av1_nvenc" => {
                        base_codec_name = "av1_nvenc".to_string();
                        base_pix_fmt = "p010le".to_string();
                        base_extra = vec!["-preset", "p6"];
                    }
                    "av1_amf" => {
                        base_codec_name = "av1_amf".to_string();
                        base_pix_fmt = "p010le".to_string();
                        base_extra = vec!["-quality", "quality"];
                    }
                    "av1_qsv" => {
                        base_codec_name = "av1_qsv".to_string();
                        base_pix_fmt = "p010le".to_string();
                    }
                    "libsvtav1" => {
                        base_codec_name = "libsvtav1".to_string();
                        base_pix_fmt = "yuv420p10le".to_string();
                        base_extra = vec!["-preset", "8"];
                    }
                    _ => {
                        base_codec_name = "libaom-av1".to_string();
                        base_pix_fmt = "yuv420p10le".to_string();
                        // `-cpu-used 4` is libaom's speed preset.
                        // `-b:v 0` is handled by `rate_control_args` for
                        // CQ modes (so bitrate modes can override cleanly).
                        base_extra = vec!["-cpu-used", "4"];
                    }
                }
            }
            CodecFamily::VP9 => {
                // VP9 quality / bitrate mode is fully driven by the user's
                // rate-control choice (`-crf X -b:v 0` for CQ modes,
                // `-b:v X -maxrate X` for bitrate modes — handled below).
                base_codec_name = "libvpx-vp9".to_string();
                base_pix_fmt = match vp9 {
                    Vp9Profile::Profile2_420_10bit => "yuv420p10le".to_string(),
                    Vp9Profile::Profile3_444_10bit => "yuv444p10le".to_string(),
                };
                base_extra = vec![];
            }
        }

        // Convert static extra args to owned Strings
        let mut extra: Vec<String> = base_extra.iter().map(|&s| s.to_string()).collect();

        // Append rate-control flags. ProRes / DNxHR ignore CRF / bitrate
        // flags (they use the explicit `-profile:v` instead), so we skip
        // them. Every other family — including VP9 and AV1 — honours the
        // user's rate-control choice.
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
            CodecFamily::VP9 => {
                // libvpx-vp9 is always a software encoder; pass it through
                // the same helper so Lossless / High / Standard / bitrate
                // / Custom presets all work consistently.
                extra.extend(rate_control_args(rate_control, "libvpx-vp9"));
            }
            CodecFamily::ProRes | CodecFamily::DNxHR => {}
        }

        tracing::debug!("ffmpeg args: codec={} pix_fmt={} extra={:?}",
            base_codec_name, base_pix_fmt, extra);

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
    Main8bit,
    High10bit,
}

impl H264Profile {
    pub fn name(&self) -> &'static str {
        match self {
            H264Profile::Main8bit => "Main 8-bit",
            H264Profile::High10bit => "High 10-bit",
        }
    }

    pub fn is_8bit(&self) -> bool {
        matches!(self, H264Profile::Main8bit)
    }

    pub fn all() -> &'static [H264Profile] {
        &[H264Profile::Main8bit, H264Profile::High10bit]
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

    pub fn prev(&self) -> Self {
        match self {
            RateControl::Lossless => RateControl::Custom(String::new()),
            RateControl::High => RateControl::Lossless,
            RateControl::Standard => RateControl::High,
            RateControl::Master400M => RateControl::Standard,
            RateControl::Standard150M => RateControl::Master400M,
            RateControl::Custom(_) => RateControl::Standard150M,
        }
    }
}

/// Build the FFmpeg rate-control / quality arguments for a given encoder.
///
/// * `is_hw` — `true` for GPU-backed encoders (nvenc / amf / qsv / videotoolbox).
/// * `encoder_name` — the FFmpeg encoder name; used to pick the right flag set.
///
/// Special cases:
/// * **libvpx-vp9** and **libaom-av1** require `-b:v 0` alongside `-crf` to
///   enable constant-quality mode. Without `-b:v 0` they treat `-crf` as a
///   max-bitrate hint and fall back to default VBR.
/// * **NVENC** bitrate modes get an explicit `-rc:v vbr` so the preset's
///   default rate control (which varies by FFmpeg / driver version) doesn't
///   silently override the requested target.
pub fn rate_control_args(rc: &RateControl, encoder_name: &str) -> Vec<String> {
    let is_hw = !encoder_name.starts_with("lib");
    let is_videotoolbox = encoder_name.ends_with("_videotoolbox");
    let is_nvenc = encoder_name.ends_with("_nvenc");
    let needs_bv0_for_crf = matches!(encoder_name, "libvpx-vp9" | "libaom-av1");

    // Helper: produce a constant-quality arg pair for the encoder.
    let cq = |value: &str| -> Vec<String> {
        if is_videotoolbox {
            vec!["-quality".into(), value.into()]
        } else if is_hw {
            vec!["-cq".into(), value.into()]
        } else if needs_bv0_for_crf {
            vec!["-crf".into(), value.into(), "-b:v".into(), "0".into()]
        } else {
            vec!["-crf".into(), value.into()]
        }
    };

    // Helper: produce a target-bitrate arg set.
    let bitrate = |value: &str| -> Vec<String> {
        let mut v = vec![
            "-b:v".into(), value.into(),
            "-maxrate".into(), value.into(),
        ];
        if is_nvenc {
            // Pin NVENC into VBR mode so `-b:v` actually drives the encoder
            // (the default rc depends on preset + driver, which made bitrate
            // modes unreliable).
            v.push("-rc:v".into());
            v.push("vbr".into());
        }
        v
    };

    match rc {
        RateControl::Lossless => {
            if is_videotoolbox {
                vec!["-quality".into(), "lossless".into()]
            } else {
                cq("16")
            }
        }
        RateControl::High => {
            if is_videotoolbox {
                vec!["-quality".into(), "max".into()]
            } else {
                cq("20")
            }
        }
        RateControl::Standard => {
            if is_videotoolbox {
                vec!["-quality".into(), "high".into()]
            } else {
                cq("24")
            }
        }
        RateControl::Master400M => bitrate("400M"),
        RateControl::Standard150M => bitrate("150M"),
        RateControl::Custom(val) => {
            if val.is_empty() {
                return vec![];
            }
            let upper = val.to_uppercase();
            if upper.ends_with('M') || upper.ends_with('K') {
                bitrate(val)
            } else if val.parse::<f64>().is_ok() {
                cq(val)
            } else {
                // Pass the raw string directly — FFmpeg validates it.
                vec![val.clone()]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_control_lossless_software_uses_crf() {
        let args = rate_control_args(&RateControl::Lossless, "libx265");
        assert_eq!(args, vec!["-crf", "16"]);
    }

    #[test]
    fn rate_control_lossless_nvenc_uses_cq() {
        let args = rate_control_args(&RateControl::Lossless, "hevc_nvenc");
        assert_eq!(args, vec!["-cq", "16"]);
    }

    #[test]
    fn rate_control_lossless_videotoolbox_uses_quality_lossless() {
        let args = rate_control_args(&RateControl::Lossless, "hevc_videotoolbox");
        assert_eq!(args, vec!["-quality", "lossless"]);
    }

    #[test]
    fn rate_control_nvenc_bitrate_mode_pins_rc_to_vbr() {
        // Regression test: NVENC bitrate modes used to silently get the
        // preset's default rate-control mode, which made `-b:v` unreliable.
        let args = rate_control_args(&RateControl::Master400M, "hevc_nvenc");
        assert!(args.contains(&"-b:v".to_string()));
        assert!(args.contains(&"-maxrate".to_string()));
        assert!(args.contains(&"-rc:v".to_string()));
        assert!(args.contains(&"vbr".to_string()));
    }

    #[test]
    fn rate_control_vp9_crf_adds_bv0() {
        // Regression test: libvpx-vp9 needs `-b:v 0` alongside `-crf`,
        // otherwise it silently falls back to default VBR.
        let args = rate_control_args(&RateControl::Standard, "libvpx-vp9");
        assert!(args.contains(&"-crf".to_string()));
        assert!(args.contains(&"24".to_string()));
        assert!(args.contains(&"-b:v".to_string()));
        assert!(args.contains(&"0".to_string()));
    }

    #[test]
    fn rate_control_libaom_av1_crf_adds_bv0() {
        let args = rate_control_args(&RateControl::High, "libaom-av1");
        assert!(args.contains(&"-crf".to_string()));
        assert!(args.contains(&"20".to_string()));
        assert!(args.contains(&"-b:v".to_string()));
        assert!(args.contains(&"0".to_string()));
    }

    #[test]
    fn rate_control_custom_numeric_routes_to_cq() {
        let args = rate_control_args(&RateControl::Custom("18".into()), "libx265");
        assert_eq!(args, vec!["-crf", "18"]);
    }

    #[test]
    fn rate_control_custom_bitrate_routes_to_bv() {
        let args = rate_control_args(&RateControl::Custom("50M".into()), "libx265");
        assert!(args.contains(&"-b:v".to_string()));
        assert!(args.contains(&"50M".to_string()));
    }

    #[test]
    fn rate_control_custom_empty_returns_empty() {
        let args = rate_control_args(&RateControl::Custom(String::new()), "libx265");
        assert!(args.is_empty());
    }
}
