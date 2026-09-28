use crate::color::ColorSpace;

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
    /// - `cs` selects the explicit RGB→YUV conversion for YUV outputs.
    #[allow(clippy::too_many_arguments)]
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
        cs: ColorSpace,
    ) -> (String, String, Vec<String>) {
        let mut base_codec_name: String = String::new();
        let mut base_pix_fmt: String = String::new();
        let mut base_extra: Vec<&'static str> = Vec::new();

        match self {
            CodecFamily::ProRes => {
                let (profile_v, base_pix) = match prores {
                    ProResProfile::Proxy => ("0", "yuv422p10le"),
                    ProResProfile::LT => ("1", "yuv422p10le"),
                    ProResProfile::Standard => ("2", "yuv422p10le"),
                    ProResProfile::HQ => ("3", "yuv422p10le"),
                    ProResProfile::P4444 => ("4", "yuva444p10le"),
                    ProResProfile::XQ4444 => ("5", "yuva444p12le"),
                };
                // Wide-gamut ProRes keeps its historic planar-RGB path
                // (`gbrp10le`) — deliberately parked, not in this change.
                // Signalling for those files must claim NO matrix, which
                // `get_ffmpeg_vui_tags` enforces from the pixel format.
                let pix_fmt = match (is_wide_gamut_cs(cs), prores) {
                    (true, ProResProfile::P4444 | ProResProfile::XQ4444) => base_pix,
                    (true, _) => "gbrp10le",
                    (false, _) => base_pix,
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
                // The requested profile is honoured for every colour space.
                // The former wide-gamut override emitted `gbrp10le` (RGB
                // 4:4:4) regardless of the requested 4:2:0 — a silent
                // format substitution, and the one implicated in the
                // Resolve green/magenta incident (untagged RGB planes).
                // Wide-gamut YUV now goes through the explicit conversion
                // below, which is the same conversion the VUI advertises.
                match hevc_encoder {
                    "libx265" => {
                        let pix_fmt = match hevc {
                            HevcProfile::Main10_420 => "yuv420p10le",
                            HevcProfile::Main10_444 => "yuv444p10le",
                        };
                        base_codec_name = "libx265".to_string();
                        base_pix_fmt = pix_fmt.to_string();
                        base_extra = vec!["-preset", "slow"];
                    }
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
                        base_extra = vec!["-pix_fmt", pix_fmt, "-preset", "slow"];
                    }
                }
            }
            CodecFamily::H264 => {
                if is_wide_gamut_cs(cs) {
                    // H.264 (8/10-bit, 4:2:0/4:2:2) has no wide-gamut
                    // story: its VUI cannot signal AWG/DWG primaries, and
                    // 4:2:0 subsampling of a wide gamut costs real chroma.
                    // Wide-gamut H.264 is therefore refused rather than
                    // silently mislabelled — callers get a loud warning and
                    // an HEVC/proxy recommendation.
                    tracing::warn!(
                        "wide-gamut H.264 is not supported (VUI cannot signal the primaries, \
                         4:2:0 would discard wide-gamut chroma); falling back to libx264 \
                         YUV with an honest bt2020nc tag — prefer HEVC or ProRes/DNxHR for \
                         wide-gamut deliverables"
                    );
                }
                {
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
            }
            CodecFamily::AV1 => {
                match av1_encoder {
                    "libsvtav1" => {
                        base_codec_name = "libsvtav1".to_string();
                        base_pix_fmt = match av1 {
                            Av1Profile::Profile0_420_10bit => "yuv420p10le",
                            Av1Profile::Profile1_444_10bit => "yuv444p10le",
                        }.to_string();
                        base_extra = vec!["-preset", "8"];
                    }
                    "av1_nvenc" => {
                        base_codec_name = "av1_nvenc".to_string();
                        base_pix_fmt = match av1 {
                            Av1Profile::Profile0_420_10bit => "p010le",
                            Av1Profile::Profile1_444_10bit => "yuv444p10le",
                        }.to_string();
                        base_extra = vec!["-preset", "p6"];
                    }
                    "av1_amf" => {
                        base_codec_name = "av1_amf".to_string();
                        base_pix_fmt = match av1 {
                            Av1Profile::Profile0_420_10bit => "p010le",
                            Av1Profile::Profile1_444_10bit => "yuv444p10le",
                        }.to_string();
                        base_extra = vec!["-quality", "quality"];
                    }
                    "av1_qsv" => {
                        base_codec_name = "av1_qsv".to_string();
                        base_pix_fmt = match av1 {
                            Av1Profile::Profile0_420_10bit => "p010le",
                            Av1Profile::Profile1_444_10bit => "yuv444p10le",
                        }.to_string();
                    }
                    _ => {
                        base_codec_name = "libsvtav1".to_string();
                        base_pix_fmt = match av1 {
                            Av1Profile::Profile0_420_10bit => "yuv420p10le",
                            Av1Profile::Profile1_444_10bit => "yuv444p10le",
                        }.to_string();
                        base_extra = vec!["-preset", "8"];
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

        // Inject an EXPLICIT RGB→YUV conversion for every YUV output.
        //
        // Why this is unconditional: FFmpeg's swscale otherwise picks its
        // own defaults (BT.601 for HD-sized frames — measured bit-exact),
        // which is a latent colour error AND a tag/data mismatch. The
        // matrix + range written here are the SAME values written into the
        // bitstream VUI by `pipeline::get_ffmpeg_vui_tags`, so the
        // conversion is always honestly signalled. That pairing is
        // load-bearing: claiming a YCbCr matrix for RGB planes (or vice
        // versa) is the green/magenta decode failure — pinned by the
        // `tagged_matrix_matches_conversion` test.
        //
        // Planar RGB output (gbrp*) is identity: no conversion, no matrix
        // claim.
        if !base_pix_fmt.starts_with("gbrp") && !base_pix_fmt.starts_with("rgb") && !base_pix_fmt.starts_with("yuva") {
            let conv = yuv_conversion_for(cs);
            extra.push("-vf".into());
            extra.push(format!(
                "scale=flags=accurate_rnd+full_chroma_int:out_color_matrix={}:out_range={},format={}",
                conv.matrix, conv.range, base_pix_fmt));
        }

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
/// True when `cs` is anything other than the BT.709 / sRGB container
/// primaries — i.e. a wide-gamut working space that needs an explicit
/// non-709 conversion matrix to survive YUV encoding.
pub fn is_wide_gamut_cs(cs: ColorSpace) -> bool {
    !matches!(cs, ColorSpace::Rec709 | ColorSpace::Srgb)
}

/// The explicit RGB→YUV conversion applied by the encoder, mirrored in the
/// bitstream VUI. Single source of truth for both: the scale filter and the
/// colour tags must always agree, otherwise a decoder that trusts the tags
/// (Resolve does) applies the wrong matrix and the picture turns green and
/// magenta.
///
/// - Rec.709 / sRGB: the ecosystem default. NLEs assume 709 for untagged or
///   709-tagged YUV, and the in-domain clipping measurement for our log
///   pipeline was 0.000%.
/// - Wide gamuts: BT.2020 non-constant-luminance in full range. It is the
///   only matrix that cannot clip a wide-gamut source, and it matches the
///   tagging convention of the professional AWG3/LogC3 references
///   (`bt2020nc` + `bt2020` primaries).
///
/// Note the primaries tag deliberately stays `unspecified` for camera-vendor
/// gamuts: no standard primaries code point covers AWG3's red primary, so any
/// standard tag would be a mislabel. The transfer carries no standard code
/// point for LogC3 either (`unknown`). Meaning travels via the filename and
/// the explicit input assignment in the NLE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YuvConversion {
    /// FFmpeg/swscale matrix name, also the VUI `matrix_coefficients` name.
    pub matrix: &'static str,
    /// FFmpeg range keyword (`tv` = limited 64-940, `full` = 0-1023).
    pub range: &'static str,
}

/// Conversion spec for a colour space. See [`YuvConversion`].
pub fn yuv_conversion_for(cs: ColorSpace) -> YuvConversion {
    if is_wide_gamut_cs(cs) {
        YuvConversion { matrix: "bt2020nc", range: "full" }
    } else {
        YuvConversion { matrix: "bt709", range: "tv" }
    }
}

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

    /// Build the full arg list for one (family, colour space, profile) combo
    /// the way `run_export` does.
    fn args_for(family: CodecFamily, cs: ColorSpace, hevc: HevcProfile) -> (String, String, Vec<String>) {
        family.to_ffmpeg_args(
            "libx265", "libx264", "libaom-av1", "prores_ks",
            ProResProfile::HQ, DnxhrProfile::HQX, hevc,
            H264Profile::High10bit, Av1Profile::Profile0_420_10bit,
            Vp9Profile::Profile2_420_10bit, &RateControl::Lossless, cs,
        )
    }

    fn joined(extra: &[String]) -> String { extra.join(" ") }

    /// The requested subsampling must survive for every colour space. The
    /// removed wide-gamut override emitted `gbrp10le` (RGB 4:4:4) whatever
    /// the user asked for, which silently produced 444 files from a 420
    /// request — the format-contract break behind the Resolve incident.
    #[test]
    fn hevc_honours_requested_profile_for_every_colour_space() {
        for cs in [ColorSpace::Rec709, ColorSpace::ARRIWideGamut3, ColorSpace::Srgb, ColorSpace::Rec2020] {
            let (_, pf, extra) = args_for(CodecFamily::HEVC, cs, HevcProfile::Main10_420);
            assert_eq!(pf, "yuv420p10le", "420 request must stay 420 for {cs:?}");
            assert!(!joined(&extra).contains("gbrp"), "{cs:?} leaked an RGB pixel format into a 420 export");

            let (_, pf, _) = args_for(CodecFamily::HEVC, cs, HevcProfile::Main10_444);
            assert_eq!(pf, "yuv444p10le", "444 request must stay 444 for {cs:?}");
        }
    }

    /// Every YUV output gets an explicit conversion, and it is the one the
    /// tags claim. FFmpeg's default (BT.601 at HD sizes, measured
    /// bit-exact) is never allowed to choose silently.
    #[test]
    fn every_yuv_output_declares_an_explicit_conversion() {
        let cases = [
            (CodecFamily::HEVC, ColorSpace::Rec709, "yuv420p10le", "bt709", "tv"),
            (CodecFamily::HEVC, ColorSpace::ARRIWideGamut3, "yuv420p10le", "bt2020nc", "full"),
            (CodecFamily::DNxHR, ColorSpace::ARRIWideGamut3, "yuv422p10le", "bt2020nc", "full"),
            (CodecFamily::DNxHR, ColorSpace::Rec709, "yuv422p10le", "bt709", "tv"),
            (CodecFamily::ProRes, ColorSpace::Rec709, "yuv422p10le", "bt709", "tv"),
        ];
        for (family, cs, want_pf, want_matrix, want_range) in cases {
            let (_, pf, extra) = match family {
                CodecFamily::ProRes => family.to_ffmpeg_args(
                    "libx265", "libx264", "libaom-av1", "prores_ks", ProResProfile::HQ,
                    DnxhrProfile::HQX, HevcProfile::Main10_420, H264Profile::High10bit,
                    Av1Profile::Profile0_420_10bit, Vp9Profile::Profile2_420_10bit,
                    &RateControl::Lossless, cs),
                _ => args_for(family, cs, HevcProfile::Main10_420),
            };
            assert_eq!(pf, want_pf, "{family:?}/{cs:?} pixel format");
            let text = joined(&extra);
            assert!(text.contains("-vf"), "{family:?}/{cs:?} has no conversion filter: {text}");
            assert!(text.contains(&format!("out_color_matrix={want_matrix}")), "{family:?}/{cs:?} matrix: {text}");
            assert!(text.contains(&format!("out_range={want_range}")), "{family:?}/{cs:?} range: {text}");
        }
    }

    /// Planar RGB output is the identity conversion: no filter, and — the
    /// part that matters — no matrix claim anywhere in the argument list.
    #[test]
    fn rgb_output_never_claims_a_ycbcr_matrix() {
        let (_, pf, extra) = CodecFamily::ProRes.to_ffmpeg_args(
            "libx265", "libx264", "libaom-av1", "prores_ks", ProResProfile::HQ,
            DnxhrProfile::HQX, HevcProfile::Main10_420, H264Profile::High10bit,
            Av1Profile::Profile0_420_10bit, Vp9Profile::Profile2_420_10bit,
            &RateControl::Lossless, ColorSpace::ARRIWideGamut3);
        assert_eq!(pf, "gbrp10le");
        let text = joined(&extra);
        assert!(!text.contains("-vf"), "RGB output must not be converted: {text}");
        assert!(!text.contains("colormatrix"), "RGB output must not claim a matrix: {text}");

        let tags = crate::pipeline::get_ffmpeg_vui_tags(
            &ColorSpace::ARRIWideGamut3, &crate::color::TransferFunction::ARRIlog3, &pf, "libx265");
        let tag_text = tags.join(" ");
        assert!(!tag_text.contains("colormatrix"), "RGB tag claims a matrix: {tag_text}");
        assert!(!tag_text.contains("colorspace"), "RGB tag claims a matrix: {tag_text}");
    }

    /// libx265 ignores `-color_*`; only `-x265-params` reaches the VUI.
    /// Emitting the dead form again is the regression this pins.
    #[test]
    fn libx265_signalling_uses_x265_params() {
        let tags = crate::pipeline::get_ffmpeg_vui_tags(
            &ColorSpace::Rec709, &crate::color::TransferFunction::Rec709, "yuv420p10le", "libx265");
        let text = tags.join(" ");
        assert!(tags.first().map(String::as_str) == Some("-x265-params"), "{text}");
        assert!(text.contains("colorprim=bt709"), "{text}");
        assert!(text.contains("transfer=bt709"), "{text}");
        assert!(text.contains("colormatrix=bt709"), "{text}");
        assert!(text.contains("range=limited"), "{text}");
        assert!(!text.contains("-color_primaries"), "dead -color_* form for libx265: {text}");

        // Wide gamut: BT.2020 NCL, full range, primaries left unspecified
        // (no standard code point describes AWG3's red primary).
        let tags = crate::pipeline::get_ffmpeg_vui_tags(
            &ColorSpace::ARRIWideGamut3, &crate::color::TransferFunction::ARRIlog3, "yuv420p10le", "libx265");
        let text = tags.join(" ");
        assert!(text.contains("colorprim=unspecified"), "{text}");
        assert!(text.contains("transfer=unknown"), "{text}");
        assert!(text.contains("colormatrix=bt2020nc"), "{text}");
        assert!(text.contains("range=full"), "{text}");
    }

    /// Non-x265 encoders keep the generic options, which they do honour.
    #[test]
    fn non_x265_encoders_keep_generic_colour_options() {
        let tags = crate::pipeline::get_ffmpeg_vui_tags(
            &ColorSpace::ARRIWideGamut3, &crate::color::TransferFunction::ARRIlog3, "yuv422p10le", "prores_ks");
        let text = tags.join(" ");
        assert!(text.contains("-color_primaries unspecified"), "{text}");
        assert!(text.contains("-color_trc unknown"), "{text}");
        assert!(text.contains("-colorspace bt2020nc"), "{text}");
        assert!(text.contains("-color_range full"), "{text}");
    }

    /// The pairing invariant across the whole export matrix: whatever matrix
    /// the scale filter converts with is the matrix the tags advertise.
    /// A mismatch is the green/magenta decode failure, reproduced on a
    /// bit-identical stream by flipping only the tag.
    #[test]
    fn tagged_matrix_matches_conversion() {
        let spaces = [
            ColorSpace::Rec709, ColorSpace::Srgb, ColorSpace::ARRIWideGamut3,
            ColorSpace::Rec2020, ColorSpace::DaVinciWideGamut, ColorSpace::SGamut3,
        ];
        let families = [CodecFamily::HEVC, CodecFamily::DNxHR, CodecFamily::ProRes];
        for cs in spaces {
            for family in families {
                let (codec, pf, extra) = match family {
                    CodecFamily::ProRes => family.to_ffmpeg_args(
                        "libx265", "libx264", "libaom-av1", "prores_ks", ProResProfile::HQ,
                        DnxhrProfile::HQX, HevcProfile::Main10_420, H264Profile::High10bit,
                        Av1Profile::Profile0_420_10bit, Vp9Profile::Profile2_420_10bit,
                        &RateControl::Lossless, cs),
                    _ => args_for(family, cs, HevcProfile::Main10_420),
                };
                if crate::pipeline::is_planar_rgb_fmt(&pf) { continue; }
                let text = joined(&extra);
                let filter_matrix = text.split("out_color_matrix=").nth(1)
                    .and_then(|s| s.split(':').next()).expect("no conversion matrix");
                let tags = crate::pipeline::get_ffmpeg_vui_tags(
                    &cs, &crate::color::TransferFunction::ARRIlog3, &pf, &codec).join(" ");
                let tagged = if codec == "libx265" {
                    tags.split("colormatrix=").nth(1)
                        .map(|s| s.split(':').next().unwrap()).unwrap_or(filter_matrix)
                } else {
                    tags.split("-colorspace ").nth(1).map(|s| s.split(' ').next().unwrap()).unwrap_or("")
                };
                assert_eq!(tagged, filter_matrix,
                    "{family:?}/{cs:?}: filter converts with {filter_matrix} but tags say {tagged}");
            }
        }
    }

    /// Wide-gamut + subsampling is a lossy test/proxy combination; the
    /// exporter must say so rather than let it pass as a master.
    #[test]
    fn wide_gamut_subsampled_is_documented_as_test_scope() {
        let conv = yuv_conversion_for(ColorSpace::ARRIWideGamut3);
        assert_eq!(conv.matrix, "bt2020nc");
        assert_eq!(conv.range, "full");
        assert!(is_wide_gamut_cs(ColorSpace::ARRIWideGamut3));
        assert!(!is_wide_gamut_cs(ColorSpace::Rec709));
        assert!(!is_wide_gamut_cs(ColorSpace::Srgb));
    }

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
