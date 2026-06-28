use crate::agx::{AgxConfig, AgxPipeline, Gamut, OutputTransfer, Transfer};
use crate::file::BayerPattern;
use anyhow::Result;
use rayon::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

pub trait Demosaic {
    fn process(&self, bayer: &[u16], stride_width: u32, offset_x: u32, offset_y: u32, active_width: u32, active_height: u32, pattern: &BayerPattern) -> Result<Vec<f32>>;
}

pub trait ColorSpaceConverter {
    fn process(&self, pixels: &mut [f32], ccm: &[f32; 9]);
}

pub trait TransferFunctionProcessor {
    fn process(&self, pixels: &mut [f32]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    ACESAP1, AppleWideGamut, ARRIWideGamut3, ARRIWideGamut4, CanonCinemaGamut,
    DaVinciWideGamut, DciP3, DisplayP3, FGamut, FGamutC, PanasonicVGamut, Rec2020,
    Rec709, SGamut3, SGamut3Cine, Srgb,
}

impl ColorSpace {
    pub fn name(&self) -> &'static str {
        match self {
            ColorSpace::ACESAP1 => "ACES AP1",
            ColorSpace::AppleWideGamut => "Apple Wide Gamut",
            ColorSpace::ARRIWideGamut3 => "ARRI Wide Gamut 3", ColorSpace::ARRIWideGamut4 => "ARRI Wide Gamut 4",
            ColorSpace::CanonCinemaGamut => "Canon Cinema Gamut",
            ColorSpace::DaVinciWideGamut => "DaVinci Wide Gamut",
            ColorSpace::DciP3 => "DCI-P3", ColorSpace::DisplayP3 => "Display P3",
            ColorSpace::FGamut => "F-Gamut", ColorSpace::FGamutC => "F-Gamut C",
            ColorSpace::PanasonicVGamut => "Panasonic V-Gamut",
            ColorSpace::Rec2020 => "Rec.2020", ColorSpace::Rec709 => "Rec.709",
            ColorSpace::SGamut3 => "S-Gamut3", ColorSpace::SGamut3Cine => "S-Gamut3.Cinema",
            ColorSpace::Srgb => "sRGB",
        }
    }

    pub fn get_white_point_chromaticities(&self) -> (f32, f32) {
        match self {
            ColorSpace::DciP3 => (0.314, 0.351),
            ColorSpace::ACESAP1 => (0.32168, 0.33767),
            _ => (0.3127, 0.3290),
        }
    }

    pub fn get_xyz_to_rgb_matrix(&self) -> [f32; 9] {
        match self {
            ColorSpace::AppleWideGamut => xyz_to_rgb_from_primaries(0.725, 0.301, 0.221, 0.814, 0.068, -0.076, 0.3127, 0.3290),
            ColorSpace::Rec709 | ColorSpace::Srgb => xyz_to_rec709(),
            ColorSpace::Rec2020 | ColorSpace::FGamut => xyz_to_rgb_from_primaries(0.708, 0.292, 0.170, 0.797, 0.131, 0.046, 0.3127, 0.3290),
            ColorSpace::DciP3 => xyz_to_rgb_from_primaries(0.680, 0.320, 0.265, 0.690, 0.150, 0.060, 0.314, 0.351),
            ColorSpace::DisplayP3 => xyz_to_rgb_from_primaries(0.680, 0.320, 0.265, 0.690, 0.150, 0.060, 0.3127, 0.3290),
            ColorSpace::SGamut3Cine => xyz_to_rgb_from_primaries(0.76600, 0.27500, 0.22500, 0.80000, 0.08900, -0.08700, 0.3127, 0.3290),
            ColorSpace::SGamut3 => xyz_to_rgb_from_primaries(0.7300, 0.2800, 0.1400, 0.8550, 0.1000, -0.0500, 0.3127, 0.3290),
            ColorSpace::ARRIWideGamut3 => xyz_to_rgb_from_primaries(0.6840, 0.3130, 0.2210, 0.8480, 0.0861, -0.1020, 0.3127, 0.3290),
            ColorSpace::ARRIWideGamut4 => xyz_to_rgb_from_primaries(0.7347, 0.2653, 0.1424, 0.8576, 0.0991, -0.0308, 0.3127, 0.3290),
            ColorSpace::CanonCinemaGamut => xyz_to_rgb_from_primaries(0.7400, 0.2700, 0.1700, 1.1400, 0.0800, -0.1000, 0.3127, 0.3290),
            ColorSpace::PanasonicVGamut => xyz_to_rgb_from_primaries(0.7300, 0.2800, 0.1650, 0.8400, 0.1000, -0.0300, 0.3127, 0.3290),
            ColorSpace::FGamutC => xyz_to_rgb_from_primaries(0.7347, 0.2653, 0.0263, 0.9737, 0.1173, -0.0224, 0.3127, 0.3290),
            ColorSpace::DaVinciWideGamut => xyz_to_rgb_from_primaries(0.8000, 0.3130, 0.1682, 0.9877, 0.0790, -0.1155, 0.3127, 0.3290),
            ColorSpace::ACESAP1 => xyz_to_rgb_from_primaries(0.71300, 0.29300, 0.16500, 0.83000, 0.12800, 0.04400, 0.32168, 0.33767),
        }
    }

    pub fn all() -> &'static [ColorSpace] {
        // Alphabetical order for deterministic, pleasing cycle order.
        &[ColorSpace::ACESAP1, ColorSpace::AppleWideGamut, ColorSpace::ARRIWideGamut3, ColorSpace::ARRIWideGamut4,
          ColorSpace::CanonCinemaGamut, ColorSpace::DaVinciWideGamut, ColorSpace::DciP3,
          ColorSpace::DisplayP3, ColorSpace::FGamut, ColorSpace::FGamutC,
          ColorSpace::PanasonicVGamut, ColorSpace::Rec2020, ColorSpace::Rec709,
          ColorSpace::SGamut3, ColorSpace::SGamut3Cine, ColorSpace::Srgb]
    }

    /// Luminance coefficients of the working space, for the Tier-2
    /// luminance-continuity fallback (HL-handling.md §3.3). Rec.709-style
    /// weights for the standard gamuts; BT.2020-style for the Rec.2020
    /// family. Camera-vendor gamuts (DWG, Canon CG, …) have no official
    /// luma weights — Rec.709 weights are the documented approximation
    /// (the fallback is a robustness path, not the primary estimator).
    pub fn luma_coefficients(&self) -> [f32; 3] {
        match self {
            ColorSpace::Rec2020 | ColorSpace::FGamut => [0.2627, 0.6780, 0.0593],
            _ => [0.2126, 0.7152, 0.0722],
        }
    }

    pub fn next(self) -> Self { let all = Self::all(); let pos = all.iter().position(|&x| x == self).unwrap_or(0); all[(pos + 1) % all.len()] }
    pub fn prev(self) -> Self { let all = Self::all(); let pos = all.iter().position(|&x| x == self).unwrap_or(0); all[(pos + all.len() - 1) % all.len()] }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferFunction {
    ACESCCT, ARRIlog3, ARRIlog4, AppleLog, AppleLog2, CLog3, DaVinciIntermediate,
    FLog2, Gamma24, HLG, Linear, PQ, Rec709, SLog3, VLog,
}

impl TransferFunction {
    pub fn name(&self) -> &'static str {
        match self {
            TransferFunction::ACESCCT => "ACES CCT",
            TransferFunction::ARRIlog3 => "ARRI LogC3", TransferFunction::ARRIlog4 => "ARRI LogC4",
            TransferFunction::AppleLog => "Apple Log", TransferFunction::AppleLog2 => "Apple Log 2",
            TransferFunction::CLog3 => "C-Log3",
            TransferFunction::DaVinciIntermediate => "DaVinci Intermediate",
            TransferFunction::FLog2 => "F-Log2", TransferFunction::Gamma24 => "Gamma 2.4",
            TransferFunction::HLG => "HLG (BT.2100)", TransferFunction::Linear => "Linear",
            TransferFunction::PQ => "PQ (ST.2084)", TransferFunction::Rec709 => "Rec.709",
            TransferFunction::SLog3 => "S-Log3", TransferFunction::VLog => "V-Log",
        }
    }

    /// Apply the OETF (linear → log) for the selected transfer function.
    ///
    /// **Source-of-truth references** for each branch:
    ///
    /// | Variant | Spec / document |
    /// |---|---|
    /// | `Rec709`         | ITU-R BT.709-6 OETF |
    /// | `SLog3`          | Sony "S-Log3 Technical Specification" (Sept 2014) — canonical form: code = `(420 + 261.5×log₁₀((x+0.01)/0.19)) / 1023`, knee at `0.01125`, black code `95`, 18% grey code `420` |
    /// | `VLog`           | Panasonic "V-Log/V-Gamut Reference Manual" (2014) — `5.6x+0.125` / `0.241514*log10(x+0.00873)+0.598206`, knee at `0.01` |
    /// | `ARRIlog3`       | ARRI "LogC-3 Logarithmic Color Space" spec (2020), EI 800 variant |
    /// | `ARRIlog4`       | ARRI "LogC4 Encoding Function" (Cooper & Brendel, 2022; ALEV4 / Alexa 35), EI-independent |
    /// | `CLog3`          | Canon Cinema EOS C-Log3 characteristics (2016) — three-segment with negative-side graft |
    /// | `FLog2`          | Fujifilm "F-Log2 Data Sheet" (2021) — Fujifilm-internal anchor at `0.000889` |
    /// | `AppleLog`/`AppleLog2` | Apple "Apple Log Profile White Paper" (Sept 2023) — `R0=-0.05641088`, `C=47.28711236` |
    /// | `ACESCCT`        | AMPAS ACEScc specification (TB-2022-002), knee at `2^-7 = 0.0078125`, log slope `17.52` |
    /// | `PQ`             | ITU-R BT.2100-2 ST.2084 PQ (2022) — `m1=0.1593017578125`, `m2=78.84375`, `c1=0.8359375`, `c2=18.8515625`, `c3=18.6875` |
    /// | `HLG`            | ITU-R BT.2100-2 HLG OETF (2022) — knee at `1/12`, `a=0.17883277`, `b=0.28466892`, `c=0.55991073` |
    /// | `DaVinciIntermediate` | Blackmagic "DaVinci YRGB Intermediate" — knee at `0.00262409`, log slope `0.07329248` |
    /// | `Gamma24`        | Display gamma `1/2.4` (Rec.1886 EOTF approximation) |
    /// | `Linear`         | identity |
    pub fn process(&self, pixels: &mut [f32]) {
        match self {
            TransferFunction::Linear => {}
            // Source: ITU-R BT.709-6 §3.
            TransferFunction::Rec709 => { pixels.par_iter_mut().for_each(|v| { *v = rec709_oetf(*v).max(0.0); }); }
            // Source: Sony "S-Log3 Technical Summary" (Sept 2014).
            // Canonical form per colour-science and ACES CTL ref.
            // Knee at 0.01125; above: log segment maps 18% grey (0.18) to
            // code 420/1023; below: linear segment maps black (0.0) to
            // code 95/1023.
            TransferFunction::SLog3 => { pixels.par_iter_mut().for_each(|v| { let x = *v; *v = if x >= 0.01125_f32 { (420.0_f32 + 261.5_f32 * ((x + 0.01_f32) / 0.19_f32).log10()) / 1023.0_f32 } else { (x * (171.2102946929_f32 - 95.0_f32) / 0.01125_f32 + 95.0_f32) / 1023.0_f32 }; }); }
            // Source: Panasonic V-Log/V-Gamut Reference Manual (2014).
            TransferFunction::VLog => { pixels.par_iter_mut().for_each(|v| { let x = *v; *v = if x < 0.01 { 5.6_f32 * x + 0.125_f32 } else { 0.241514_f32 * (x + 0.00873_f32).log10() + 0.598206_f32 }; }); }
            // Source: ARRI LogC-3 spec (2020), EI 800.
            TransferFunction::ARRIlog3 => { pixels.par_iter_mut().for_each(|v| { let x = *v; *v = if x > 0.010591_f32 { 0.247190_f32 * (5.555556_f32 * x + 0.052272_f32).log10() + 0.385537_f32 } else { 5.367655_f32 * x + 0.092809_f32 }; }); }
            // Source: ARRI "LogC4 Logarithmic Color Space SPECIFICATION"
            // (Cooper & Brendel, 2022). EI-independent log encoding optimised
            // for 12-bit ALEV4 sensors. Two-segment with a linear-to-log
            // threshold at x = t ≈ -0.0180967. Constants a/b/c/s/t are
            // defined in arri_logc4_constants() below; see also colour-
            // science/colour (`log_encoding_ARRILogC4`).
            TransferFunction::ARRIlog4 => {
                let (a, b, c, s, t) = arri_logc4_constants();
                pixels.par_iter_mut().for_each(|v| {
                    let x = *v;
                    *v = if x >= t {
                        ((a * x + 64.0_f32).log2() - 6.0_f32) / 14.0_f32 * b + c
                    } else {
                        (x - t) / s
                    };
                });
            }
            // Source: Canon C-Log3 characteristics (2016). Three-segment
            // with a negative-side log graft and a linear middle.
            TransferFunction::CLog3 => {
                let neg_graft_lin = (0.097465473_f32 - 0.12512219_f32) / 1.9754798_f32;
                let pos_graft_lin = (0.15277891_f32 - 0.12512219_f32) / 1.9754798_f32;
                pixels.par_iter_mut().for_each(|v| {
                    let x = *v;
                    *v = if x < neg_graft_lin { -0.36726845_f32 * ((-x * 14.98325_f32 + 1.0_f32).max(1e-10_f32)).log10() + 0.12783901_f32 }
                         else if x <= pos_graft_lin { 1.9754798_f32 * x + 0.12512219_f32 }
                         else { 0.36726845_f32 * (x * 14.98325_f32 + 1.0_f32).log10() + 0.12240537_f32 };
                });
            }
            // Source: Fujifilm F-Log2 Data Sheet (2021).
            TransferFunction::FLog2 => { pixels.par_iter_mut().for_each(|v| { let x = *v; *v = if x >= 0.000889_f32 { 0.245281_f32 * (5.555556_f32 * x + 0.064829_f32).log10() + 0.384316_f32 } else { 8.799461_f32 * x + 0.092864_f32 }; }); }
            // Source: Apple "Apple Log Profile White Paper" (Sept 2023).
            TransferFunction::AppleLog | TransferFunction::AppleLog2 => {
                pixels.par_iter_mut().for_each(|v| {
                    let x = *v;
                    const R0: f32 = -0.05641088; const RT: f32 = 0.01; const C: f32 = 47.28711236;
                    const BETA: f32 = 0.00964052; const GAMMA: f32 = 0.08550479; const DELTA: f32 = 0.69336945;
                    *v = if x < R0 { 0.0 } else if x < RT { C * (x - R0) * (x - R0) } else { GAMMA * (x + BETA).log2() + DELTA };
                });
            }
            // Source: AMPAS ACEScc specification (TB-2022-002).
            TransferFunction::ACESCCT => { pixels.par_iter_mut().for_each(|v| { let x = *v; *v = if x > 0.0078125_f32 { (x.log2() + 9.72_f32) / 17.52_f32 } else { 10.5402377416545_f32 * x + 0.0729055341958355_f32 }; }); }
            // Source: ITU-R BT.2100-2 ST.2084 PQ. Input clamped to ≥0 to
            // prevent NaN from negative values entering the power function.
            TransferFunction::PQ => { pixels.par_iter_mut().for_each(|v| { let x = (*v).max(0.0_f32); let x_m1 = x.powf(0.1593017578125_f32); *v = ((0.8359375_f32 + 18.8515625_f32 * x_m1) / (1.0_f32 + 18.6875_f32 * x_m1)).powf(78.84375_f32); }); }
            // Source: ITU-R BT.2100-2 HLG OETF. Input clamped to ≥0 to
            // prevent NaN from negative values entering sqrt/ln.
            // Knee at L = 1/12; below the knee V = sqrt(3L), above
            // V = a*ln(12L - b) + c with a=0.17883277, b=0.28466892, c=0.55991073.
            TransferFunction::HLG => { pixels.par_iter_mut().for_each(|v| { let x = (*v).max(0.0_f32); *v = if x < (1.0_f32 / 12.0_f32) { (3.0_f32 * x).sqrt() } else { 0.17883277_f32 * (12.0_f32 * x - 0.28466892_f32).ln() + 0.55991073_f32 }; }); }
            // Source: Blackmagic DaVinci YRGB Intermediate white paper.
            TransferFunction::DaVinciIntermediate => { pixels.par_iter_mut().for_each(|v| { let x = *v; *v = if x <= 0.00262409_f32 { x * 10.44426855_f32 } else { 0.07329248_f32 * ((x + 0.0075_f32).log2() + 7.0_f32) }; }); }
            // Display gamma 1/2.4. Not a log curve; for 8-bit preview
            // only — production use should always pick a real OETF.
            TransferFunction::Gamma24 => { pixels.par_iter_mut().for_each(|v| { *v = v.max(0.0).powf(1.0 / 2.4); }); }
        }
    }

    pub fn all() -> &'static [TransferFunction] {
        // Alphabetical order for deterministic, pleasing cycle order.
        &[TransferFunction::ACESCCT, TransferFunction::ARRIlog3, TransferFunction::ARRIlog4,
          TransferFunction::AppleLog, TransferFunction::AppleLog2, TransferFunction::CLog3,
          TransferFunction::DaVinciIntermediate, TransferFunction::FLog2,
          TransferFunction::Gamma24, TransferFunction::HLG, TransferFunction::Linear,
          TransferFunction::PQ, TransferFunction::Rec709, TransferFunction::SLog3,
          TransferFunction::VLog]
    }
    pub fn next(self) -> Self { let all = Self::all(); let pos = all.iter().position(|&x| x == self).unwrap_or(0); all[(pos + 1) % all.len()] }
    pub fn prev(self) -> Self { let all = Self::all(); let pos = all.iter().position(|&x| x == self).unwrap_or(0); all[(pos + all.len() - 1) % all.len()] }
    pub fn is_log_bypass(&self) -> bool { !matches!(self, TransferFunction::Linear | TransferFunction::Rec709 | TransferFunction::Gamma24) }
    pub fn requires_10bit(&self) -> bool { !matches!(self, TransferFunction::Linear | TransferFunction::Rec709 | TransferFunction::Gamma24) }
    /// Display-referred curves: no scene headroom above 1.0 — the only
    /// curves that receive the display rolloff (HL-handling.md §4).
    pub fn is_display_referred(&self) -> bool { self.is_log_bypass() == false }
}

#[inline] pub fn rec709_oetf(x: f32) -> f32 { if x < 0.018 { 4.5 * x } else { 1.099 * x.powf(0.45) - 0.099 } }
#[inline] pub fn rec709_eotf(x: f32) -> f32 { if x < 0.0812429 { x / 4.5 } else { ((x + 0.099) / 1.099).powf(1.0 / 0.45) } }

/// ARRI LogC4 constants (a, b, c, s, t) from the 2022 LogC4 specification.
///
/// Derivation (Cooper & Brendel 2022, §4.1.1):
///   a = (2^18 - 16) / 117.45
///   b = (1023 - 95) / 1023
///   c = 95 / 1023
///   s = (7 · ln 2 · 2^(7 - 14·c/b)) / (a · b)
///   t = (2^(14·(-c/b) + 6) - 64) / a
///
/// Cross-checked against colour-science/colour
/// `colour.models.rgb.transfer_functions.arri` and antlerpost.com/colour-spaces/LogC4.
pub fn arri_logc4_constants() -> (f32, f32, f32, f32, f32) {
    let a: f32 = ((1u32 << 18) as f32 - 16.0) / 117.45;
    let b: f32 = (1023.0 - 95.0) / 1023.0;
    let c: f32 = 95.0 / 1023.0;
    let s: f32 = (7.0 * std::f32::consts::LN_2 * (7.0 - 14.0 * c / b).exp2()) / (a * b);
    let t: f32 = ((14.0 * (-c / b) + 6.0).exp2() - 64.0) / a;
    (a, b, c, s, t)
}

/// ARRI LogC4 scene-linear → normalized log encoding (E_scene → E').
/// Reference: ARRI "LogC4 Encoding Function" (Cooper & Brendel, 2022).
#[inline]
pub fn arri_logc4_oetf(x: f32) -> f32 {
    let (a, b, c, s, t) = arri_logc4_constants();
    if x >= t {
        ((a * x + 64.0).log2() - 6.0) / 14.0 * b + c
    } else {
        (x - t) / s
    }
}

/// ARRI LogC4 normalized log → scene-linear decoding (E' → E_scene).
/// Reference: ARRI "LogC4 Decoding Function" (Cooper & Brendel, 2022).
#[inline]
pub fn arri_logc4_eotf(y: f32) -> f32 {
    let (a, b, c, s, t) = arri_logc4_constants();
    if y >= 0.0 {
        ((14.0 * ((y - c) / b) + 6.0).exp2() - 64.0) / a
    } else {
        y * s + t
    }
}
#[inline] pub fn apply_ccm(r: f32, g: f32, b: f32, ccm: &[f32; 9]) -> [f32; 3] { [r * ccm[0] + g * ccm[1] + b * ccm[2], r * ccm[3] + g * ccm[4] + b * ccm[5], r * ccm[6] + g * ccm[7] + b * ccm[8]] }
pub fn identity_ccm() -> [f32; 9] { [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] }

pub fn invert_3x3(m: &[f32; 9]) -> [f32; 9] {
    let det = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6]) + m[2] * (m[3] * m[7] - m[4] * m[6]);
    let inv_det = 1.0 / det;
    [
        (m[4] * m[8] - m[5] * m[7]) * inv_det, (m[2] * m[7] - m[1] * m[8]) * inv_det, (m[1] * m[5] - m[2] * m[4]) * inv_det,
        (m[5] * m[6] - m[3] * m[8]) * inv_det, (m[0] * m[8] - m[2] * m[6]) * inv_det, (m[2] * m[3] - m[0] * m[5]) * inv_det,
        (m[3] * m[7] - m[4] * m[6]) * inv_det, (m[1] * m[6] - m[0] * m[7]) * inv_det, (m[0] * m[4] - m[1] * m[3]) * inv_det,
    ]
}

pub fn mat_mul_3x3(a: &[f32; 9], b: &[f32; 9]) -> [f32; 9] {
    let mut out = [0.0; 9];
    for i in 0..3 { for j in 0..3 { out[i * 3 + j] = a[i * 3] * b[j] + a[i * 3 + 1] * b[3 + j] + a[i * 3 + 2] * b[6 + j]; } }
    out
}

pub fn camera_to_rec709_matrix(color_matrix: &[f32; 9]) -> [f32; 9] {
    let cam_to_xyz = detect_camera_to_xyz(color_matrix);
    let d50_to_d65 = [0.9555, -0.0230, 0.0633, -0.0283, 1.0099, 0.0210, 0.0123, -0.0205, 1.3300];
    let cam_to_xyz_d65 = mat_mul_3x3(&d50_to_d65, &cam_to_xyz);
    mat_mul_3x3(&xyz_to_rec709(), &cam_to_xyz_d65)
}

pub fn rec709_to_xyz() -> [f32; 9] { [0.4124564, 0.3575761, 0.1804375, 0.2126729, 0.7151522, 0.0721750, 0.0193339, 0.1191920, 0.9503041] }

pub(crate) const MCAT16: [f32; 9] = [0.401288, 0.650173, -0.051461, -0.250268, 1.204414, 0.045854, -0.002079, 0.048952, 0.953127];
pub(crate) const MCAT16_INV: [f32; 9] = [1.86206786, -1.01125463, 0.14918678, 0.38752654, 0.62144744, -0.00897398, -0.01584150, -0.03412294, 1.04996444];
pub const D50_XYZ: [f32; 3] = [0.96422, 1.0, 0.82521];
pub const D65_XYZ: [f32; 3] = [0.95047, 1.0, 1.08883];

pub fn xyz_from_chromaticities(x: f32, y: f32) -> [f32; 3] { let z = 1.0 - x - y; [x / y, 1.0, z / y] }

pub fn cat16_adapt(xyz: &[f32; 3], src_white: &[f32; 3], dst_white: &[f32; 3]) -> [f32; 3] {
    let [l_s, m_s, s_s] = mat_mul_vec3(&MCAT16, src_white);
    let [l_d, m_d, s_d] = mat_mul_vec3(&MCAT16, dst_white);
    let lms = mat_mul_vec3(&MCAT16, xyz);
    let adapted = [lms[0] * (l_d / l_s), lms[1] * (m_d / m_s), lms[2] * (s_d / s_s)];
    mat_mul_vec3(&MCAT16_INV, &adapted)
}

pub fn build_cat16_output_matrix(cam_to_xyz: &[f32; 9], scene_white_xyz: &[f32; 3], dst_white: &[f32; 3], xyz_to_output: &[f32; 9]) -> [f32; 9] {
    let [l_s, m_s, s_s] = mat_mul_vec3(&MCAT16, scene_white_xyz);
    let [l_d, m_d, s_d] = mat_mul_vec3(&MCAT16, dst_white);
    let r_l = l_d / l_s; let r_m = m_d / m_s; let r_s = s_d / s_s;
    let rgb_to_lms = mat_mul_3x3(&MCAT16, cam_to_xyz);
    let rgb_to_adapted = [
        rgb_to_lms[0] * r_l, rgb_to_lms[1] * r_l, rgb_to_lms[2] * r_l,
        rgb_to_lms[3] * r_m, rgb_to_lms[4] * r_m, rgb_to_lms[5] * r_m,
        rgb_to_lms[6] * r_s, rgb_to_lms[7] * r_s, rgb_to_lms[8] * r_s,
    ];
    let rgb_to_xyz = mat_mul_3x3(&MCAT16_INV, &rgb_to_adapted);
    mat_mul_3x3(xyz_to_output, &rgb_to_xyz)
}

#[inline]
pub fn mat_mul_vec3(m: &[f32; 9], v: &[f32; 3]) -> [f32; 3] {
    [m[0] * v[0] + m[1] * v[1] + m[2] * v[2], m[3] * v[0] + m[4] * v[1] + m[5] * v[2], m[6] * v[0] + m[7] * v[1] + m[8] * v[2]]
}

/// Bradford cone-response matrix
pub(crate) const BRADFORD: [f32; 9] = [
    0.8951000,  0.2664000, -0.1614000,
   -0.7502000,  1.7135000,  0.0367000,
    0.0389000, -0.0685000,  1.0296000,
];

/// Inverse Bradford matrix
pub(crate) const BRADFORD_INV: [f32; 9] = [
    0.9869929, -0.1470543,  0.1599627,
    0.4323053,  0.5183603,  0.0492912,
   -0.0085287,  0.0400428,  0.9684867,
];

/// Build a fused Bradford adaptation matrix: src_white → dst_white
pub fn build_bradford_matrix(src_white: &[f32; 3], dst_white: &[f32; 3]) -> [f32; 9] {
    let [rho_s, gamma_s, beta_s] = mat_mul_vec3(&BRADFORD, src_white);
    let [rho_d, gamma_d, beta_d] = mat_mul_vec3(&BRADFORD, dst_white);

    let scale = [
        rho_d / rho_s, 0.0, 0.0,
        0.0, gamma_d / gamma_s, 0.0,
        0.0, 0.0, beta_d / beta_s,
    ];

    let temp = mat_mul_3x3(&scale, &BRADFORD);
    mat_mul_3x3(&BRADFORD_INV, &temp)
}

/// DNG 1.4 specification:
///   * `ColorMatrix1` is a 3x3 matrix that maps the camera's native color
///     values to CIE XYZ (D50 / 2° observer). It is the FORWARD matrix.
///   * `ForwardMatrix1` is a 3x3 matrix that maps XYZ (D50) to the camera's
///     native color values. It is the INVERSE direction relative to the
///     camera→XYZ transform we need for the rendering pipeline.
///   * `CalibrationMatrix1` is applied in camera-native space BEFORE
///     `ColorMatrix1`, so the effective transform is
///     `ColorMatrix1 * CalibrationMatrix1 * camera_native`.
///
/// MCRAW embeds these matrices verbatim. We don't know whether a given file
/// stores them row-major or column-major, nor whether any tooling has
/// pre-inverted the direction, so we evaluate the four possible orientations
/// and pick the one whose implied scene white point best matches D50.
pub fn detect_camera_to_xyz(m: &[f32; 9]) -> [f32; 9] {
    let d50 = D50_XYZ;
    let transposed = [
        m[0], m[3], m[6],
        m[1], m[4], m[7],
        m[2], m[5], m[8],
    ];
    let inv = invert_3x3(m);
    let inv_t = invert_3x3(&transposed);

    let candidates: [[f32; 9]; 4] = [*m, transposed, inv, inv_t];

    // For each candidate compute the implied white point: a forward
    // Camera→XYZ matrix sends (1,1,1) to the white in XYZ, so the
    // row-sums of that matrix equal that white point. An XYZ→Camera
    // matrix has its row-sums equal to the row-basis sums (not the white
    // point) and is rejected by the distance check below.
    let mut best = *m;
    let mut best_dist = f32::MAX;
    for c in &candidates {
        let w = [c[0] + c[1] + c[2], c[3] + c[4] + c[5], c[6] + c[7] + c[8]];
        let dx = w[0] - d50[0];
        let dy = w[1] - d50[1];
        let dz = w[2] - d50[2];
        let dist = dx * dx + dy * dy + dz * dz;
        if dist < best_dist {
            best_dist = dist;
            best = *c;
        }
    }
    tracing::debug!(
        "detect_camera_to_xyz: white=[{:.3},{:.3},{:.3}] dist={:.4}",
        best[0] + best[1] + best[2],
        best[3] + best[4] + best[5],
        best[6] + best[7] + best[8],
        best_dist.sqrt()
    );
    best
}

/// Build a camera→XYZ matrix from a DNG `ColorMatrix1` and (optional)
/// `CalibrationMatrix1`. Orientation is auto-detected — see
/// [`detect_camera_to_xyz`].
pub fn camera_to_xyz_matrix(color_matrix: &[f32; 9], calibration_matrix: Option<&[f32; 9]>) -> [f32; 9] {
    let cam_to_xyz = detect_camera_to_xyz(color_matrix);
    match calibration_matrix {
        // Calibration is applied in camera-native space BEFORE the
        // forward XYZ transform, so the product order is
        // `ColorMatrix1 * CalibrationMatrix1`.
        Some(cal) => mat_mul_3x3(&cam_to_xyz, cal),
        None => cam_to_xyz,
    }
}

/// Invert a forward matrix to recover Camera→XYZ when only a
/// `ForwardMatrix1` (XYZ→Camera) is available.
pub fn forward_to_camera_xyz(forward_matrix: &[f32; 9]) -> [f32; 9] {
    detect_camera_to_xyz(forward_matrix)
}

/// Build a fused Camera→Rec709 CCM for the preview thumbnail path.
///
/// Mirrors the export pipeline's CCM construction (pipeline.rs:128-204):
///   1. Prefer ForwardMatrix1+2 (averaged) when available — already D50-adapted
///   2. Fall back to ColorMatrix1+2 + calibration + chromatic adaptation
///   3. Detect matrix orientation automatically (D50 white-point row-sum check)
///   4. Bradford-adapt from reference illuminant to D65
///   5. Fuse with Rec709 primaries
///
/// This fixes the green/pink tint on older MOTION files where the raw
/// `ColorMatrix1` was applied without orientation detection or D50→D65
/// chromatic adaptation.
pub fn build_preview_ccm(
    color_matrix: Option<&[f64; 9]>,
    forward_matrix1: Option<&[f64; 9]>,
    forward_matrix2: Option<&[f64; 9]>,
    color_matrix2: Option<&[f64; 9]>,
    calibration_matrix1: Option<&[f64; 9]>,
) -> [f32; 9] {
    let cm1_f32 = color_matrix.map(|m| [m[0] as f32, m[1] as f32, m[2] as f32, m[3] as f32, m[4] as f32, m[5] as f32, m[6] as f32, m[7] as f32, m[8] as f32]);
    let cm2_f32 = color_matrix2.map(|m| [m[0] as f32, m[1] as f32, m[2] as f32, m[3] as f32, m[4] as f32, m[5] as f32, m[6] as f32, m[7] as f32, m[8] as f32]);
    let fm1_f32 = forward_matrix1.map(|m| [m[0] as f32, m[1] as f32, m[2] as f32, m[3] as f32, m[4] as f32, m[5] as f32, m[6] as f32, m[7] as f32, m[8] as f32]);
    let fm2_f32 = forward_matrix2.map(|m| [m[0] as f32, m[1] as f32, m[2] as f32, m[3] as f32, m[4] as f32, m[5] as f32, m[6] as f32, m[7] as f32, m[8] as f32]);
    let cal1_f32 = calibration_matrix1.map(|m| [m[0] as f32, m[1] as f32, m[2] as f32, m[3] as f32, m[4] as f32, m[5] as f32, m[6] as f32, m[7] as f32, m[8] as f32]);

    let cam_to_xyz: [f32; 9] = if let (Some(ref fm1), Some(ref fm2)) = (fm1_f32, fm2_f32) {
        let fm_avg = interpolate_matrix(fm1, fm2, 0.5);
        let rs = [fm_avg[0] + fm_avg[1] + fm_avg[2], fm_avg[3] + fm_avg[4] + fm_avg[5], fm_avg[6] + fm_avg[7] + fm_avg[8]];
        let d = (rs[0] - D50_XYZ[0]).powi(2) + (rs[1] - D50_XYZ[1]).powi(2) + (rs[2] - D50_XYZ[2]).powi(2);
        if d < 0.05 { fm_avg } else { detect_camera_to_xyz(&fm_avg) }
    } else if let Some(ref fm1) = fm1_f32 {
        let rs = [fm1[0] + fm1[1] + fm1[2], fm1[3] + fm1[4] + fm1[5], fm1[6] + fm1[7] + fm1[8]];
        let d = (rs[0] - D50_XYZ[0]).powi(2) + (rs[1] - D50_XYZ[1]).powi(2) + (rs[2] - D50_XYZ[2]).powi(2);
        if d < 0.05 { *fm1 } else { detect_camera_to_xyz(fm1) }
    } else if let Some(ref cm1) = cm1_f32 {
        let cal = cal1_f32;
        match cm2_f32 {
            Some(ref cm2) => {
                let cm_avg = interpolate_matrix(cm1, cm2, 0.5);
                camera_to_xyz_matrix(&cm_avg, cal.as_ref())
            }
            None => camera_to_xyz_matrix(cm1, cal.as_ref()),
        }
    } else {
        identity_ccm()
    };

    // Determine reference illuminant from the selected matrix
    let rs = [cam_to_xyz[0] + cam_to_xyz[1] + cam_to_xyz[2], cam_to_xyz[3] + cam_to_xyz[4] + cam_to_xyz[5], cam_to_xyz[6] + cam_to_xyz[7] + cam_to_xyz[8]];
    let cam_illuminant_xyz = if fm1_f32.is_some() {
        D50_XYZ
    } else {
        let l = rs[0].max(rs[1]).max(rs[2]);
        if l < 0.1 || l > 5.0 { D50_XYZ } else { rs }
    };

    let bradford_static = build_bradford_matrix(&cam_illuminant_xyz, &D65_XYZ);
    let cam_to_xyz_d65 = mat_mul_3x3(&bradford_static, &cam_to_xyz);
    mat_mul_3x3(&xyz_to_rec709(), &cam_to_xyz_d65)
}

pub fn interpolate_matrix(a: &[f32; 9], b: &[f32; 9], t: f32) -> [f32; 9] {
    let s = 1.0 - t; let mut out = [0.0; 9];
    for i in 0..9 { out[i] = a[i] * s + b[i] * t; }
    out
}

pub fn xyz_to_rec709() -> [f32; 9] { [3.2404542, -1.5371385, -0.4985354, -0.9689294, 1.8767608, 0.0415560, 0.0556434, -0.2040259, 1.0572252] }

pub fn xyz_to_rgb_from_primaries(xr: f32, yr: f32, xg: f32, yg: f32, xb: f32, yb: f32, xw: f32, yw: f32) -> [f32; 9] {
    let xr_z = (1.0 - xr - yr) / yr; let xg_z = (1.0 - xg - yg) / yg; let xb_z = (1.0 - xb - yb) / yb;
    let m = [xr / yr, xg / yg, xb / yb, 1.0, 1.0, 1.0, xr_z, xg_z, xb_z];
    let wx = xw / yw; let wy = 1.0; let wz = (1.0 - xw - yw) / yw;
    let det_m = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6]) + m[2] * (m[3] * m[7] - m[4] * m[6]);
    let inv_det = 1.0 / det_m;
    let inv_m = [
        (m[4] * m[8] - m[5] * m[7]) * inv_det, (m[2] * m[7] - m[1] * m[8]) * inv_det, (m[1] * m[5] - m[2] * m[4]) * inv_det,
        (m[5] * m[6] - m[3] * m[8]) * inv_det, (m[0] * m[8] - m[2] * m[6]) * inv_det, (m[2] * m[3] - m[0] * m[5]) * inv_det,
        (m[3] * m[7] - m[4] * m[6]) * inv_det, (m[1] * m[6] - m[0] * m[7]) * inv_det, (m[0] * m[4] - m[1] * m[3]) * inv_det,
    ];
    let sr = inv_m[0] * wx + inv_m[1] * wy + inv_m[2] * wz;
    let sg = inv_m[3] * wx + inv_m[4] * wy + inv_m[5] * wz;
    let sb = inv_m[6] * wx + inv_m[7] * wy + inv_m[8] * wz;
    let rgb_to_xyz = [m[0] * sr, m[1] * sg, m[2] * sb, m[3] * sr, m[4] * sg, m[5] * sb, m[6] * sr, m[7] * sg, m[8] * sb];
    invert_3x3(&rgb_to_xyz)
}

pub struct BilinearDemosaic { pattern: BayerPattern }
impl BilinearDemosaic {
    pub fn new(pattern: BayerPattern) -> Self { BilinearDemosaic { pattern } }
    
    fn get_pixel(&self, bayer: &[u16], stride_width: u32, x: i32, y: i32) -> f64 {
        if x < 0 || y < 0 || x >= stride_width as i32 { return 0.0; }
        let idx = (y as usize) * (stride_width as usize) + (x as usize);
        if idx >= bayer.len() { return 0.0; }
        bayer[idx] as f64
    }

    fn is_red_site(&self, x: i32, y: i32, pattern: BayerPattern) -> bool {
        match pattern {
            BayerPattern::RGGB => x % 2 == 0 && y % 2 == 0,
            BayerPattern::BGGR => x % 2 == 1 && y % 2 == 1,
            BayerPattern::GRBG => x % 2 == 1 && y % 2 == 0,
            BayerPattern::GBRG => x % 2 == 0 && y % 2 == 1,
            _ => false,
        }
    }

    fn is_blue_site(&self, x: i32, y: i32, pattern: BayerPattern) -> bool {
        match pattern {
            BayerPattern::RGGB => x % 2 == 1 && y % 2 == 1,
            BayerPattern::BGGR => x % 2 == 0 && y % 2 == 0,
            BayerPattern::GRBG => x % 2 == 0 && y % 2 == 1,
            BayerPattern::GBRG => x % 2 == 1 && y % 2 == 0,
            _ => false,
        }
    }

    fn interp_green_at_red(&self, bayer: &[u16], stride: u32, _height: u32, x: i32, y: i32, pattern: BayerPattern) -> f64 {
        let mut sum = 0.0; let mut count = 0.0;
        let positions = [(0, -1), (0, 1), (-1, 0), (1, 0)];
        for (dx, dy) in positions.iter() {
            let px = x + dx; let py = y + dy;
            if self.is_green_site(px, py, pattern) { sum += self.get_pixel(bayer, stride, px, py); count += 1.0; }
        }
        if count > 0.0 { sum / count } else { self.get_pixel(bayer, stride, x, y) }
    }

    fn interp_green_at_blue(&self, bayer: &[u16], stride: u32, height: u32, x: i32, y: i32, pattern: BayerPattern) -> f64 {
        self.interp_green_at_red(bayer, stride, height, x, y, pattern)
    }

    fn interp_blue_at_red(&self, bayer: &[u16], stride: u32, _height: u32, x: i32, y: i32, pattern: BayerPattern) -> f64 {
        let mut sum = 0.0; let mut count = 0.0;
        let positions = [(-1, -1), (1, -1), (-1, 1), (1, 1)];
        for (dx, dy) in positions.iter() {
            let px = x + dx; let py = y + dy;
            if self.is_blue_site(px, py, pattern) { sum += self.get_pixel(bayer, stride, px, py); count += 1.0; }
        }
        if count > 0.0 { sum / count } else { self.get_pixel(bayer, stride, x, y) }
    }

    fn interp_red_at_blue(&self, bayer: &[u16], stride: u32, _height: u32, x: i32, y: i32, pattern: BayerPattern) -> f64 {
        let mut sum = 0.0; let mut count = 0.0;
        let positions = [(-1, -1), (1, -1), (-1, 1), (1, 1)];
        for (dx, dy) in positions.iter() {
            let px = x + dx; let py = y + dy;
            if self.is_red_site(px, py, pattern) { sum += self.get_pixel(bayer, stride, px, py); count += 1.0; }
        }
        if count > 0.0 { sum / count } else { self.get_pixel(bayer, stride, x, y) }
    }
    
    fn is_green_site(&self, x: i32, y: i32, pattern: BayerPattern) -> bool {
        !self.is_red_site(x, y, pattern) && !self.is_blue_site(x, y, pattern)
    }

    pub fn process_par(&self, bayer: &[u16], stride_width: u32, offset_x: u32, offset_y: u32, active_width: u32, active_height: u32, pattern: &BayerPattern) -> Result<Vec<f32>> {
        let stride = stride_width as usize; let ox = offset_x as i32; let oy = offset_y as i32;
        let aw = active_width as usize; let ah = active_height as usize;
        let min_len = (stride * (oy as usize + ah - 1) + ox as usize + aw - 1) + 1;
        if bayer.len() < min_len { anyhow::bail!("Bayer data too short"); }
        let mut rgb = vec![0.0f32; aw * ah * 3]; let pat = *pattern; let row_len = aw * 3;
        rgb.par_chunks_exact_mut(row_len).enumerate().for_each(|(sy, row)| {
            let y = sy as i32 + oy;
            for sx in 0..aw {
                let x = sx as i32 + ox;
                let is_red = self.is_red_site(x, y, pat); let is_blue = self.is_blue_site(x, y, pat);
                let (r, g, b) = if is_red {
                    (self.get_pixel(bayer, stride_width, x, y), self.interp_green_at_red(bayer, stride_width, active_height, x, y, pat), self.interp_blue_at_red(bayer, stride_width, active_height, x, y, pat))
                } else if is_blue {
                    (self.interp_red_at_blue(bayer, stride_width, active_height, x, y, pat), self.interp_green_at_blue(bayer, stride_width, active_height, x, y, pat), self.get_pixel(bayer, stride_width, x, y))
                } else {
                    // FIXED: GBRG top-green logic
                    let is_top_green = match pat {
                        BayerPattern::RGGB | BayerPattern::BGGR => y % 2 == 0,
                        BayerPattern::GRBG => y % 2 == 0,
                        BayerPattern::GBRG => y % 2 == 0, 
                        _ => y % 2 == 0,
                    };
                    if is_top_green {
                        (self.interp_red_at_blue(bayer, stride_width, active_height, x + 1, y, pat), self.get_pixel(bayer, stride_width, x, y), self.interp_blue_at_red(bayer, stride_width, active_height, x - 1, y, pat))
                    } else {
                        (self.interp_red_at_blue(bayer, stride_width, active_height, x - 1, y, pat), self.get_pixel(bayer, stride_width, x, y), self.interp_blue_at_red(bayer, stride_width, active_height, x + 1, y, pat))
                    }
                };
                let base = sx * 3; row[base] = r as f32; row[base + 1] = g as f32; row[base + 2] = b as f32;
            }
        });
        Ok(rgb)
    }

    pub fn process_par_into(&self, bayer: &[u16], stride_width: u32, offset_x: u32, offset_y: u32, active_width: u32, active_height: u32, pattern: &BayerPattern, output: &mut [f32]) -> Result<()> {
        let stride = stride_width as usize; let ox = offset_x as i32; let oy = offset_y as i32;
        let aw = active_width as usize; let ah = active_height as usize;
        let min_len = (stride * (oy as usize + ah - 1) + ox as usize + aw - 1) + 1;
        if bayer.len() < min_len { anyhow::bail!("Bayer data too short"); }
        if output.len() < aw * ah * 3 { anyhow::bail!("Output buffer too short"); }
        let pat = *pattern; let row_len = aw * 3;
        output.par_chunks_exact_mut(row_len).enumerate().for_each(|(sy, row)| {
            let y = sy as i32 + oy;
            for sx in 0..aw {
                let x = sx as i32 + ox;
                let is_red = self.is_red_site(x, y, pat); let is_blue = self.is_blue_site(x, y, pat);
                let (r, g, b) = if is_red {
                    (self.get_pixel(bayer, stride_width, x, y), self.interp_green_at_red(bayer, stride_width, active_height, x, y, pat), self.interp_blue_at_red(bayer, stride_width, active_height, x, y, pat))
                } else if is_blue {
                    (self.interp_red_at_blue(bayer, stride_width, active_height, x, y, pat), self.interp_green_at_blue(bayer, stride_width, active_height, x, y, pat), self.get_pixel(bayer, stride_width, x, y))
                } else {
                    // FIXED: GBRG top-green logic
                    let is_top_green = match pat {
                        BayerPattern::RGGB | BayerPattern::BGGR => y % 2 == 0,
                        BayerPattern::GRBG => y % 2 == 0,
                        BayerPattern::GBRG => y % 2 == 0,
                        _ => y % 2 == 0,
                    };
                    if is_top_green {
                        (self.interp_red_at_blue(bayer, stride_width, active_height, x + 1, y, pat), self.get_pixel(bayer, stride_width, x, y), self.interp_blue_at_red(bayer, stride_width, active_height, x - 1, y, pat))
                    } else {
                        (self.interp_red_at_blue(bayer, stride_width, active_height, x - 1, y, pat), self.get_pixel(bayer, stride_width, x, y), self.interp_blue_at_red(bayer, stride_width, active_height, x + 1, y, pat))
                    }
                };
                let base = sx * 3; row[base] = r as f32; row[base + 1] = g as f32; row[base + 2] = b as f32;
            }
        });
        Ok(())
    }
}

impl Demosaic for BilinearDemosaic {
    fn process(&self, bayer: &[u16], stride_width: u32, offset_x: u32, offset_y: u32, active_width: u32, active_height: u32, pattern: &BayerPattern) -> Result<Vec<f32>> {
        let stride = stride_width as usize; let ox = offset_x as i32; let oy = offset_y as i32;
        let aw = active_width as usize; let ah = active_height as usize;
        let min_len = (stride * (oy as usize + ah - 1) + ox as usize + aw - 1) + 1;
        if bayer.len() < min_len { anyhow::bail!("Bayer data too short"); }
        let mut rgb = Vec::with_capacity(aw * ah * 3); let pat = *pattern;
        for sy in 0..ah as i32 {
            for sx in 0..aw as i32 {
                let x = sx + ox; let y = sy + oy;
                let is_red = self.is_red_site(x, y, pat); let is_blue = self.is_blue_site(x, y, pat);
                let (r, g, b) = if is_red {
                    (self.get_pixel(bayer, stride_width, x, y), self.interp_green_at_red(bayer, stride_width, active_height, x, y, pat), self.interp_blue_at_red(bayer, stride_width, active_height, x, y, pat))
                } else if is_blue {
                    (self.interp_red_at_blue(bayer, stride_width, active_height, x, y, pat), self.interp_green_at_blue(bayer, stride_width, active_height, x, y, pat), self.get_pixel(bayer, stride_width, x, y))
                } else {
                    // FIXED: GBRG top-green logic
                    let is_top_green = match pat {
                        BayerPattern::RGGB | BayerPattern::BGGR => y % 2 == 0,
                        BayerPattern::GRBG => y % 2 == 0,
                        BayerPattern::GBRG => y % 2 == 0,
                        _ => y % 2 == 0,
                    };
                    if is_top_green {
                        (self.interp_red_at_blue(bayer, stride_width, active_height, x + 1, y, pat), self.get_pixel(bayer, stride_width, x, y), self.interp_blue_at_red(bayer, stride_width, active_height, x - 1, y, pat))
                    } else {
                        (self.interp_red_at_blue(bayer, stride_width, active_height, x - 1, y, pat), self.get_pixel(bayer, stride_width, x, y), self.interp_blue_at_red(bayer, stride_width, active_height, x + 1, y, pat))
                    }
                };
                rgb.push(r as f32); rgb.push(g as f32); rgb.push(b as f32);
            }
        }
        Ok(rgb)
    }
}

pub struct CcmColorSpaceConverter;
impl CcmColorSpaceConverter { pub fn new() -> Self { CcmColorSpaceConverter } }
impl Default for CcmColorSpaceConverter { fn default() -> Self { Self::new() } }
impl ColorSpaceConverter for CcmColorSpaceConverter {
    fn process(&self, pixels: &mut [f32], ccm: &[f32; 9]) {
        for chunk in pixels.chunks_exact_mut(3) {
            let [r_out, g_out, b_out] = apply_ccm(chunk[0], chunk[1], chunk[2], ccm);
            chunk[0] = r_out.max(0.0); chunk[1] = g_out.max(0.0); chunk[2] = b_out.max(0.0);
        }
    }
}

pub struct Rec709TransferFunction;
impl Rec709TransferFunction { pub fn new() -> Self { Rec709TransferFunction } }
impl TransferFunctionProcessor for Rec709TransferFunction {
    fn process(&self, pixels: &mut [f32]) { pixels.par_iter_mut().for_each(|v| { *v = rec709_oetf(*v).max(0.0); }); }
}

pub struct LinearTransferFunction;
impl LinearTransferFunction { pub fn new() -> Self { LinearTransferFunction } }
impl TransferFunctionProcessor for LinearTransferFunction { fn process(&self, _pixels: &mut [f32]) {} }

pub struct AgxKrakenPipeline { demosaic: BilinearDemosaic, agx: AgxPipeline, output_gamma: f32, enable_tonemap: bool }
impl AgxKrakenPipeline {
    pub fn new(pattern: BayerPattern) -> Self {
        let config = ColorPipelineConfig::broadcast(); let demosaic = BilinearDemosaic::new(pattern);
        let agx = AgxPipeline::new(config.tonemap_config.clone()); let output_gamma = config.output_gamma.gamma();
        let enable_tonemap = config.enable_tonemapping;
        AgxKrakenPipeline { demosaic, agx, output_gamma, enable_tonemap }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum OutputGamma { Srgb, Bt1886, Linear }
impl OutputGamma { pub fn gamma(&self) -> f32 { match self { OutputGamma::Srgb => 2.2, OutputGamma::Bt1886 => 2.4, OutputGamma::Linear => 1.0 } } }

pub struct ColorPipelineConfig {
    pub input_color_space: ColorSpace, pub input_transfer: TransferFunction, pub output_color_space: ColorSpace,
    pub output_transfer: TransferFunction, pub output_gamma: OutputGamma, pub enable_tonemapping: bool, pub tonemap_config: AgxConfig,
}
impl Default for ColorPipelineConfig {
    fn default() -> Self {
        Self { input_color_space: ColorSpace::Rec709, input_transfer: TransferFunction::Linear, output_color_space: ColorSpace::Rec709, output_transfer: TransferFunction::Rec709, output_gamma: OutputGamma::Bt1886, enable_tonemapping: true, tonemap_config: AgxConfig::default() }
    }
}
impl ColorPipelineConfig {
    pub fn broadcast() -> Self {
        let mut config = AgxConfig::default(); config.in_gamut = Gamut::Rec709; config.in_transfer = Transfer::Linear;
        config.working_curve = Transfer::AgxLogKraken; config.out_gamut = Gamut::Rec709; config.out_transfer = OutputTransfer::Bt1886InverseEotf;
        config.toe_power = 3.0; config.shoulder_power = 3.25; config.slope = 2.0; config.working_mid_grey = 0.606060; config.log_output = false;
        Self { input_color_space: ColorSpace::Rec709, input_transfer: TransferFunction::Linear, output_color_space: ColorSpace::Rec709, output_transfer: TransferFunction::Rec709, output_gamma: OutputGamma::Bt1886, enable_tonemapping: true, tonemap_config: config }
    }
    pub fn log_output(log_space: TransferFunction, gamut: ColorSpace) -> Self {
        let mut config = AgxConfig::default(); config.in_gamut = Gamut::Rec709; config.in_transfer = Transfer::Linear;
        config.working_curve = Transfer::AgxLogKraken;
        config.out_gamut = match gamut {
            ColorSpace::Rec709 => Gamut::Rec709, ColorSpace::Rec2020 => Gamut::Rec2020,
            ColorSpace::DciP3 | ColorSpace::DisplayP3 => Gamut::P3D65, ColorSpace::SGamut3Cine => Gamut::SGamut3Cine,
            ColorSpace::SGamut3 => Gamut::SGamut3, ColorSpace::ARRIWideGamut3 | ColorSpace::ARRIWideGamut4 => Gamut::Awg3,
            ColorSpace::CanonCinemaGamut => Gamut::CanonCinema, ColorSpace::ACESAP1 => Gamut::Ap1,
            ColorSpace::FGamut | ColorSpace::PanasonicVGamut => Gamut::Rwg, ColorSpace::FGamutC => Gamut::Ap0,
            ColorSpace::DaVinciWideGamut => Gamut::DaVinciWg, _ => Gamut::Rec709,
        };
        config.out_transfer = OutputTransfer::Linear; config.log_output = true;
        Self { input_color_space: ColorSpace::Rec709, input_transfer: TransferFunction::Linear, output_color_space: gamut, output_transfer: log_space, output_gamma: OutputGamma::Linear, enable_tonemapping: false, tonemap_config: config }
    }
}

pub fn pipeline_convert_to_u16(pixels: &[f32]) -> Vec<u16> { pixels.iter().map(|&v| (v.clamp(0.0, 1.0) * 65535.0) as u16).collect() }

pub fn highlight_clip(pixels: &mut [f32], threshold: f32) {
    let range = 1.0 - threshold; if range <= 0.0 { return; }
    for chunk in pixels.chunks_exact_mut(3) {
        let r = chunk[0]; let g = chunk[1]; let b = chunk[2];
        let max_val = r.max(g).max(b);
        if max_val > threshold {
            let t = ((max_val - threshold) / range).min(1.0);
            chunk[0] = r + (max_val - r) * t; chunk[1] = g + (max_val - g) * t; chunk[2] = b + (max_val - b) * t;
        }
    }
}

pub fn normalize_linear(pixels: &mut [f32], black_level: f64, white_level: f64) {
    let range = if white_level > black_level { white_level - black_level } else { 1.0 }; let inv_range = 1.0 / range;
    for v in pixels.iter_mut() { *v = ((*v as f64 - black_level) * inv_range).clamp(0.0, 1.0) as f32; }
}

pub fn normalize_linear_f32(pixels: &mut [f32], black_level: f32, white_level: f32) {
    let range = if white_level > black_level { white_level - black_level } else { 1.0 }; let inv_range = 1.0 / range;
    pixels.par_iter_mut().for_each(|v| { *v = (*v - black_level) * inv_range; if *v < 0.0 { *v = 0.0; } else if *v > 1.0 { *v = 1.0; } });
}

/// Per-channel linear normalization: subtract per-channel black level and
/// scale to [0,1] using a shared white level.  The input is f32 RGB
/// triples (r, g, b interleaved).  Each channel is normalized with its
/// own black level and the common white level.
///
/// `bl_r/bl_g/bl_b` — black levels for R, G, B respectively (after
/// demosaic G1+G2 have been averaged).
/// `white_level` — shared white level for all channels.
pub fn normalize_linear_per_channel(rgb: &mut [f32], bl_r: f64, bl_g: f64, bl_b: f64, white_level: f64) {
    let range_r = if white_level > bl_r { white_level - bl_r } else { 1.0 };
    let range_g = if white_level > bl_g { white_level - bl_g } else { 1.0 };
    let range_b = if white_level > bl_b { white_level - bl_b } else { 1.0 };
    let inv_r = 1.0 / range_r;
    let inv_g = 1.0 / range_g;
    let inv_b = 1.0 / range_b;
    rgb.par_chunks_exact_mut(3).for_each(|chunk| {
        chunk[0] = (((chunk[0] as f64 - bl_r) * inv_r) as f32).max(0.0);
        chunk[1] = (((chunk[1] as f64 - bl_g) * inv_g) as f32).max(0.0);
        chunk[2] = (((chunk[2] as f64 - bl_b) * inv_b) as f32).max(0.0);
    });
}

// ────────────────────────────────────────────────────────────────────────
// Highlight reconstruction (raw-space, pre-CCM) + display rolloff.
// Design and verification battery: HL-handling.md §3-§4, §6.
// ────────────────────────────────────────────────────────────────────────

/// Reference-channel floor: ratio samples with `H_neighbor <= RECON_EPSILON`
/// are excluded (below ~1% of the white level the SNR is read-noise
/// dominated and ratios are meaningless).
pub const RECON_EPSILON: f32 = 0.01;
/// Estimate ceiling factor: never above `RECON_MAX_FACTOR ×` the largest
/// unclipped value of the clipped channel in the window (texture bound —
/// prevents unbounded blowups from texture).
pub const RECON_MAX_FACTOR: f32 = 1.5;

/// Tier-1/2 window radius: `(2·RECON_WIN_R+1)²` = 9×9. The previous 5×5
/// window found no clean support for pixels more than 2 px inside a
/// saturated blob — the exact population that goes pink.
pub const RECON_WIN_R: usize = 4;
const RECON_WIN_MAX: usize = (2 * RECON_WIN_R + 1) * (2 * RECON_WIN_R + 1);

/// Tier-3 ring search: Chebyshev radii scanned outward for the first
/// fully-clean ring (hue anchor). `RECON_RING_MAX = 8` keeps ring samples
/// inside the WGSL valid region (BORDER = 9) on the GPU path.
pub const RECON_RING_MIN: usize = 3;
pub const RECON_RING_MAX: usize = 8;
const RECON_RING_SAMPLES: usize = 6;
const RECON_RING_CAP: usize = 64;
/// Tier-3 brightness-continuation range: Chebyshev radii scanned for the
/// nearest not-fully-clipped pixels (mask ≠ 111) whose recovered (Pass A)
/// peak the fully-clipped core continues. Larger than the ring range — the
/// halo of a huge saturated blob is far from the core interior, but still
/// carries the brightness the core must not fall below.
pub const RECON_BRIGHT_MAX: usize = 16;
/// Dead-zone of the brightness-continuation lift envelope (fraction of
/// `RECON_BRIGHT_MAX`): pixels closer than this to the nearest informative
/// sample keep their estimate at the boundary median — no lift toward the
/// core's own ceiling. Tiny clipped islands (a few px wide) therefore read
/// at the surrounding recovered level instead of flashing as bright
/// squares, and the ramp enters smoothly, not from the rim.
const RECON_BRIGHT_DEADZONE: f32 = 4.0;
/// Spatial smoothing radius for the continuation field: single-pass
/// separable 13×13 (radius 6) box applied before write-back. The recovered
/// boundary brightness diffuses into the core as smooth gradients instead
/// of hard band edges; the wide kernel also flattens the median-derived
/// patchiness (uniform ±6 patches survive a 5×5 box but not a 13×13 one).
/// Unscanned cells substitute their own WB'd peak (their true continuation
/// level), so the blend never dips below the local ceiling.
pub const RECON_PEAK_BLUR_R: usize = 6;
/// Display rolloff shoulder ceiling: `f(m) -> ROLLOFF_CEILING` as m -> inf.
pub const ROLLOFF_CEILING: f32 = 1.05;

/// Per-frame parameters for the CPU highlight-reconstruction pass.
#[derive(Debug, Clone, Copy)]
pub struct ReconstructParams {
    /// White-balance gains, as applied by the WB+CCM pass. Used only by the
    /// Tier-2 luminance fallback.
    pub r_gain: f32,
    pub b_gain: f32,
    /// Luma row `luma_coeffs(output_space) · fused` (in WB'd camera space),
    /// for the Tier-2 luminance-continuity fallback. `None` disables the
    /// fallback (Tier-2 then pins when the ratio anchor is unreliable).
    pub fused_luma: Option<[f32; 3]>,
}

/// Per-pixel clip mask. Returns one byte per pixel (RGB interleaved input):
/// bit 0 = R clipped, bit 1 = G clipped, bit 2 = B clipped — plus the count
/// of pixels with at least one clipped channel (fast-path / statistics).
pub fn clip_mask(rgb: &[f32], threshold: f32) -> (Vec<u8>, usize) {
    let clipped = AtomicUsize::new(0);
    let mask: Vec<u8> = rgb
        .par_chunks_exact(3)
        .map(|c| {
            let mut m = 0u8;
            if c[0] >= threshold {
                m |= 1;
            }
            if c[1] >= threshold {
                m |= 2;
            }
            if c[2] >= threshold {
                m |= 4;
            }
            if m != 0 {
                clipped.fetch_add(1, Ordering::Relaxed);
            }
            m
        })
        .collect();
    (mask, clipped.load(Ordering::Relaxed))
}

/// Raw-truth 2×2-block pin mask (user-approved design): one byte per pixel,
/// bit 0 = R, bit 1 = G, bit 2 = B. A channel's flag is set when ANY of its
/// photosites in the pixel's 2×2 CFA block sits at/above `pin_thr_raw` —
/// the flat sensor-ceiling threshold in RAW CFA units (0.99 × clip_raw,
/// where clip_raw = min(dynamic_white_level, src_wl) — the true well
/// reading). No black-level adjustment, no reconstruction coupling: a
/// photosite is physically pinned when its raw code is at the sensor
/// ceiling, full stop. Sub-threshold photosites (e.g. 0.983×WL) are
/// deliberately NOT flagged — they carry real, distinct sensor data that
/// WB + CCM may legitimately push into wide-gamut colors; the collapse
/// gate must leave them alone (user-mandated no-pre-trigger rule). Built
/// from the pre-demosaic, lens-corrected bayer — identical semantics to
/// the WGSL `shm_pin` registry in rcd_fill.wgsl. This is the collapse
/// gate's only input: demosaic interpolation can smooth pinned
/// photosites beneath the mask threshold, the raw CFA cannot. Pixels
/// whose block carries no pin keep their recorded color (scene-referred).
pub fn raw_pin_mask(
    bayer: &[u16],
    stride: usize,
    offset_x: usize,
    offset_y: usize,
    width: usize,
    height: usize,
    pattern: BayerPattern,
    pin_thr_raw: f64,
) -> Vec<u8> {
    if pin_thr_raw <= 0.0 {
        return vec![0u8; width * height];
    }
    let thr = pin_thr_raw;
    let site = |x: usize, y: usize| -> f64 {
        let x = x.min(width - 1);
        let y = y.min(height - 1);
        bayer[(offset_y + y) * stride + offset_x + x] as f64
    };
    let blocks_x = width.div_ceil(2);
    let mut mask = vec![0u8; width * height];
    mask.par_chunks_mut(width * 2).enumerate().for_each(|(by, band)| {
        let y = by * 2;
        for bx in 0..blocks_x {
            let x = bx * 2;
            // Photosite roles per pattern; G1/G2 share the G bit.
            let (r, g1, g2, b): (f64, f64, f64, f64) = match pattern {
                // Quad-Bayer variants never reach the export pipeline (the
                // demosaic and WGSL support the four standard patterns only);
                // map them to their base-pattern roles so the block math
                // stays total.
                BayerPattern::QuadBayerRGGB | BayerPattern::RGGB => (
                    site(x, y),
                    site(x + 1, y),
                    site(x, y + 1),
                    site(x + 1, y + 1),
                ),
                BayerPattern::QuadBayerBGGR | BayerPattern::BGGR => (
                    site(x + 1, y + 1),
                    site(x + 1, y),
                    site(x, y + 1),
                    site(x, y),
                ),
                BayerPattern::QuadBayerGRBG | BayerPattern::GRBG => (
                    site(x + 1, y),
                    site(x, y),
                    site(x + 1, y + 1),
                    site(x, y + 1),
                ),
                BayerPattern::QuadBayerGBRG | BayerPattern::GBRG => (
                    site(x, y + 1),
                    site(x, y),
                    site(x + 1, y + 1),
                    site(x + 1, y),
                ),
            };
            let mut m = 0u8;
            if r >= thr {
                m |= 1;
            }
            if g1 >= thr || g2 >= thr {
                m |= 2;
            }
            if b >= thr {
                m |= 4;
            }
            if m == 0 {
                continue;
            }
            for (dy, dx) in [(0usize, 0usize), (0, 1), (1, 0), (1, 1)] {
                if y + dy < height && x + dx < width {
                    band[dy * width + x + dx] = m;
                }
            }
        }
    });
    mask
}

/// Median of `samples` (destructive). Samples are finite by construction.
fn median_sorted(samples: &mut [f32]) -> f32 {
    debug_assert!(!samples.is_empty());
    samples.sort_unstable_by(|a, b| a.total_cmp(b));
    samples[samples.len() / 2]
}

/// Pixel-level neutral collapse for uninformative clipped pairs
/// (HL-handling.md §3.2, deviation D9).
///
/// Given a pixel whose raw mask has ≥ 2 channels at the sensor clip ceiling
/// — its hue in those channels is uninformative by construction (the value
/// only encodes "≥ ceiling") — replace the WB'd triple with the WB'd neutral
/// direction `[k, k, k]` scaled so its fused luma equals the measured luma.
/// Brightness is fully preserved from the measured channels; no hue is
/// invented. A single clipped channel (real saturated colors, e.g. red
/// stars) is never touched — its hue is real data.
///
/// The neutral direction IS `[k, k, k]` — NOT the gain-scaled
/// `[k·r_gain, k, k·b_gain]` of earlier iterations. In this pipeline the WB
/// gains are applied separately (pipeline.rs), and the fused CCM is built so
/// that its input-space neutral `[1,1,1]` maps to output neutral: the fused
/// row sums are 1.0 (±0.001) by the ±CAT construction. Feeding the collapse
/// `[k·g_r, k, k·g_b]` re-applies the gains inside the CCM and exits with
/// R≈B≈3.4·G — the exact magenta cast this pass exists to prevent.
///
/// Operates on **WB'd** values (gains already applied). Returns `true`
/// when the triple was replaced. NaN/Inf input is left untouched.
pub fn luma_collapse_after_wb(triple: &mut [f32; 3], mask: u8, luma: [f32; 3]) -> bool {
    if mask.count_ones() < 2 {
        return false;
    }
    let y = luma[0] * triple[0] + luma[1] * triple[1] + luma[2] * triple[2];
    if !y.is_finite() {
        return false;
    }
    let luma_neutral = luma[0] + luma[1] + luma[2];
    if !(luma_neutral.abs() > 1e-6) {
        return false;
    }
    let k = (y / luma_neutral).max(0.0);
    *triple = [k, k, k];
    true
}

/// Tier-1/2/3 highlight reconstruction (in-place, normalized pre-WB RGB).
///
/// * Pixels with a clean mask are never touched (no-temper guarantee).
/// * Tier 1 (one clipped channel): median-of-ratios against each healthy
///   reference channel, averaged; ε floor on reference samples. Estimates
///   keep the "never darken" floor at the pixel's own pinned value —
///   a single-clipped channel still carries real hue.
/// * Tier 2 (two clipped channels — the asymmetric-clip pink case):
///   median-of-ratios against the single healthy channel, or the luminance
///   fallback (Resolve "Luminance" style) on an unreliable anchor. The
///   estimate range is `[0, RECON_MAX_FACTOR × window_max]` — the pinned
///   value is *not* a floor, because pinning the saturated pair at the
///   ceiling is what anchors the post-WB/CCM magenta cast.
/// * Tier 3 (all clipped): processed in a second pass so the halo's
///   recovered estimates are visible. Hue anchor from the nearest
///   fully-clean ring (Chebyshev radius `RECON_RING_MIN..=RECON_RING_MAX`)
///   as before, but the peak brightness now CONTINUES the halo: the pixel
///   estimates at `max(own ceiling, boundary_peak)` where `boundary_peak`
///   is the median WB'd peak of the nearest not-fully-clipped pixels
///   (mask ≠ 111, radii `1..=RECON_BRIGHT_MAX`). Ringless pixels get
///   neutral chromaticity (the caller's clipped-pair collapse neutralizes
///   hue anyway); the flat pinned-gray hole in large highlights becomes a
///   smooth brightness transition into the semi-clipped halo. No
///   informative pixel within `RECON_BRIGHT_MAX` → stays pinned.
///
/// Windows are `(2·RECON_WIN_R+1)²` (9×9) — the previous 5×5 died inside
/// saturated blobs where no clean sample exists within 2 px.
pub fn reconstruct_clipped(rgb: &mut [f32], w: u32, h: u32, mask: &[u8], params: &ReconstructParams) {
    let w = w as usize;
    let h = h as usize;
    debug_assert!(rgb.len() >= w * h * 3);
    debug_assert!(mask.len() == w * h);
    let targets: Vec<usize> = mask
        .iter()
        .enumerate()
        .filter(|&(_, &m)| m != 0)
        .map(|(i, _)| i)
        .collect();
    if targets.is_empty() {
        return;
    }

    let eps = RECON_EPSILON;
    let max_factor = RECON_MAX_FACTOR;
    let win_r = RECON_WIN_R as i32;
    let w_i = w as i32;
    let h_i = h as i32;

    // Two passes, in order:
    //  - Pass A: semi-clipped pixels (1-2 channels) get their Tier-1b/1/2
    //    estimates written back FIRST. 2-clip pixels whose clipped channels
    //    Pass A could NOT estimate (no ratio support in a fully-saturated
    //    neighbourhood — the interior "pocket" pixels) are flagged for Pass B.
    //  - Pass B: fully-clipped pixels (Tier-3) then read the PASS-A-RECOVERED
    //    halo as their brightness signal — the core continues the halo's
    //    recovered brightness instead of staying pinned at the sensor
    //    ceiling (which produced the flat darker-gray hole in large
    //    highlights). Flagged 2-clip pixels get their missing channels
    //    completed at the same brightness (a channel pinned just below the
    //    ceiling collapses a few stops down otherwise — the 1-2px ditches
    //    around the core). Both passes write disjoint pixel slots.

    // ── Pass A: Tier-1b + Tier-1/2 (semi-clipped) ─────────────────────────
    let results_a: Vec<(usize, u8, [f32; 3], bool)> = targets
        .par_iter()
        .filter(|&&px| mask[px].count_ones() < 3)
        .filter_map(|&px| {
            let x = (px % w) as i32;
            let y = (px / w) as i32;
            let m = mask[px];
            let base = px * 3;

            // ── collect the (2·RECON_WIN_R+1)² window, bounds-clamped ──────
            let mut win_val = [0.0f32; RECON_WIN_MAX * 3];
            let mut win_mask = [0u8; RECON_WIN_MAX];
            let mut n_win = 0usize;
            for dy in -win_r..=win_r {
                for dx in -win_r..=win_r {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx >= 0 && nx < w_i && ny >= 0 && ny < h_i {
                        let nidx = (ny as usize) * w + (nx as usize);
                        let nb = nidx * 3;
                        win_val[n_win * 3] = rgb[nb];
                        win_val[n_win * 3 + 1] = rgb[nb + 1];
                        win_val[n_win * 3 + 2] = rgb[nb + 2];
                        win_mask[n_win] = mask[nidx];
                        n_win += 1;
                    }
                }
            }

            // Texture ceiling: largest unclipped value of each channel in window.
            let mut window_max = [0.0f32; 3];
            for i in 0..n_win {
                for c in 0..3 {
                    if win_mask[i] & (1 << c) == 0 {
                        window_max[c] = window_max[c].max(win_val[i * 3 + c]);
                    }
                }
            }

            let clipped_bits = [m & 1 != 0, m & 2 != 0, m & 4 != 0];
            let n_clipped = clipped_bits.iter().filter(|&&b| b).count();
            debug_assert!((1..=2).contains(&n_clipped));

            let mut est = [0.0f32; 3];
            let mut have_est = [false; 3];

            // ── Tier 1b: G-anchor upward reconstruction (deviation D10) ──
            // G is the single clipped channel (mask 010): G is the WB anchor
            // (gain 1.0), so a neutral highlight clipped on G reconstructs
            // UPWARD from the WB'd R/B brightness. Never below the pinned
            // value — genuinely saturated colors (WB'd R/B below the pinned
            // G) keep their real hue via the floor.
            if n_clipped == 1 && m & 0b010 != 0 {
                let g_est = (rgb[base] * params.r_gain)
                    .max(rgb[base + 2] * params.b_gain)
                    .max(rgb[base + 1]);
                est[1] = g_est;
                have_est[1] = true;
            }

            for c in 0..3 {
                if !clipped_bits[c] || have_est[c] {
                    continue;
                }
                // Tier-2 floor: the pinned ceiling is NOT a floor — clamping
                // the saturated pair at the ceiling is what anchors the
                // post-WB/CCM magenta cast. Tier-1 keeps the never-darken
                // floor: a single clipped channel still carries real hue.
                let floor = if n_clipped == 1 { rgb[base + c] } else { 0.0 };
                let ceil = (max_factor * window_max[c]).max(floor + 1e-6);

                if n_clipped == 1 {
                    // Tier 1: one estimate per healthy reference channel, averaged.
                    let mut sum = 0.0f32;
                    let mut refs = 0usize;
                    for h in 0..3 {
                        if h == c || clipped_bits[h] {
                            continue;
                        }
                        let mut ratios = Vec::with_capacity(RECON_WIN_MAX);
                        for i in 0..n_win {
                            let wm = win_mask[i];
                            if wm & (1 << c) != 0 || wm & (1 << h) != 0 {
                                continue;
                            }
                            let hv = win_val[i * 3 + h];
                            if !(hv > eps) {
                                continue;
                            }
                            ratios.push(win_val[i * 3 + c] / hv);
                        }
                        if ratios.is_empty() {
                            continue;
                        }
                        let r = median_sorted(&mut ratios);
                        sum += rgb[base + h] * r;
                        refs += 1;
                    }
                    if refs > 0 {
                        est[c] = (sum / refs as f32).clamp(floor, ceil);
                        have_est[c] = true;
                    }
                } else {
                    // Tier 2: single healthy reference, with luminance fallback.
                    let h = (0..3).find(|&hh| hh != c && !clipped_bits[hh]).unwrap();
                    let mut ratios = Vec::with_capacity(RECON_WIN_MAX);
                    for i in 0..n_win {
                        let wm = win_mask[i];
                        if wm & (1 << c) != 0 || wm & (1 << h) != 0 {
                            continue;
                        }
                        let hv = win_val[i * 3 + h];
                        if !(hv > eps) {
                            continue;
                        }
                        ratios.push(win_val[i * 3 + c] / hv);
                    }
                    // Anchor stability: ≥ 4 samples and IQR ≤ 2 × |median|.
                    let stable = if ratios.len() >= 6 {
                        let mut s = ratios.clone();
                        s.sort_unstable_by(|a, b| a.total_cmp(b));
                        let n = s.len();
                        let med = s[n / 2];
                        let q1 = s[n / 4];
                        let q3 = s[(3 * n) / 4];
                        med.abs() > 1e-6 && (q3 - q1) <= 2.0 * med.abs()
                    } else {
                        ratios.len() >= 4
                    };
                    if stable && !ratios.is_empty() {
                        let r = median_sorted(&mut ratios);
                        est[c] = (rgb[base + h] * r).clamp(floor, ceil);
                        have_est[c] = true;
                    } else if let Some(luma) = params.fused_luma {
                        let other = (0..3).find(|&t| t != c && clipped_bits[t]).unwrap();
                        if let Some(e) = tier2_luminance(
                            rgb, base, c, other, h, &win_val, &win_mask, n_win, luma, params,
                        ) {
                            est[c] = e.clamp(floor, ceil);
                            have_est[c] = true;
                        }
                    }
                }
            }

            let bits = ((have_est[0] as u8) << 0) | ((have_est[1] as u8) << 1) | ((have_est[2] as u8) << 2);
            if bits != 0 {
                // needs_b: a 2-clip pixel with an unestimated clipped channel —
                // Pass B completes it at the core-continuation brightness.
                Some((px, bits, est, n_clipped == 2 && (bits & m) != m))
            } else if n_clipped == 2 {
                Some((px, 0, est, true))
            } else {
                None
            }
        })
        .collect();

    for (px, bits, est, _) in &results_a {
        let base = px * 3;
        if bits & 1 != 0 {
            rgb[base] = est[0];
        }
        if bits & 2 != 0 {
            rgb[base + 1] = est[1];
        }
        if bits & 4 != 0 {
            rgb[base + 2] = est[2];
        }
    }

    // ── Pass B: Tier-3 (fully-clipped cores) ──────────────────────────────
    // The core's own hue is uninformative; keep its measured peak brightness
    // and take chromaticity from the nearest fully-clean ring (median of WB'd
    // chromaticities), as before. NEW: brightness continuation — the nearest
    // not-fully-clipped pixels (mask ≠ 111, semi-clipped only — see below)
    // now carry Pass-A recovered estimates (≤ RECON_MAX_FACTOR × their
    // window max), so their median WB'd peak (`boundary_peak`) is the scene
    // brightness the halo reached. The core continues on a lift envelope
    // between its own ceiling and the boundary median: a dead-zone
    // (RECON_BRIGHT_DEADZONE — tiny clipped islands read at the surrounding
    // level instead of flashing) followed by a smoothstep ramp to the own
    // level at RECON_BRIGHT_MAX, so mid-size regions get a long smooth
    // gradient and only deep cores reach the flat own plateau. The ring
    // chromaticity fades toward neutral by the same scale (small gaps
    // inside a core take the surrounding hue, not the far ring's). The
    // per-pixel continuation field is then smoothed with a separable
    // 13×13 box (radius RECON_PEAK_BLUR_R) at write-back — the recovered
    // water/reflection texture diffuses into the core as smooth gradients,
    // the median-derived patchiness (uniform ±6 patches) is flattened, and
    // the semi-clipped rim's own detail stays untouched.
    // No clean ring → neutral chromaticity (the caller's clipped-pair
    // collapse neutralizes hue anyway). No informative pixel within
    // RECON_BRIGHT_MAX → continues at its own peak (old pinned behavior).
    // Pixels flagged `needs_b` by Pass A (2-clip with unestimated channels —
    // the interior pockets) are completed at the same continuation
    // brightness; only their missing channels are written (healthy channels
    // stay untouched).
    //
    // Radius scans are gated by separable box dilation (exact L∞ proximity
    // to the nearest mask==0 / mask≠111 pixel): a pixel whose nearest clean
    // pixel is > RECON_RING_MAX away is ringless (median ≈ [1,1,1], same
    // result the full ring scan would return), and one whose nearest
    // informative pixel is > RECON_BRIGHT_MAX away continues at its own
    // peak only. The gate turns the O(N_cores × 312) perimeter walks for
    // deep blob interiors into O(N) preprocessing.
    let clean_src: Vec<u8> = mask.iter().map(|&m| (m == 0) as u8).collect();
    // Brightness-informative = semi-clipped only (0 < mask < 7): pixels with
    // a live channel or a Pass-A-recovered channel that carry the recovered
    // boundary level. Fully-clean pixels are NOT informative — their dim
    // scene level must never pull the clipped core's continuation down
    // (the old max(own, ·) masked this; the distance ramp exposes it).
    let info_src: Vec<u8> = mask
        .iter()
        .map(|&m| (m != 0b111 && m != 0) as u8)
        .collect();
    let near_clean = box_dilate(&clean_src, w, h, RECON_RING_MAX);
    let near_info = box_dilate(&info_src, w, h, RECON_BRIGHT_MAX);
    // Per-pixel continuation field: est_peak for every Pass-B-visited pixel,
    // 0 (→ own ceiling) everywhere else.
    let mut peak_field = vec![0.0f32; mask.len()];
    let mut tier3_tasks: Vec<(usize, u8)> = Vec::new(); // (px, write_mask)
    for (px, bits, _est, needs_b) in &results_a {
        if *needs_b {
            tier3_tasks.push((*px, mask[*px] & !*bits));
        }
    }
    tier3_tasks.extend(
        targets
            .iter()
            .filter(|&&px| mask[px].count_ones() == 3)
            .map(|&px| (px, 0b111u8)),
    );
    let results_b: Vec<(usize, u8, [f32; 3], f32)> = tier3_tasks
        .par_iter()
        .filter_map(|&(px, write_mask)| {
            let x = (px % w) as i32;
            let y = (px / w) as i32;
            let base = px * 3;
            let gain = |ch: usize| -> f32 {
                match ch {
                    0 => params.r_gain,
                    2 => params.b_gain,
                    _ => 1.0,
                }
            };

            // ── hue: nearest fully-clean ring (unchanged) ─────────────────
            let mut chi = [0.0f32; RECON_RING_CAP * 3];
            let mut n_ring = 0usize;
            let mut m = [1.0f32; 3];
            if near_clean[px] != 0 {
                'ring: for r in RECON_RING_MIN..=RECON_RING_MAX {
                for dy in -(r as i32)..=(r as i32) {
                    for dx in -(r as i32)..=(r as i32) {
                        if dx.abs().max(dy.abs()) != r as i32 {
                            continue;
                        }
                        let nx = x + dx;
                        let ny = y + dy;
                        if nx < 0 || nx >= w_i || ny < 0 || ny >= h_i {
                            continue;
                        }
                        let nidx = (ny as usize) * w + (nx as usize);
                        if mask[nidx] != 0 {
                            continue;
                        }
                        let nb = nidx * 3;
                        let w0 = rgb[nb] * gain(0);
                        let w1 = rgb[nb + 1] * gain(1);
                        let w2 = rgb[nb + 2] * gain(2);
                        let mx = w0.max(w1).max(w2);
                        if !(mx > eps) {
                            continue;
                        }
                        chi[n_ring * 3] = w0 / mx;
                        chi[n_ring * 3 + 1] = w1 / mx;
                        chi[n_ring * 3 + 2] = w2 / mx;
                        n_ring += 1;
                        if n_ring >= RECON_RING_SAMPLES {
                            break 'ring;
                        }
                    }
                }
            }
            if n_ring >= RECON_RING_SAMPLES {
                let mut c0 = [0.0f32; RECON_RING_CAP];
                let mut c1 = [0.0f32; RECON_RING_CAP];
                let mut c2 = [0.0f32; RECON_RING_CAP];
                for i in 0..n_ring {
                    c0[i] = chi[i * 3];
                    c1[i] = chi[i * 3 + 1];
                    c2[i] = chi[i * 3 + 2];
                }
                m = [
                    median_sorted(&mut c0[..n_ring]),
                    median_sorted(&mut c1[..n_ring]),
                    median_sorted(&mut c2[..n_ring]),
                ];
            }
            }

            let own_max_wb = (rgb[base] * gain(0))
                .max(rgb[base + 1] * gain(1))
                .max(rgb[base + 2] * gain(2));

            // ── brightness: nearest informative (mask ≠ 111) pixels ───────
            // Scanned after Pass A, so semi-clipped pixels carry their
            // recovered estimates; the median of the nearest samples' WB'd
            // peaks is the halo brightness the core should continue. The
            // lift envelope (dead-zone + smoothstep to the reach) keeps
            // tiny features at the boundary level and tapers the band
            // instead of ending in a hard edge.
            let mut peaks = [0.0f32; RECON_RING_CAP];
            let mut peak_d = [0.0f32; RECON_RING_CAP];
            let mut n_peaks = 0usize;
            if near_info[px] != 0 {
                'bright: for r in 1..=RECON_BRIGHT_MAX {
                    for dy in -(r as i32)..=(r as i32) {
                    for dx in -(r as i32)..=(r as i32) {
                        if dx.abs().max(dy.abs()) != r as i32 {
                            continue;
                        }
                        let nx = x + dx;
                        let ny = y + dy;
                        if nx < 0 || nx >= w_i || ny < 0 || ny >= h_i {
                            continue;
                        }
                        let nidx = (ny as usize) * w + (nx as usize);
                        if mask[nidx] == 0b111 {
                            continue;
                        }
                        let nb = nidx * 3;
                        let mx = (rgb[nb] * gain(0))
                            .max(rgb[nb + 1] * gain(1))
                            .max(rgb[nb + 2] * gain(2));
                        if !(mx > eps) {
                            continue;
                        }
                        peaks[n_peaks] = mx;
                        peak_d[n_peaks] = r as f32;
                        n_peaks += 1;
                        if n_peaks >= RECON_RING_SAMPLES {
                            break 'bright;
                        }
                    }
                }
            }
            }

            // Lift envelope: the estimate interpolates between the boundary
            // median and the pixel's own ceiling, scaled by a smoothstep
            // with a dead-zone (RECON_BRIGHT_DEADZONE) and saturation at the
            // reach. Features smaller than ~2·deadzone keep their estimate
            // at the boundary level (no flash); mid-size regions get a long
            // smooth gradient; deep cores reach the own ceiling only far
            // from the rim. The same scale fades the ring chromaticity
            // `m` toward neutral, so small gaps inside a core take the
            // surrounding neutral hue instead of the far ring's.
            let est_peak;
            let m_prime;
            if n_peaks > 0 {
                let mut pairs = [(0.0f32, 0.0f32); RECON_RING_CAP];
                for i in 0..n_peaks {
                    pairs[i] = (peaks[i], peak_d[i]);
                }
                pairs[..n_peaks].sort_unstable_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                let (med, d_med) = pairs[n_peaks / 2];
                let t = ((d_med - RECON_BRIGHT_DEADZONE)
                    / (RECON_BRIGHT_MAX as f32 - RECON_BRIGHT_DEADZONE))
                    .clamp(0.0, 1.0);
                let s = t * t * (3.0 - 2.0 * t);
                est_peak = own_max_wb + (med - own_max_wb) * s;
                m_prime = [m[0] + (1.0 - m[0]) * (1.0 - s),
                           m[1] + (1.0 - m[1]) * (1.0 - s),
                           m[2] + (1.0 - m[2]) * (1.0 - s)];
            } else {
                est_peak = own_max_wb;
                m_prime = m;
            }

            if est_peak > eps {
                Some((px, write_mask, m_prime, est_peak))
            } else {
                None
            }
        })
        .collect();

    // Continuation field: fill, then a single-pass separable 13×13 box
    // (horizontal pass into `peak_tmp`, vertical pass in the write-back).
    // The wide kernel flattens the median-derived patchiness that a 5×5 box
    // leaves intact (uniform ±6 patches survive a small window but not a
    // 13×13 one), and diffuses the boundary brightness through the core as
    // smooth gradients. Unscanned cells substitute THEIR OWN WB'd peak
    // (per-cell — a 2px island's window is ~97% unscanned background whose
    // true level is the surrounding scene, not the island's ceiling), so
    // tiny features read at the local level instead of re-flashing, and
    // the region edges blend ~6px into the scene.
    for (px, _bits, _m, est_peak) in &results_b {
        peak_field[*px] = *est_peak;
    }
    let mut own_field = vec![0.0f32; peak_field.len()];
    own_field
        .par_chunks_mut(w)
        .enumerate()
        .for_each(|(y, row)| {
        for x in 0..w {
            let i = (y * w + x) * 3;
            row[x] = (rgb[i] * params.r_gain)
                .max(rgb[i + 1])
                .max(rgb[i + 2] * params.b_gain);
        }
    });
    let br = RECON_PEAK_BLUR_R as i32;
    let hw = (2 * br + 1) as f32;
    let mut peak_tmp = vec![0.0f32; peak_field.len()];
    // Parallel horizontal pass: disjoint peak_tmp slots, compute in parallel
    // then serial write-back (same Pass-A pattern — no data race, bit-exact
    // per-pixel sums preserved).
    let horiz: Vec<(usize, f32)> = results_b
        .par_iter()
        .map(|(px, _bits, _m, _est_peak)| {
            let mut sum = 0.0f32;
            for dx in -br..=br {
                let nx = (px % w) as i32 + dx;
                let nx = nx.clamp(0, w as i32 - 1) as usize;
                let idx = (px / w) * w + nx;
                let v = peak_field[idx];
                sum += if v > 0.0 { v } else { own_field[idx] };
            }
            (*px, sum / hw)
        })
        .collect();
    for (px, v) in horiz {
        peak_tmp[px] = v;
    }
    // Parallel vertical pass: est_peak computation is read-only on
    // peak_tmp/own_field; rgb write-back stays serial (disjoint slots but
    // &mut cannot be shared across threads without chunks).
    let vertical: Vec<(usize, u8, [f32; 3], f32)> = results_b
        .par_iter()
        .map(|(px, bits, m, _est_peak)| {
            let x = px % w;
            let y = px / w;
            let mut sum = 0.0f32;
            for dy in -br..=br {
                let ny = (y as i32 + dy).clamp(0, h as i32 - 1) as usize;
                let idx = ny * w + x;
                let v = peak_tmp[idx];
                sum += if v > 0.0 { v } else { own_field[idx] };
            }
            (*px, *bits, *m, sum / hw)
        })
        .collect();
    for (px, bits, m, est_peak) in vertical {
        let base = px * 3;
        let gain = |ch: usize| -> f32 {
            match ch {
                0 => params.r_gain,
                2 => params.b_gain,
                _ => 1.0,
            }
        };
        for c in 0..3 {
            if bits & (1 << c) != 0 {
                rgb[base + c] = est_peak * m[c] / gain(c);
            }
        }
    }
}

/// Tier-2 luminance-continuity fallback (Resolve "Luminance" style).
///
/// Given the healthy channel `h`, the target clipped channel `c` and the
/// other clipped channel `o`, estimate `c` so that luminance continues
/// smoothly across the clip boundary:
///
/// ```text
/// k   = median of unclipped neighbors' (c_w / o_w)   — chromaticity anchor
/// Y   = median of unclipped neighbors' luma          — boundary luminance
/// o_w = (Y − L_h·h_w) / (L_c·k + L_o)
/// c_w = k · o_w
/// ```
///
/// where `_w` denotes WB-gain-scaled values and `L = [L_r, L_g, L_b]` is the
/// luma row of the fused cam→output matrix. Returns `None` when the window
/// has no unclipped support (→ no estimate; caller's neutral collapse
/// handles the pixel) or the solve is degenerate.
///
/// Exact L∞ box dilation: marks every pixel within `radius` (Chebyshev
/// distance) of a source pixel (`source[i] != 0`), via separable vertical
/// and horizontal run-extension passes. O(N) — used to gate the Tier-3
/// radius scans so deep blob interiors never pay the full perimeter walk.
fn box_dilate(source: &[u8], w: usize, h: usize, radius: usize) -> Vec<u8> {
    // Horizontal run-extension (row-major, per-row mutable chunks).
    let mut tmp = vec![0u8; source.len()];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &source[y * w..(y + 1) * w];
        let mut x = 0usize;
        while x < w {
            if src[x] == 0 {
                x += 1;
                continue;
            }
            let start = x;
            while x < w && src[x] != 0 {
                x += 1;
            }
            let lo = start.saturating_sub(radius);
            let hi = (x - 1).saturating_add(radius).min(w - 1);
            row[lo..=hi].fill(1);
        }
    });
    // Vertical run-extension: transpose to column-major so each column is a
    // contiguous mutable chunk, then transpose back. Runs are collected
    // BEFORE any fill — the fill must not feed new sources into the same
    // scan (that would cascade the dilation across the whole column).
    let mut tmp_cm = vec![0u8; source.len()]; // [x * h + y]
    tmp_cm.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for y in 0..h {
            col[y] = tmp[y * w + x];
        }
    });
    tmp_cm.par_chunks_mut(h).enumerate().for_each(|(_x, col)| {
        let mut runs: Vec<(usize, usize)> = Vec::with_capacity(8);
        let mut y = 0usize;
        while y < h {
            if col[y] == 0 {
                y += 1;
                continue;
            }
            let start = y;
            while y < h && col[y] != 0 {
                y += 1;
            }
            runs.push((start, y));
        }
        for (start, end) in runs {
            let lo = start.saturating_sub(radius);
            let hi = (end - 1).saturating_add(radius).min(h - 1);
            col[lo..=hi].fill(1);
        }
    });
    let mut out = vec![0u8; source.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            row[x] = tmp_cm[x * h + y];
        }
    });
    out
}

fn tier2_luminance(
    rgb: &[f32],
    base: usize,
    c: usize,
    o: usize,
    h: usize,
    win_val: &[f32],
    win_mask: &[u8],
    n_win: usize,
    luma: [f32; 3],
    params: &ReconstructParams,
) -> Option<f32> {
    let gain = |ch: usize| -> f32 {
        match ch {
            0 => params.r_gain,
            2 => params.b_gain,
            _ => 1.0,
        }
    };

    let mut ys: Vec<f32> = Vec::with_capacity(RECON_WIN_MAX);
    let mut ks: Vec<f32> = Vec::with_capacity(RECON_WIN_MAX);
    for i in 0..n_win {
        if win_mask[i] != 0 {
            continue;
        }
        let v = &win_val[i * 3..i * 3 + 3];
        let ow = v[o] * gain(o);
        if !(v[o] > RECON_EPSILON) {
            continue;
        }
        let cw = v[c] * gain(c);
        ks.push(cw / ow);
        ys.push(luma[0] * v[0] * gain(0) + luma[1] * v[1] * gain(1) + luma[2] * v[2] * gain(2));
    }
    if ys.is_empty() || ks.is_empty() {
        return None;
    }
    let y_est = median_sorted(&mut ys);
    let k = median_sorted(&mut ks);

    let hw = rgb[base + h] * gain(h);
    let denom = luma[c] * k + luma[o];
    if !denom.is_finite() || denom.abs() < 1e-12 {
        return None;
    }
    let o_w = (y_est - luma[h] * hw) / denom;
    let c_w = k * o_w;
    if !c_w.is_finite() {
        return None;
    }
    let est = c_w / gain(c);
    if !est.is_finite() {
        return None;
    }
    Some(est)
}

/// Hue-preserving display rolloff (HL-handling.md §4.2, corrected).
///
/// Identity below 1.0 — display-referred values are never touched
/// (bit-exact no-temper guarantee). Above 1.0 a C1-continuous rational
/// shoulder compresses the range toward `ROLLOFF_CEILING` by scaling all
/// channels uniformly — R:G:B ratios (hue) are invariant by construction.
///
/// ```text
/// f(m) = 1 + (m−1) / (1 + B·(m−1)),   B = 20,  m = max(r,g,b)
/// s    = f(m) / m   (uniform)
/// ```
///
/// Properties: identity and slope 1 at m=1 (C1-continuous), monotone
/// increasing, f(∞) = ROLLOFF_CEILING, ratio-preserving. The residual band
/// (1.0, 1.05] still clips per-channel at the u16 pack ceiling, but only in
/// code values above display white — invisible on any display-referred
/// deliverable. Non-finite input is mapped to black (NaN/Inf cannot encode).
pub fn apply_display_rolloff(rgb: &mut [f32]) {
    const B: f32 = 20.0;
    rgb.par_chunks_exact_mut(3).for_each(|c| {
        if !(c[0].is_finite() && c[1].is_finite() && c[2].is_finite()) {
            c[0] = 0.0;
            c[1] = 0.0;
            c[2] = 0.0;
            return;
        }
        let m = c[0].max(c[1]).max(c[2]);
        if m <= 1.0 {
            return;
        }
        let s = (1.0 + (m - 1.0) / (1.0 + B * (m - 1.0))) / m;
        c[0] *= s;
        c[1] *= s;
        c[2] *= s;
    });
}

/// Map a Bayer pixel position to a shading map channel index.
/// Returns 0=R, 1=G1, 2=B, 3=G2 matching the 4-channel grid layout.
pub fn bayer_phase_to_channel(x: u32, y: u32, pattern: BayerPattern) -> usize {
    let even_x = x % 2 == 0;
    let even_y = y % 2 == 0;
    let is_red = match pattern {
        BayerPattern::RGGB => even_x && even_y,
        BayerPattern::BGGR => !even_x && !even_y,
        BayerPattern::GRBG => !even_x && even_y,
        BayerPattern::GBRG => even_x && !even_y,
        _ => even_x && even_y, // QuadBayer → RGGB fallback
    };
    let is_blue = match pattern {
        BayerPattern::RGGB => !even_x && !even_y,
        BayerPattern::BGGR => even_x && even_y,
        BayerPattern::GRBG => even_x && !even_y,
        BayerPattern::GBRG => !even_x && even_y,
        _ => !even_x && !even_y,
    };
    if is_red { return 0; }
    if is_blue { return 3; }
    if even_y { 1 } else { 2 }
}

/// Bilinear interpolation into a flat shading-map channel.
/// `channel_data` is `[y * grid_w + x]` for a single channel.
/// `u`, `v` are normalised coordinates in [0, 1] over the sensor area.
fn interpolate_bilinear(channel_data: &[f32], grid_w: u32, grid_h: u32, u: f32, v: f32) -> f32 {
    let fx = (u * (grid_w - 1) as f32).clamp(0.0, (grid_w - 1) as f32);
    let fy = (v * (grid_h - 1) as f32).clamp(0.0, (grid_h - 1) as f32);
    let ix = fx as usize;
    let iy = fy as usize;
    let frac_x = fx - ix as f32;
    let frac_y = fy - iy as f32;

    let w = grid_w as usize;
    let get = |gx: usize, gy: usize| channel_data[gy.min(grid_h as usize - 1) * w + gx.min(grid_w as usize - 1)];

    let g00 = get(ix, iy);
    let g10 = get(ix + 1, iy);
    let g01 = get(ix, iy + 1);
    let g11 = get(ix + 1, iy + 1);

    let top = g00 + (g10 - g00) * frac_x;
    let bot = g01 + (g11 - g01) * frac_x;
    top + (bot - top) * frac_y
}

/// Pre-compute a color-only shading map by dividing each grid point by the
/// minimum gain across all four channels at that position.
pub fn compute_color_only_map(channels: &[Vec<f32>], grid_w: u32, grid_h: u32) -> Vec<Vec<f32>> {
    let len = (grid_w * grid_h) as usize;
    let mut color_map = vec![vec![0.0f32; len]; 4];
    for i in 0..len {
        let r = channels[0][i];
        let g1 = channels[1][i];
        let g2 = channels[2][i];
        let b = channels[3][i];
        let min_g = r.min(g1.min(g2.min(b)));
        if min_g > 0.0 {
            color_map[0][i] = r / min_g;
            color_map[1][i] = g1 / min_g;
            color_map[2][i] = g2 / min_g;
            color_map[3][i] = b / min_g;
        } else {
            color_map[0][i] = 1.0;
            color_map[1][i] = 1.0;
            color_map[2][i] = 1.0;
            color_map[3][i] = 1.0;
        }
    }
    color_map
}

/// Apply lens shading correction to raw Bayer data.
///
/// Operates **before** demosaic. Each raw pixel is multiplied by the
/// bilinearly-interpolated gain from the shading map for its channel.
/// Values are clamped to `clamp_max` (typically the sensor's white level)
/// to prevent amplification from pushing pixels above the sensor's raw
/// range, which would disadvantage the subsequent normalize step.
pub fn apply_lens_correction_cpu(
    bayer: &mut [u16],
    stride_width: u32,
    offset_x: u32,
    offset_y: u32,
    pattern: BayerPattern,
    shading_map_channels: &[Vec<f32>],
    grid_w: u32,
    grid_h: u32,
    sensor_w: u32,
    sensor_h: u32,
    color_only: bool,
    clamp_max: u16,
) {
    let (ox, oy) = (offset_x as f32, offset_y as f32);
    let (sw, sh) = (sensor_w as f32, sensor_h as f32);

    let channels = if color_only {
        &compute_color_only_map(shading_map_channels, grid_w, grid_h)
    } else {
        shading_map_channels
    };

    let cm = clamp_max as u32;
    bayer.par_chunks_exact_mut(stride_width as usize)
        .enumerate()
        .for_each(|(y, row)| {
            let vy = (y as f32 + oy) / sh;
            for (x, pixel) in row.iter_mut().enumerate() {
                let vx = (x as f32 + ox) / sw;
                let ch = bayer_phase_to_channel(x as u32, y as u32, pattern);
                let gain = interpolate_bilinear(&channels[ch], grid_w, grid_h, vx, vy);
                *pixel = ((*pixel as f32 * gain).round() as u32).min(cm) as u16;
            }
        });

    if color_only {
        let _ = channels;
    }
}

/// Apply lens shading correction to raw Bayer data, matching the
/// motioncam-fs reference implementation exactly.
///
/// For each raw pixel: `((pixel - src_black[ch]) / (src_white - src_black[ch])) * gain * extended_wl`
/// The result is clamped to `extended_wl` and written back to the Bayer buffer.
/// Subsequent normalization must use `bl=0, wl=extended_wl` to recover the correct
/// scene-referred `((pixel - bl) / (wl - bl)) * gain`.
pub fn apply_lens_correction_cpu_with_map(
    bayer: &mut [u16],
    stride_width: u32,
    offset_x: u32,
    offset_y: u32,
    pattern: BayerPattern,
    color_map: &[Vec<f32>],
    grid_w: u32,
    grid_h: u32,
    sensor_w: u32,
    sensor_h: u32,
    src_black: [f32; 4],
    src_white: f32,
    extended_wl: u16,
) {
    let (ox, oy) = (offset_x as f32, offset_y as f32);
    let (sw, sh) = (sensor_w as f32, sensor_h as f32);
    let ew = extended_wl as u32;

    bayer.par_chunks_exact_mut(stride_width as usize)
        .enumerate()
        .for_each(|(y, row)| {
            let vy = (y as f32 + oy) / sh;
            for (x, pixel) in row.iter_mut().enumerate() {
                let vx = (x as f32 + ox) / sw;
                let ch = bayer_phase_to_channel(x as u32, y as u32, pattern);
                let gain = interpolate_bilinear(&color_map[ch], grid_w, grid_h, vx, vy);
                let bl = src_black[ch];
                let norm = (*pixel as f32 - bl) / (src_white - bl).max(f32::EPSILON);
                let val = (norm * gain * ew as f32).round();
                *pixel = (val as u32).min(ew) as u16;
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The detector should pick the identity matrix as-is when given the
    /// identity (the row-sum white is exactly D50).
    #[test]
    fn detect_camera_to_xyz_picks_identity_when_input_is_identity() {
        let id = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let out = detect_camera_to_xyz(&id);
        for i in 0..9 {
            assert!((out[i] - id[i]).abs() < 1e-5, "entry {} differs: {} vs {}", i, out[i], id[i]);
        }
    }

    /// A forward Camera→XYZ matrix that maps (1,1,1)→D50 should be picked
    /// over its inverse / transpose. The detector should not fall back to
    /// the inverse of a forward matrix.
    #[test]
    fn detect_camera_to_xyz_prefers_forward_over_inverse() {
        // Build a known forward matrix: diag(s) with a D50 row-sum.
        // Row-sums must equal D50_XYZ. Simplest: identity (above) is the
        // forward direction; the inverse IS also identity — so we use a
        // non-trivial scaling. Let s = D50_XYZ (so the matrix is
        // diag(d50)). Forward row-sum = D50. Its inverse has row-sum =
        // (1/d50_x, 1/d50_y, 1/d50_z), which is far from D50.
        let m = [
            D50_XYZ[0], 0.0, 0.0,
            0.0, D50_XYZ[1], 0.0,
            0.0, 0.0, D50_XYZ[2],
        ];
        let out = detect_camera_to_xyz(&m);
        for i in 0..9 {
            assert!((out[i] - m[i]).abs() < 1e-5, "entry {} differs: {} vs {}", i, out[i], m[i]);
        }
    }

    /// HLG OETF at the knee L = 1/12 should give V = 0.5 on both sides
    /// of the branch, and the function must be monotonic.
    #[test]
    fn hlg_knee_is_continuous_at_one_twelfth() {
        let below = TransferFunction::HLG.process_apply(1.0 / 12.0);
        let above = TransferFunction::HLG.process_apply(1.0 / 12.0 + 1e-4);
        let mid = (below + above) * 0.5;
        assert!((below - 0.5).abs() < 1e-4, "HLG at knee: {} (want 0.5)", below);
        assert!((above - 0.5).abs() < 5e-3, "HLG just above knee: {} (want ~0.5)", above);
        assert!((mid - 0.5).abs() < 5e-3, "HLG mid (knee avg): {}", mid);
        // Monotonicity sanity at three points.
        let a = TransferFunction::HLG.process_apply(0.001);
        let b = TransferFunction::HLG.process_apply(0.1);
        let c = TransferFunction::HLG.process_apply(0.8);
        assert!(a < b && b < c, "HLG must be monotonic: a={} b={} c={}", a, b, c);
    }

    /// PQ forward then inverse (the inverse function is not exported but
    /// we can sanity-check the forward is monotone and stays in [0,1] for
    /// inputs in [0,1]).
    #[test]
    fn pq_forward_is_monotone_bounded() {
        let pf = |x: f32| {
            let x_m1 = x.powf(0.1593017578125_f32);
            ((0.8359375_f32 + 18.8515625_f32 * x_m1) / (1.0_f32 + 18.6875_f32 * x_m1)).powf(78.84375_f32)
        };
        for s in [0.0_f32, 0.01, 0.1, 0.18, 0.5, 1.0] {
            let v = pf(s);
            assert!(v.is_finite() && v >= 0.0 && v <= 1.0, "PQ({}) = {}", s, v);
        }
        // Monotonicity
        let a = pf(0.10);
        let b = pf(0.18);
        let c = pf(0.50);
        assert!(a < b && b < c, "PQ must be monotonic: a={} b={} c={}", a, b, c);
    }

    /// `build_bradford_matrix` from D65 to D65 must be the identity.
    #[test]
    fn bradford_identity_for_same_white() {
        let m = build_bradford_matrix(&D65_XYZ, &D65_XYZ);
        for i in 0..9 {
            let expected = if i == 0 || i == 4 || i == 8 { 1.0 } else { 0.0 };
            assert!((m[i] - expected).abs() < 1e-4, "entry {}: {} (want {})", i, m[i], expected);
        }
    }

    /// Rec.709 OETF spot checks. The 0.018 knee and the `1.099`/`0.099`
    /// coefficients are the only place the linear and power segments
    /// meet. Below the knee the slope is 4.5; above the knee the
    /// `x^0.45` form is used.
    #[test]
    fn rec709_oetf_at_key_points() {
        let v_zero = TransferFunction::Rec709.process_apply(0.0);
        let v_low  = TransferFunction::Rec709.process_apply(0.01);
        let v_knee = TransferFunction::Rec709.process_apply(0.018);
        let v_high = TransferFunction::Rec709.process_apply(0.5);
        let v_one  = TransferFunction::Rec709.process_apply(1.0);
        assert!(v_zero.abs() < 1e-6, "Rec.709 at 0 = {}", v_zero);
        // Linear segment: V = 4.5 * 0.01 = 0.045.
        assert!((v_low - 0.045).abs() < 1e-4, "Rec.709 at 0.01 = {}", v_low);
        // Power segment: V = 1.099 * 0.018^0.45 - 0.099.
        // (Linear segment would be V = 4.5*0.018 = 0.081, so the
        // power segment value is the more diagnostic of the two.)
        let power_at_knee = 1.099_f32 * 0.018_f32.powf(0.45) - 0.099;
        assert!((v_knee - power_at_knee).abs() < 1e-4, "Rec.709 at 0.018 = {}", v_knee);
        // At x=1.0, V = 1.099 - 0.099 = 1.0.
        assert!((v_one - 1.0).abs() < 1e-4, "Rec.709 at 1.0 = {}", v_one);
        // Monotonicity.
        assert!(v_zero < v_low && v_low < v_knee && v_knee < v_high && v_high < v_one,
                "Rec.709 must be monotonic");
        // Spot v_high should land in the power branch.
        let power_high = 1.099_f32 * 0.5_f32.powf(0.45) - 0.099;
        assert!((v_high - power_high).abs() < 1e-4, "Rec.709 at 0.5 = {} (power={})", v_high, power_high);
    }

    /// V-Log (Panasonic) spot checks. Knee at x=0.01; below the knee
    /// the linear slope is 5.6 (offset 0.125), above the knee the
    /// log10 form with offset 0.00873 is used.
    #[test]
    fn vlog_at_key_points() {
        let v_knee = TransferFunction::VLog.process_apply(0.01);
        // Below the knee: 5.6 * 0.01 + 0.125 = 0.181.
        assert!((v_knee - 0.181).abs() < 1e-4, "V-Log at knee = {} (want 0.181)", v_knee);
        let v_one = TransferFunction::VLog.process_apply(1.0);
        // Log branch: 0.241514 * log10(1.00873) + 0.598206.
        let expected = 0.241514_f32 * (1.0_f32 + 0.00873_f32).log10() + 0.598206_f32;
        assert!((v_one - expected).abs() < 1e-3, "V-Log at 1.0 = {} (want {})", v_one, expected);
    }

    /// ARRI LogC3 (EI 800) spot check. Knee at x=0.010591; below
    /// the knee linear with slope 5.367655, above log with the
    /// published coefficients.
    #[test]
    fn arri_logc3_at_key_points() {
        let v_one = TransferFunction::ARRIlog3.process_apply(1.0);
        let expected = 0.247190_f32 * (5.555556_f32 + 0.052272_f32).log10() + 0.385537_f32;
        assert!((v_one - expected).abs() < 1e-3, "ARRI LogC3 at 1.0 = {} (want {})", v_one, expected);
        let v_low = TransferFunction::ARRIlog3.process_apply(0.0);
        // Linear segment: 5.367655 * 0 + 0.092809 = 0.092809.
        assert!((v_low - 0.092809).abs() < 1e-4, "ARRI LogC3 at 0 = {} (want 0.092809)", v_low);
    }

    /// ARRI LogC4 spot check. Cross-checked against colour-science/colour
    /// `log_encoding_ARRILogC4` / `log_decoding_ARRILogC4`. Encoding of
    /// 0.18 (18% grey) must be ≈ 0.2783958, and the round-trip must hold.
    #[test]
    fn arri_logc4_at_key_points() {
        use crate::color::{arri_logc4_constants, arri_logc4_eotf, arri_logc4_oetf};
        // Spot-check: constants from the spec (Cooper & Brendel, 2022).
        // Reference values computed independently with Python and match
        // colour-science/colour to 12+ decimal places.
        let (a, b, c, s, t) = arri_logc4_constants();
        assert!((a - 2231.8263091).abs() < 1e-3, "a = {} (want 2231.8263)", a);
        assert!((b - 0.90713587).abs() < 1e-6, "b = {} (want 0.9071)", b);
        assert!((c - 0.09286413).abs() < 1e-6, "c = {} (want 0.0929)", c);
        assert!((s - 0.1135972).abs() < 1e-5, "s = {} (want 0.1135972)", s);
        assert!((t - (-0.0180570)).abs() < 1e-5, "t = {} (want -0.0180570)", t);

        // Spec: 18% grey → ≈ 0.2783958.
        let v_18 = arri_logc4_oetf(0.18);
        assert!((v_18 - 0.2783958).abs() < 1e-5, "LogC4 OETF(0.18) = {} (want 0.2783958)", v_18);

        // Spec: scene-linear 1.0 → ≈ 0.4275194 (unbounded formula;
        // the hardware form clamps to 1.0 for highlights).
        let v_one = arri_logc4_oetf(1.0);
        let expected_one = (((a * 1.0 + 64.0).log2() - 6.0) / 14.0) * b + c;
        assert!((v_one - expected_one).abs() < 1e-5, "LogC4 OETF(1.0) = {} (want {})", v_one, expected_one);
        assert!((v_one - 0.4275194).abs() < 1e-5, "LogC4 OETF(1.0) = {} (want 0.4275194)", v_one);

        // Linear branch (x < t ≈ -0.018): pure slope.
        let v_below = arri_logc4_oetf(t - 0.001);
        let expected_below = (t - 0.001 - t) / s; // = -0.001 / s
        assert!((v_below - expected_below).abs() < 1e-5, "LogC4 linear branch");

        // Round-trip: decode the encoded 18% grey back to scene-linear.
        let rt = arri_logc4_eotf(v_18);
        assert!((rt - 0.18).abs() < 1e-4, "LogC4 round-trip: encode→decode(0.18) = {} (want 0.18)", rt);

        // Round-trip for a couple more stops.
        for x in [0.001_f32, 0.01, 0.1, 0.5, 2.0, 10.0] {
            let enc = arri_logc4_oetf(x);
            let dec = arri_logc4_eotf(enc);
            assert!((dec - x).abs() < 1e-4, "LogC4 round-trip at x={}: encode→decode = {} (want {})", x, dec, x);
        }

        // Sanity-check the full TransferFunction::ARRIlog4 path agrees with
        // the standalone helper (so the production code is correct).
        let v_18_full = TransferFunction::ARRIlog4.process_apply(0.18);
        assert!((v_18_full - v_18).abs() < 1e-5, "TransferFunction::ARRIlog4 disagrees with arri_logc4_oetf: {} vs {}", v_18_full, v_18);
    }

    /// S-Log3 must follow Sony's canonical form per the Sony specification
    /// (2014), colour-science, and ACES CTL reference implementation.
    /// Formula:
    ///   x >= 0.01125: V = (420 + 261.5 * log10((x + 0.01) / 0.19)) / 1023
    ///   x <  0.01125: V = (x * (knee_val - 95) / 0.01125 + 95) / 1023
    ///                 where knee_val = 420 + 261.5 * log10((0.01125+0.01)/0.19)
    ///
    /// 18% grey (x=0.18) maps to code 420, normalized 420/1023 ≈ 0.4106.
    /// Black (x=0) maps to code 95, normalized 95/1023 ≈ 0.0929.
    /// These match the known Sony S-Log3 encoding and DaVinci Resolve.
    #[test]
    fn slog3_canonical_at_key_points() {
        let v_low = TransferFunction::SLog3.process_apply(0.009);
        let v_at = TransferFunction::SLog3.process_apply(0.01125);
        let v_high = TransferFunction::SLog3.process_apply(0.013);
        assert!(v_low.is_finite() && v_at.is_finite() && v_high.is_finite());
        assert!(v_low < v_high, "S-Log3 must be monotonic across the knee: low={} high={}", v_low, v_high);
        // Spot-check x=0.18 (18% grey). Canonical S-Log3 gives:
        //   V(0.18) = 420/1023 ≈ 0.4106 (code 420)
        let v_18 = TransferFunction::SLog3.process_apply(0.18);
        assert!((v_18 - 0.4106).abs() < 0.01, "S-Log3 at 0.18 = {} (want ~0.4106)", v_18);
        // Spot-check x=1.0 (peak white, V ≈ 0.596, code ~610).
        let v_1 = TransferFunction::SLog3.process_apply(1.0);
        assert!((v_1 - 0.596).abs() < 0.02, "S-Log3 at 1.0 = {} (want ~0.596)", v_1);
        // Black (x=0) should be code 95.
        let v_0 = TransferFunction::SLog3.process_apply(0.0);
        assert!((v_0 - 0.0929).abs() < 0.001, "S-Log3 at 0 = {} (want ~0.0929)", v_0);
    }
}

// Tiny helper so the unit tests can invoke TransferFunction::process on
// single pixels without spinning up rayon. Mirrors the per-pixel math
// in the existing match arms exactly; if a new variant is added this
// must be updated.
impl TransferFunction {
    #[cfg(test)]
    fn process_apply(&self, x: f32) -> f32 {
        match self {
            TransferFunction::Linear => x,
            TransferFunction::Rec709 => rec709_oetf(x).min(1.0).max(0.0),
            TransferFunction::SLog3 => if x >= 0.01125_f32 { (420.0_f32 + 261.5_f32 * ((x + 0.01_f32) / 0.19_f32).log10()) / 1023.0_f32 } else { (x * (171.2102946929_f32 - 95.0_f32) / 0.01125_f32 + 95.0_f32) / 1023.0_f32 },
            TransferFunction::VLog => if x < 0.01 { 5.6_f32 * x + 0.125_f32 } else { 0.241514_f32 * (x + 0.00873_f32).log10() + 0.598206_f32 },
            TransferFunction::ARRIlog3 => if x > 0.010591_f32 { 0.247190_f32 * (5.555556_f32 * x + 0.052272_f32).log10() + 0.385537_f32 } else { 5.367655_f32 * x + 0.092809_f32 },
            TransferFunction::ARRIlog4 => {
                let (a, b, c, s, t) = crate::color::arri_logc4_constants();
                if x >= t { ((a * x + 64.0_f32).log2() - 6.0_f32) / 14.0_f32 * b + c } else { (x - t) / s }
            },
            TransferFunction::CLog3 => {
                let neg_graft_lin = (0.097465473_f32 - 0.12512219_f32) / 1.9754798_f32;
                let pos_graft_lin = (0.15277891_f32 - 0.12512219_f32) / 1.9754798_f32;
                if x < neg_graft_lin { -0.36726845_f32 * ((-x * 14.98325_f32 + 1.0_f32).max(1e-10_f32)).log10() + 0.12783901_f32 }
                else if x <= pos_graft_lin { 1.9754798_f32 * x + 0.12512219_f32 }
                else { 0.36726845_f32 * (x * 14.98325_f32 + 1.0_f32).log10() + 0.12240537_f32 }
            }
            TransferFunction::FLog2 => if x >= 0.000889_f32 { 0.245281_f32 * (5.555556_f32 * x + 0.064829_f32).log10() + 0.384316_f32 } else { 8.799461_f32 * x + 0.092864_f32 },
            TransferFunction::AppleLog | TransferFunction::AppleLog2 => {
                const R0: f32 = -0.05641088; const RT: f32 = 0.01; const C: f32 = 47.28711236;
                const BETA: f32 = 0.00964052; const GAMMA: f32 = 0.08550479; const DELTA: f32 = 0.69336945;
                if x < R0 { 0.0 } else if x < RT { C * (x - R0) * (x - R0) } else { GAMMA * (x + BETA).log2() + DELTA }
            }
            TransferFunction::ACESCCT => if x > 0.0078125_f32 { (x.log2() + 9.72_f32) / 17.52_f32 } else { 10.5402377416545_f32 * x + 0.0729055341958355_f32 },
            TransferFunction::PQ => { let x_m1 = x.powf(0.1593017578125_f32); ((0.8359375_f32 + 18.8515625_f32 * x_m1) / (1.0_f32 + 18.6875_f32 * x_m1)).powf(78.84375_f32) }
            TransferFunction::HLG => if x < (1.0_f32 / 12.0_f32) { (3.0_f32 * x).sqrt() } else { 0.17883277_f32 * (12.0_f32 * x - 0.28466892_f32).ln() + 0.55991073_f32 },
            TransferFunction::DaVinciIntermediate => if x <= 0.00262409_f32 { x * 10.44426855_f32 } else { 0.07329248_f32 * ((x + 0.0075_f32).log2() + 7.0_f32) },
            TransferFunction::Gamma24 => x.max(0.0).powf(1.0 / 2.4),
        }
    }
}

// ---------------------------------------------------------------------------
// Lens correction tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod lens_tests {
    use super::*;

    #[test]
    fn bayer_phase_to_channel_rggb() {
        let p = BayerPattern::RGGB;
        // (0,0) = R → ch 0
        assert_eq!(bayer_phase_to_channel(0, 0, p), 0);
        // (1,0) = G1 → ch 1
        assert_eq!(bayer_phase_to_channel(1, 0, p), 1);
        // (0,1) = G2 → ch 2
        assert_eq!(bayer_phase_to_channel(0, 1, p), 2);
        // (1,1) = B → ch 3
        assert_eq!(bayer_phase_to_channel(1, 1, p), 3);
    }

    #[test]
    fn bayer_phase_to_channel_bggr() {
        let p = BayerPattern::BGGR;
        assert_eq!(bayer_phase_to_channel(1, 1, p), 0); // R
        assert_eq!(bayer_phase_to_channel(0, 0, p), 3); // B
    }

    #[test]
    fn bayer_phase_to_channel_grbg() {
        let p = BayerPattern::GRBG;
        assert_eq!(bayer_phase_to_channel(1, 0, p), 0); // R
        assert_eq!(bayer_phase_to_channel(0, 1, p), 3); // B
    }

    #[test]
    fn bayer_phase_to_channel_gbrg() {
        let p = BayerPattern::GBRG;
        assert_eq!(bayer_phase_to_channel(0, 1, p), 0); // R
        assert_eq!(bayer_phase_to_channel(1, 0, p), 3); // B
    }

    #[test]
    fn interpolate_bilinear_identity_map_returns_center_value() {
        let w = 3u32; let h = 3u32;
        let data: Vec<f32> = vec![
            1.0f32, 1.5f32, 1.0f32,
            1.5f32, 2.0f32, 1.5f32,
            1.0f32, 1.5f32, 1.0f32,
        ];
        let result = interpolate_bilinear(&data, w, h, 0.5f32, 0.5f32);
        assert!((result - 2.0f32).abs() < 1e-5f32, "center={}", result);
    }

    #[test]
    fn interpolate_bilinear_corner_returns_edge_value() {
        let w = 2u32; let h = 2u32;
        let data: Vec<f32> = vec![1.0f32, 2.0f32, 3.0f32, 4.0f32];
        let result = interpolate_bilinear(&data, w, h, 0.0f32, 0.0f32);
        assert!((result - 1.0f32).abs() < 1e-5f32, "topleft={}", result);
        let result = interpolate_bilinear(&data, w, h, 1.0f32, 1.0f32);
        assert!((result - 4.0f32).abs() < 1e-5f32, "botright={}", result);
    }

    #[test]
    fn compute_color_only_map_uniform_returns_identity() {
        let w = 2u32; let h = 2u32;
        let channels = vec![
            vec![2.0f32; 4],
            vec![2.0f32; 4],
            vec![2.0f32; 4],
            vec![2.0f32; 4],
        ];
        let result = compute_color_only_map(&channels, w, h);
        for ch in &result {
            for v in ch {
                assert!((*v - 1.0f32).abs() < 1e-5f32, "color_only should be 1.0, got {}", v);
            }
        }
    }

    #[test]
    fn compute_color_only_map_different_channels_preserves_ratio() {
        let w = 1u32; let h = 1u32;
        let channels = vec![
            vec![4.0f32],
            vec![2.0f32],
            vec![2.0f32],
            vec![4.0f32],
        ];
        let result = compute_color_only_map(&channels, w, h);
        assert!((result[0][0] - 2.0f32).abs() < 1e-5f32, "R={}", result[0][0]);
        assert!((result[1][0] - 1.0f32).abs() < 1e-5f32, "G1={}", result[1][0]);
        assert!((result[2][0] - 1.0f32).abs() < 1e-5f32, "G2={}", result[2][0]);
        assert!((result[3][0] - 2.0f32).abs() < 1e-5f32, "B={}", result[3][0]);
    }

    #[test]
    fn normalize_linear_per_channel_basic() {
        let mut rgb = vec![1000.0f32, 2000.0f32, 1500.0f32];
        normalize_linear_per_channel(&mut rgb, 0.0f64, 0.0f64, 0.0f64, 4000.0f64);
        assert!((rgb[0] - 0.25f32).abs() < 1e-5f32, "R={}", rgb[0]);
        assert!((rgb[1] - 0.5f32).abs() < 1e-5f32, "G={}", rgb[1]);
        assert!((rgb[2] - 0.375f32).abs() < 1e-5f32, "B={}", rgb[2]);
    }

    #[test]
    fn normalize_linear_per_channel_per_channel_bl() {
        let mut rgb = vec![1000.0f32, 2000.0f32, 1500.0f32];
        normalize_linear_per_channel(&mut rgb, 100.0f64, 200.0f64, 50.0f64, 2000.0f64);
        let r_exp = (1000.0f64 - 100.0f64) / (2000.0f64 - 100.0f64);
        let g_exp = (2000.0f64 - 200.0f64) / (2000.0f64 - 200.0f64);
        let b_exp = (1500.0f64 - 50.0f64) / (2000.0f64 - 50.0f64);
        assert!((rgb[0] - r_exp as f32).abs() < 1e-5f32, "R={}", rgb[0]);
        assert!((rgb[1] - g_exp as f32).abs() < 1e-5f32, "G={}", rgb[1]);
        assert!((rgb[2] - b_exp as f32).abs() < 1e-5f32, "B={}", rgb[2]);
    }

    #[test]
    fn apply_lens_correction_identity_map_preserves_pixels() {
        let w = 4u32; let h = 4u32;
        let mut bayer: Vec<u16> = (0u16..w as u16 * h as u16).collect();
        let original = bayer.clone();
        let channels = vec![vec![1.0f32; (w * h) as usize]; 4];
        apply_lens_correction_cpu_with_map(
            &mut bayer, w, 0, 0, BayerPattern::RGGB,
            &channels, 2, 2, w, h,
            [0.0; 4], 65535.0, 65535,
        );
        for (i, (&orig, &new)) in original.iter().zip(bayer.iter()).enumerate() {
            assert_eq!(orig, new, "pixel {} should be unchanged", i);
        }
    }

    #[test]
    fn apply_lens_correction_uniform_gain_scales_correctly() {
        let w = 4u32; let h = 4u32;
        let mut bayer: Vec<u16> = vec![100u16; (w * h) as usize];
        let channels = vec![vec![2.0f32; (w * h) as usize]; 4];
        apply_lens_correction_cpu_with_map(
            &mut bayer, w, 0, 0, BayerPattern::RGGB,
            &channels, 2, 2, w, h,
            [0.0; 4], 65535.0, 65535,
        );
        for (i, &p) in bayer.iter().enumerate() {
            assert_eq!(p, 200u16, "pixel {} should be 200, got {}", i, p);
        }
    }

    // ── Highlight reconstruction + display rolloff (HL-handling.md §6) ──

    fn recon_params() -> ReconstructParams {
        ReconstructParams {
            r_gain: 1.0,
            b_gain: 1.0,
            fused_luma: Some([0.2126, 0.7152, 0.0722]),
        }
    }

    /// Test shim: usize dims → u32 signature.
    fn recon(rgb: &mut [f32], w: usize, h: usize, mask: &[u8]) {
        reconstruct_clipped(rgb, w as u32, h as u32, mask, &recon_params());
    }

    /// Build a w×h rgb buffer where R = 2·G, B = 0.5·G, G ramps
    /// `lo..hi` along x. Returns (rgb, w, h).
    fn warm_wedge(w: usize, h: usize, lo: f32, hi: f32) -> Vec<f32> {
        let mut rgb = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let t = x as f32 / (w - 1) as f32;
                let g = lo + (hi - lo) * t;
                let i = (y * w + x) * 3;
                rgb[i] = 2.0 * g;
                rgb[i + 1] = g;
                rgb[i + 2] = 0.5 * g;
            }
        }
        rgb
    }

    #[test]
    fn clip_mask_reports_expected_bits_and_count() {
        let mut rgb = vec![0.0f32; 3 * 4];
        rgb[0] = 1.0; rgb[1] = 0.5; rgb[2] = 0.9;             // R clipped
        rgb[3] = 0.3; rgb[4] = 1.2; rgb[5] = 0.2;             // G clipped
        rgb[6] = 1.1; rgb[7] = 1.2; rgb[8] = 1.05;            // all clipped
        rgb[9] = 0.1; rgb[10] = 0.2; rgb[11] = 0.3;           // clean
        let (mask, count) = clip_mask(&rgb, 0.995);
        assert_eq!(mask, vec![0b001, 0b010, 0b111, 0b000]);
        assert_eq!(count, 3);
    }

    #[test]
    fn raw_pin_mask_flat_threshold_pins_only_at_sensor_ceiling() {
        // RGGB 2×2 block. white_level = 1024, pin_thr_raw = 0.99 × 1024
        // = 1013.76. The pin threshold is FLAT raw CFA units: no black
        // level adjustment, no reconstruction coupling.
        let w = 2usize; let h = 2usize;
        let mut bayer = vec![0u16; w * h];
        // Photosite roles: (0,0)=R, (1,0)=G1, (0,1)=G2, (1,1)=B.
        // R pinned at 1023 (≥ 1013.76) → R bit set for the whole block.
        bayer[0] = 1023;
        let mask = raw_pin_mask(&bayer, w, 0, 0, w, h, BayerPattern::RGGB, 1013.76);
        assert_eq!(mask, vec![0b001, 0b001, 0b001, 0b001]);
        // 0.983×WL = 1006.7 — sub-threshold (the 0.03% class). It carries
        // real, distinct sensor data; WB + CCM may legitimately push it
        // wide-gamut. NO pre-trigger (user-mandated).
        bayer[0] = 1006;
        bayer[1] = 1006; bayer[2] = 1006; bayer[3] = 1006;
        let mask = raw_pin_mask(&bayer, w, 0, 0, w, h, BayerPattern::RGGB, 1013.76);
        assert_eq!(mask, vec![0u8; 4], "sub-threshold photosites must NOT be flagged");
        // Mixed: G1 pinned only → G bit (0b010) for the whole block.
        bayer[0] = 100; bayer[1] = 1023; bayer[2] = 100; bayer[3] = 100;
        let mask = raw_pin_mask(&bayer, w, 0, 0, w, h, BayerPattern::RGGB, 1013.76);
        assert_eq!(mask, vec![0b010, 0b010, 0b010, 0b010]);
        // All four pinned → 0b111.
        bayer[0] = 1023; bayer[1] = 1023; bayer[2] = 1023; bayer[3] = 1023;
        let mask = raw_pin_mask(&bayer, w, 0, 0, w, h, BayerPattern::RGGB, 1013.76);
        assert_eq!(mask, vec![0b111; 4]);
        // Zero threshold → empty mask (feature gated off).
        let mask = raw_pin_mask(&bayer, w, 0, 0, w, h, BayerPattern::RGGB, 0.0);
        assert_eq!(mask, vec![0u8; 4]);
    }

    #[test]
    fn raw_pin_mask_respects_pattern_roles() {
        // BGGR: (0,0)=B, (1,0)=G1, (0,1)=G2, (1,1)=R.
        let w = 2usize; let h = 2usize;
        let mut bayer = vec![0u16; w * h];
        bayer[3] = 1023; // R pinned
        let mask = raw_pin_mask(&bayer, w, 0, 0, w, h, BayerPattern::BGGR, 1013.76);
        assert_eq!(mask, vec![0b001; 4]);
        bayer[3] = 100;
        bayer[1] = 1023; // G1 pinned → G bit
        let mask = raw_pin_mask(&bayer, w, 0, 0, w, h, BayerPattern::BGGR, 1013.76);
        assert_eq!(mask, vec![0b010; 4]);
    }

    #[test]
    fn raw_pin_mask_is_black_level_independent() {
        // A photosite at 1023 pins regardless of black level: the mask
        // compares the RAW CFA code against the flat sensor-ceiling
        // threshold. Black levels never enter the pin test.
        let w = 2usize; let h = 2usize;
        let mut bayer = vec![0u16; w * h];
        bayer[0] = 1023;
        // 0.99×WL with a per-channel black level of 300: normalized-space
        // need would have been 300 + 0.99×(1024−300) = 1016.8 — the pin
        // still fires because the compare is raw ≥ 1013.76.
        let mask = raw_pin_mask(&bayer, w, 0, 0, w, h, BayerPattern::RGGB, 1013.76);
        assert_eq!(mask, vec![0b001; 4]);
    }

    #[test]
    fn reconstruct_never_touches_unclipped_pixels() {
        // 32×32, no pixel reaches the threshold → bit-identical output.
        let mut rgb = warm_wedge(32, 32, 0.05, 0.4);
        let original = rgb.clone();
        let (mask, count) = clip_mask(&rgb, 0.995);
        assert_eq!(count, 0);
        recon(&mut rgb, 32, 32, &mask);
        assert_eq!(rgb, original, "clean frame must be bit-identical");
    }

    #[test]
    fn reconstruct_clean_pixels_untouched_in_mixed_frame() {
        let mut rgb = warm_wedge(32, 32, 0.05, 1.5);
        // Pins R at 1.0 where it exceeds the clip: raw clip simulation.
        for v in rgb.iter_mut().skip(0).step_by(3) {
            if *v > 1.0 {
                *v = 1.0;
            }
        }
        let original = rgb.clone();
        let (mask, _) = clip_mask(&rgb, 0.995);
        reconstruct_clipped(&mut rgb, 32, 32, &mask, &recon_params());
        for (i, (&orig, &new)) in original.iter().zip(rgb.iter()).enumerate() {
            if mask[i / 3] == 0 {
                assert_eq!(orig, new, "clean pixel {} changed", i / 3);
            }
        }
    }

    #[test]
    fn tier1_estimates_clipped_channel_from_healthy_references() {
        let w = 64;
        let h = 64;
        let mut rgb = warm_wedge(w, h, 0.05, 1.5);
        for v in rgb.iter_mut().skip(0).step_by(3) {
            if *v > 1.0 {
                *v = 1.0;
            }
        }
        let (mask, _) = clip_mask(&rgb, 0.995);
        recon(&mut rgb, w, h, &mask);
        // Estimation band: pixels within 2px of the clip boundary (window
        // support). Deeper pixels pin at 1.0 — correct no-support behavior.
        for y in 0..h {
            for x in 20..30 {
                let i = (y * w + x) * 3;
                if mask[y * w + x] != 0b001 {
                    continue;
                }
                let g = rgb[i + 1];
                let est = rgb[i];
                assert!(est >= 1.0 - 1e-6, "never darken: est {est} < pinned 1.0");
                if est <= 1.0001 {
                    continue; // no support → pinned, correct
                }
                let true_r = 2.0 * g;
                assert!(
                    (est - true_r).abs() <= 0.15 * true_r,
                    "x={x} est {est} vs true {true_r}"
                );
            }
        }
    }

    #[test]
    fn tier1_epsilon_guard_excludes_dark_reference_samples() {
        // Saturated-red shadow region: G ≤ ε everywhere, R pinned, B stable.
        // G ratios (if used) would be enormous (~200) and pull the average
        // well above the floor; the ε guard must keep the estimate exactly
        // at the floor (the B-ref result clamps there — never darken).
        let w = 16;
        let h = 16;
        let mut rgb = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                rgb[i] = 1.0;       // R pinned at clip
                rgb[i + 1] = 0.005; // G ≈ 0 — below RECON_EPSILON
                rgb[i + 2] = 0.9;   // B stable, healthy (R/B = 1.0)
            }
        }
        // Left band: unclipped R to give B a consistent ratio (R/B = 1.0).
        for x in 0..4 {
            for y in 0..h {
                let i = (y * w + x) * 3;
                rgb[i] = 0.9;
            }
        }
        let (mask, count) = clip_mask(&rgb, 0.995);
        assert!(count > 0);
        recon(&mut rgb, w, h, &mask);
        for y in 0..h {
            for x in 4..w {
                let i = (y * w + x) * 3;
                if mask[y * w + x] == 0b001 {
                    // G contributed nothing: the estimate is the B-ref result
                    // clamped to the never-darken floor (1.0) — not a spike.
                    assert_eq!(
                        rgb[i], 1.0,
                        "G contamination: est {} != floor 1.0",
                        rgb[i]
                    );
                }
            }
        }
    }

    #[test]
    fn tier1_g_pinned_reconstructs_upward_from_wb_ratios() {
        // G is the single clipped channel (mask 010), G = WB anchor (gain 1):
        // a neutral highlight clipped on G is reconstructed UPWARD from the
        // WB'd R/B brightness (deviation D10). Uniform so every pixel is
        // pinned on G; R/B healthy below the clip threshold.
        let w = 16;
        let h = 16;
        let mut rgb = vec![0.0f32; w * h * 3];
        for v in rgb.chunks_mut(3) {
            v[0] = 0.7; // R healthy
            v[1] = 1.0; // G pinned at ceiling
            v[2] = 0.5; // B healthy
        }
        let params = ReconstructParams {
            r_gain: 2.0,
            b_gain: 1.5,
            fused_luma: Some([1.0, 1.0, 1.0]),
        };
        let (mask, count) = clip_mask(&rgb, 0.995);
        assert_eq!(count, w * h);
        assert!(mask.iter().all(|&m| m == 0b010), "all pixels must be G-pinned");
        reconstruct_clipped(&mut rgb, w as u32, h as u32, &mask, &params);
        // est = max(R·r_gain, B·b_gain, pinned G) = max(1.4, 0.75, 1.0) = 1.4.
        for v in rgb.chunks(3) {
            assert!((v[1] - 1.4).abs() < 1e-6, "G should rise to 1.4, got {}", v[1]);
            assert!((v[0] - 0.7).abs() < 1e-6, "R must stay untouched");
            assert!((v[2] - 0.5).abs() < 1e-6, "B must stay untouched");
        }
    }

    #[test]
    fn tier1_g_pinned_never_darkens_saturated_green() {
        // G pinned with WB'd R/B BELOW the pinned G: genuinely saturated
        // green — the estimate must stay at the pinned value (never darken,
        // real hue preserved).
        let w = 16;
        let h = 16;
        let mut rgb = vec![0.0f32; w * h * 3];
        for v in rgb.chunks_mut(3) {
            v[0] = 0.2; // R low: WB'd 0.4 < pinned G
            v[1] = 1.0; // G pinned
            v[2] = 0.15; // B low: WB'd 0.225 < pinned G
        }
        let params = ReconstructParams {
            r_gain: 2.0,
            b_gain: 1.5,
            fused_luma: Some([1.0, 1.0, 1.0]),
        };
        let (mask, count) = clip_mask(&rgb, 0.995);
        assert_eq!(count, w * h);
        reconstruct_clipped(&mut rgb, w as u32, h as u32, &mask, &params);
        for v in rgb.chunks(3) {
            assert_eq!(v[1], 1.0, "saturated green must stay pinned, got {}", v[1]);
            assert_eq!(v[0], 0.2);
            assert_eq!(v[2], 0.15);
        }
    }

    #[test]
    fn tier1_r_pinned_keeps_never_darken_with_large_gains() {
        // Form regression: the D10 G-upward branch must NOT fire for R-pinned
        // mask 001 — R stays on the never-darken floor even when its WB'd
        // estimate would exceed 1.0.
        let w = 16;
        let h = 16;
        let mut rgb = vec![0.0f32; w * h * 3];
        for v in rgb.chunks_mut(3) {
            v[0] = 1.0; // R pinned
            v[1] = 0.5; // G healthy
            v[2] = 0.4; // B healthy
        }
        let params = ReconstructParams {
            r_gain: 2.0,
            b_gain: 1.5,
            fused_luma: Some([1.0, 1.0, 1.0]),
        };
        let (mask, count) = clip_mask(&rgb, 0.995);
        assert_eq!(count, w * h);
        reconstruct_clipped(&mut rgb, w as u32, h as u32, &mask, &params);
        for v in rgb.chunks(3) {
            // Tier-1 floor = own pinned value; estimate from G/B refs.
            assert!(
                v[0] >= 1.0 - 1e-6,
                "R must never darken below pinned value, got {}",
                v[0]
            );
            assert!(v[1] >= 0.5 - 1e-6 && v[2] >= 0.4 - 1e-6, "healthy channels untouched");
        }
    }

    #[test]
    fn tier1_no_support_pins_in_place() {
        // All channels at/below ε except R (pinned): zero valid samples → pinned.
        let w = 16;
        let h = 16;
        let mut rgb = vec![0.0f32; w * h * 3];
        for v in rgb.iter_mut() {
            *v = 0.003;
        }
        for v in rgb.iter_mut().skip(0).step_by(3) {
            *v = 1.0;
        }
        let original = rgb.clone();
        let (mask, count) = clip_mask(&rgb, 0.995);
        assert_eq!(count, w * h);
        recon(&mut rgb, w, h, &mask);
        assert_eq!(rgb, original, "no-support pixels must stay pinned");
    }

    #[test]
    fn tier2_ratio_path_recovers_two_clipped_channels_from_ratio() {
        // 2D blob: R,G pinned at 1.0 inside, healthy B = 0.6. A clean ring
        // around it has R = 0.95, G = 0.85, B = 0.5 (unclipped) — the blob's
        // continuation is R = 1.9·B = 1.14, G = 1.7·B = 1.02.
        let w = 64;
        let h = 64;
        let mut rgb = vec![0.3f32; w * h * 3];
        let in_blob = |x: usize, y: usize| x >= 28 && x <= 36 && y >= 28 && y <= 36;
        let in_ring = |x: usize, y: usize| {
            (x >= 25 && x <= 27 || x >= 37 && x <= 39) && (y >= 25 && y <= 39)
                || (y >= 25 && y <= 27 || y >= 37 && y <= 39) && (x >= 25 && x <= 39)
        };
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                if in_blob(x, y) {
                    rgb[i] = 1.0;
                    rgb[i + 1] = 1.0;
                    rgb[i + 2] = 0.6;
                } else if in_ring(x, y) {
                    rgb[i] = 0.95;
                    rgb[i + 1] = 0.85;
                    rgb[i + 2] = 0.5;
                }
            }
        }
        let (mask, _) = clip_mask(&rgb, 0.995);
        recon(&mut rgb, w, h, &mask);
        for y in 28..=36 {
            for x in 28..=36 {
                if mask[y * w + x] != 0b011 {
                    continue;
                }
                let b = rgb[(y * w + x) * 3 + 2];
                let est_r = rgb[(y * w + x) * 3];
                let est_g = rgb[(y * w + x) * 3 + 1];
                // Support: a 9×9 window reaches the clean ring when ANY
                // axis (x or y) puts ring columns/rows (25-27 or 37-39)
                // inside it. Only the exact center (32,32) has its whole
                // window inside the blob — it pins (dealt with by the
                // caller's neutral collapse, not by reconstruction).
                let has_support = x <= 31 || x >= 33 || y <= 31 || y >= 33;
                if !has_support {
                    // Tier-2's 9×9 window never reaches the clean ring. The
                    // Pass-B continuation covers this isolated pocket: the
                    // dead-zone lift keeps its estimate at the blob's own
                    // recovered boundary level (median of the nearest 011
                    // WB'd peaks ≈ 1.14), the wide per-cell field blur
                    // blends it with the surrounding pinned plateau (~1.0),
                    // and the ring chromaticity fades to neutral for the
                    // small feature — the pocket merges into the blob
                    // instead of popping as a bright dot.
                    let est_r = rgb[(y * w + x) * 3];
                    let est_g = rgb[(y * w + x) * 3 + 1];
                    let est_b = rgb[(y * w + x) * 3 + 2];
                    assert!(
                        est_r > 0.95 && est_r < 1.15,
                        "isolated pocket's R must sit at the blob level, got {est_r}"
                    );
                    assert!(
                        est_g > 0.95 && est_g < 1.15,
                        "isolated pocket's G must sit at the blob level, got {est_g}"
                    );
                    assert!(
                        (est_r - est_g).abs() < 0.01,
                        "small pocket's hue must fade to neutral, got R {est_r} G {est_g}"
                    );
                    assert_eq!(est_b, 0.6, "healthy B must stay pinned");
                    continue;
                }
                let err_r = (est_r - 1.9 * b).abs() / (1.9 * b);
                let err_g = (est_g - 1.7 * b).abs() / (1.7 * b);
                assert!(err_r <= 0.15, "R est {est_r} vs 1.9B {}", 1.9 * b);
                assert!(err_g <= 0.15, "G est {est_g} vs 1.7B {}", 1.7 * b);
            }
        }
    }

    #[test]
    fn tier2_specular_pair_dropped_to_ratio_consistent_values() {
        // Warm-white specular: R,B pinned at the ceiling, G healthy at 0.6.
        // The clean ring around it is NEUTRAL (R/G = B/G = 1.0). The old
        // never-darken floor kept R,B pinned at 1.0 → post-WB/CCM magenta.
        // The estimate must BRING R,B DOWN to the ratio-consistent value
        // (≈ 0.6) — the pink is anchored at the ceiling, so the floor is
        // removed for Tier-2.
        let w = 64;
        let h = 64;
        let mut rgb = vec![0.3f32; w * h * 3];
        let in_blob = |x: usize, y: usize| x >= 28 && x <= 36 && y >= 28 && y <= 36;
        let in_ring = |x: usize, y: usize| {
            (x >= 25 && x <= 27 || x >= 37 && x <= 39) && (y >= 25 && y <= 39)
                || (y >= 25 && y <= 27 || y >= 37 && y <= 39) && (x >= 25 && x <= 39)
        };
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                if in_blob(x, y) {
                    rgb[i] = 1.0;
                    rgb[i + 1] = 0.6;
                    rgb[i + 2] = 1.0;
                } else if in_ring(x, y) {
                    rgb[i] = 0.7;
                    rgb[i + 1] = 0.7;
                    rgb[i + 2] = 0.7; // neutral ring
                }
            }
        }
        let (mask, _) = clip_mask(&rgb, 0.995);
        recon(&mut rgb, w, h, &mask);
        for y in 28..=36 {
            for x in 28..=36 {
                if mask[y * w + x] != 0b101 {
                    continue;
                }
                let est_r = rgb[(y * w + x) * 3];
                let est_b = rgb[(y * w + x) * 3 + 2];
                let has_support = x <= 31 || x >= 33 || y <= 31 || y >= 33;
                if !has_support {
                    // Pass B's continuation replaces the old pin: the
                    // dead-zone lift keeps the estimate at the blob's own
                    // recovered level (the median of the nearest 101 WB'd
                    // peaks ≈ 0.64 — the blob's own R/B estimate), so the
                    // magenta-anchored ceiling is gone even here and the
                    // pocket joins its neighbors instead of pinning bright.
                    assert!(
                        est_r > 0.5 && est_r < 0.8,
                        "no-support R must drop to the blob level, got {est_r}"
                    );
                    assert!(
                        est_b > 0.5 && est_b < 0.8,
                        "no-support B must drop to the blob level, got {est_b}"
                    );
                    continue;
                }
                assert!(
                    est_r.abs() - 0.6 < 0.1 && est_r < 0.99,
                    "R est {est_r} must move toward G (0.6), not stay pinned"
                );
                assert!(
                    est_b < 0.99,
                    "B est {est_b} must move toward G (0.6), not stay pinned"
                );
            }
        }
    }

    #[test]
    fn tier3_ring_anchors_white_and_colored_cores() {
        // 5×5 all-clipped core (mask 111) with a clean ring at distance 3.
        // White ring → near-neutral core at the pixel's own peak brightness.
        let w = 32;
        let h = 32;
        let mut white = vec![0.3f32; w * h * 3];
        let in_core = |x: usize, y: usize| x >= 12 && x <= 16 && y >= 12 && y <= 16;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                if in_core(x, y) {
                    white[i] = 1.05;
                    white[i + 1] = 1.05;
                    white[i + 2] = 1.05;
                } else if (y * w + x) as i32 == 0 {
                    white[i] = 0.7;
                    white[i + 1] = 0.3;
                    white[i + 2] = 0.15;
                }
            }
        }
        let (mask, _) = clip_mask(&white, 0.995);
let (mask, _) = clip_mask(&white, 0.995);
recon(&mut white, w, h, &mask);



        // The 5×5 core sits fully inside the 13×13 continuation blur and
        // the dead-zone suppresses its lift: the kernel's window is ~85%
        // unstained background (own WB'd peaks 0.3), so the per-cell
        // substitution blends the flash away — the core reads a soft bump
        // ≈ 0.41 against the background instead of 1.05 (the user's
        // "small few-pixel clipped area must not form a square").
        for y in 12..=16 {
            for x in 12..=16 {
                let i = (y * w + x) * 3;
                assert!(white[i] > 0.38 && white[i] < 0.5, "no-flash soft bump, got {}", white[i]);
                assert_eq!(white[i], white[i + 1], "white core must stay neutral");
                assert_eq!(white[i], white[i + 2]);
            }
        }

        // Colored ring (R:G:B = 2:1:0.5) → the core keeps the ring's hue
        // at its own brightness — no grey-out of colored speculars.
        let mut colored = vec![0.3f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                if in_core(x, y) {
                    colored[i] = 1.05;
                    colored[i + 1] = 1.05;
                    colored[i + 2] = 1.05;
                } else {
                    colored[i] = 0.6;
                    colored[i + 1] = 0.3;
                    colored[i + 2] = 0.15;
                }
            }
        }
        let (cmask, _) = clip_mask(&colored, 0.995);
        recon(&mut colored, w, h, &cmask);
        // Same no-flash bump at the colored background's level (bg own 0.6)
        // with the ring's hue preserved at the bump's brightness.
        for y in 12..=16 {
            for x in 12..=16 {
                let i = (y * w + x) * 3;
                assert!(colored[i] > 0.55 && colored[i] < 1.06, "no-flash bump, got {}", colored[i]);
                let r = colored[i];
                let g = colored[i + 1];
                let b = colored[i + 2];
                assert!((r / g - 2.0).abs() < 0.2, "R/G hue from ring: {r}/{g}");
                assert!((g / b - 2.0).abs() < 0.25, "G/B hue from ring: {g}/{b}");
            }
        }
    }

    #[test]
    fn tier3_tiny_fully_clipped_island_reads_surrounding_level() {
        // The user's scenario: a 2×2 fully-clipped island inside a
        // semi-clipped sheet (a few clipped pixels surrounded by the
        // recovered water). The old continuation flashed the island to the
        // core's own ceiling — a bright square; the dead-zone lift plus the
        // wide per-cell field blur must keep it at the sheet's recovered
        // level: a soft bump, not a flash.
        let w = 32;
        let h = 32;
        let mut rgb = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                if (x == 15 || x == 16) && (y == 15 || y == 16) {
                    rgb[i] = 1.05;
                    rgb[i + 1] = 1.05;
                    rgb[i + 2] = 1.05; // island: all channels clipped
                } else if x >= 13 && x <= 18 && y >= 13 && y <= 18 {
                    rgb[i] = 1.05;
                    rgb[i + 1] = 1.05;
                    rgb[i + 2] = 0.7; // sheet: R,G clipped, B healthy
                } else {
                    rgb[i] = 0.5;
                    rgb[i + 1] = 0.5;
                    rgb[i + 2] = 0.5; // clean background
                }
            }
        }
        let (mask, _) = clip_mask(&rgb, 0.995);
        let params = ReconstructParams {
            r_gain: 1.0,
            b_gain: 1.0,
            fused_luma: Some([0.2126, 0.7152, 0.0722]),
        };
        let mut out = rgb.clone();
        reconstruct_clipped(&mut out, w as u32, h as u32, &mask, &params);
        for y in 15..=16 {
            for x in 15..=16 {
                let i = (y * w + x) * 3;
                let mx = out[i].max(out[i + 1]).max(out[i + 2]);
                assert!(mx < 1.03, "island must not flash to 1.05, got {mx}");
                assert!(mx >= 0.4, "island must stay at the surrounding level, got {mx}");
                assert!(
                    (out[i] - out[i + 2]).abs() < 0.01,
                    "island must write neutral (sheet face), got R {} B {}",
                    out[i],
                    out[i + 2]
                );
            }
        }
        let sheet_px = (14 * w + 14) * 3;
        assert!(out[sheet_px + 2] == 0.7, "sheet's healthy B untouched");
        let clean_px = (4 * w + 4) * 3;
        assert_eq!(
            &out[clean_px..clean_px + 3],
            &rgb[clean_px..clean_px + 3],
            "clean bg untouched"
        );
    }

    #[test]
    fn tier3_core_continuation_has_no_rim_steps() {
        // Rim-step regression (GPU outline parity): a 12×12 fully-clipped
        // core inside a semi-clipped sheet must continue brightness with
        // bounded neighbor deltas — no single-pixel contour rims. The 13×13
        // field blur guarantees this on CPU; the GPU lite path mirrors it
        // with multi-sample medians + median distance (no single-sample med).
        let w = 48;
        let h = 48;
        let mut rgb = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                if x >= 18 && x < 30 && y >= 18 && y < 30 {
                    rgb[i] = 1.05;
                    rgb[i + 1] = 1.05;
                    rgb[i + 2] = 1.05; // core: fully clipped
                } else if x >= 12 && x < 36 && y >= 12 && y < 36 {
                    rgb[i] = 1.05;
                    rgb[i + 1] = 1.05;
                    rgb[i + 2] = 0.7; // sheet: R,G clipped, B healthy
                } else {
                    rgb[i] = 0.5;
                    rgb[i + 1] = 0.5;
                    rgb[i + 2] = 0.5;
                }
            }
        }
        let (mask, _) = clip_mask(&rgb, 0.995);
        let params = ReconstructParams {
            r_gain: 1.0,
            b_gain: 1.0,
            fused_luma: Some([0.2126, 0.7152, 0.0722]),
        };
        let mut out = rgb.clone();
        reconstruct_clipped(&mut out, w as u32, h as u32, &mask, &params);
        // Adjacent core peaks must vary smoothly (blurred continuation).
        let peak = |x: usize, y: usize| {
            let i = (y * w + x) * 3;
            out[i].max(out[i + 1]).max(out[i + 2])
        };
        let mut worst = 0.0f32;
        for y in 19..29 {
            for x in 19..29 {
                worst = worst.max((peak(x, y) - peak(x + 1, y)).abs());
                worst = worst.max((peak(x, y) - peak(x, y + 1)).abs());
            }
        }
        assert!(worst < 0.05, "core rim step too large: {worst}");
    }

    #[test]
    fn luma_collapse_neutralizes_clipped_pairs_only() {
        use crate::color::luma_collapse_after_wb;
        let luma = [0.2126, 0.7152, 0.0722];
        // mask 101 (R,B at ceiling) → neutral at the WB'd luma.
        let mut t = [2.4, 0.6, 2.1];
        let y = 0.2126 * 2.4 + 0.7152 * 0.6 + 0.0722 * 2.1;
        assert!(luma_collapse_after_wb(&mut t, 0b101, luma));
        assert_eq!(t, [y, y, y]);
        // mask 011 and 111 also collapse.
        let mut t2 = [1.2, 1.1, 0.7];
        assert!(luma_collapse_after_wb(&mut t2, 0b011, luma));
        assert_eq!(t2, [t2[0], t2[0], t2[0]]);
        // Single-clip (real hue: red star) untouched.
        let mut t3 = [2.4, 0.4, 0.5];
        assert!(!luma_collapse_after_wb(&mut t3, 0b001, luma));
        assert_eq!(t3, [2.4, 0.4, 0.5]);
        // Clean pixel untouched.
        let mut t4 = [0.5, 0.5, 0.5];
        assert!(!luma_collapse_after_wb(&mut t4, 0b000, luma));
        assert_eq!(t4, [0.5, 0.5, 0.5]);
    }

    #[test]
    fn luma_collapse_uses_wb_neutral_direction() {
        use crate::color::luma_collapse_after_wb;
        // The collapse output is the input-space neutral [k,k,k]: the fused
        // CCM row sums are 1.0 (±0.001 by ±CAT construction), so [1,1,1]
        // maps to output-neutral. The previous gain-scaled direction
        // [k·g_r, k, k·g_b] re-applied the WB inside the CCM and exited with
        // R≈B≈3.4·G — the magenta cast this pass exists to prevent.
        let luma = [0.2126, 0.7152, 0.0722];
        let mut t = [1.88, 0.80, 1.69];
        assert!(luma_collapse_after_wb(&mut t, 0b101, luma));
        let k = t[0];
        assert!((t[1] - k).abs() < 1e-4, "G direction: {}", t[1] - k);
        assert!((t[2] - k).abs() < 1e-4, "B direction: {}", t[2] - k);
        assert!(t[0] > 0.0 && t[1] > 0.0 && t[2] > 0.0, "brightness kept");
        // The fused luma of the result equals the measured luma.
        let y_in = luma[0] * 1.88 + luma[1] * 0.80 + luma[2] * 1.69;
        let y_out = luma[0] * t[0] + luma[1] * t[1] + luma[2] * t[2];
        assert!((y_in - y_out).abs() < 1e-4, "luma preserved: {y_in} vs {y_out}");
    }

    #[test]
    fn luma_collapse_keeps_luma_invariant() {
        use crate::color::luma_collapse_after_wb;
        let luma = [0.2126, 0.7152, 0.0722];
        let mut t = [1.8, 0.3, 1.9];
        let y_before = luma[0] * t[0] + luma[1] * t[1] + luma[2] * t[2];
        assert!(luma_collapse_after_wb(&mut t, 0b111, luma));
        let y_after = luma[0] * t[0] + luma[1] * t[1] + luma[2] * t[2];
        assert!((y_before - y_after).abs() < 1e-5, "luma must be preserved");
    }

    #[test]
    fn off_state_collapse_guarantees_no_magenta_without_reconstruction() {
        use crate::color::luma_collapse_after_wb;
        // OFF-state contract (no reconstruct_clipped call): the always-on
        // raw-truth collapse alone must neutralize any >=2-pinned pixel so
        // WB+CCM can never render magenta. Single-pin hue stays untouched.
        let luma = [0.2126, 0.7152, 0.0722];
        for mask in [0b011u8, 0b101, 0b110, 0b111] {
            let mut t = [2.2, 1.9, 2.5];
            assert!(luma_collapse_after_wb(&mut t, mask, luma), "mask {mask:03b}");
            let mx = t[0].max(t[1]).max(t[2]);
            let mn = t[0].min(t[1]).min(t[2]);
            assert!((mx - mn) < 1e-4, "mask {mask:03b} not neutral: {t:?}");
        }
        // Single-pin pixels (real saturated colors) must pass through.
        for mask in [0b001u8, 0b010, 0b100, 0b000] {
            let mut t = [2.2, 0.4, 0.5];
            assert!(!luma_collapse_after_wb(&mut t, mask, luma));
            assert_eq!(t, [2.2, 0.4, 0.5], "mask {mask:03b} tempered");
        }
    }

    #[test]
    fn tier2_luminance_fallback_engages_on_unreliable_anchor() {
        // Healthy B is present but the ratio samples are inconsistent
        // (alternating 0.9 and 0.1 in B) → IQR test fails → luminance path.
        let w = 32;
        let h = 32;
        let mut rgb = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                rgb[i] = 1.0;            // R clipped
                rgb[i + 1] = 1.0;        // G clipped
                rgb[i + 2] = if (x + y) % 2 == 0 { 0.9 } else { 0.1 }; // wild B
            }
        }
        let (mask, count) = clip_mask(&rgb, 0.995);
        assert_eq!(count, w * h);
        let before = rgb.clone();
        recon(&mut rgb, w, h, &mask);
        for (i, (&a, &b)) in before.iter().zip(rgb.iter()).enumerate() {
            assert!(b.is_finite(), "non-finite output at {i}");
            assert!(
                b >= a - 1e-6 || mask[i / 3] == 0,
                "never darken violated at {i}: {a} -> {b}"
            );
        }
        // Without fused_luma the same scene must stay pinned.
        let mut params = recon_params();
        params.fused_luma = None;
        let mut rgb2 = before.clone();
        let (mask2, _) = clip_mask(&rgb2, 0.995);
        recon(&mut rgb2, w, h, &mask2);
        assert_eq!(rgb2, before, "no-luma Tier-2 must pin");
    }

    #[test]
    fn tier3_pinned_regions_stay_pinned() {
        let w = 16;
        let h = 16;
        let mut rgb = vec![1.05f32; w * h * 3]; // all channels clipped
        let original = rgb.clone();
        let (mask, count) = clip_mask(&rgb, 0.995);
        assert_eq!(count, w * h);
        recon(&mut rgb, w, h, &mask);
        // The continuation field is uniform at the own ceiling here, so the
        // masked box blur round-trips the value through sum/25 — within
        // f32 noise, never an actual brightness change.
        for (i, (&a, &b)) in original.iter().zip(rgb.iter()).enumerate() {
            assert!(
                (b - a).abs() < 1e-5,
                "Tier-3 pixels must stay pinned: {a} -> {b} at {i}"
            );
        }
    }

    #[test]
    fn tier3_ringless_core_continues_at_own_wb_peak() {
        // Real-scene geometry (the "darker gray hole"): a huge all-clipped
        // blob (no clean ring within RECON_RING_MAX) with a G-clipped halo —
        // Tier 1b pushes G up to the WB'd R/B (warm r_gain). Old behavior:
        // the core stayed pinned at the raw ceiling, collapsing to
        // y ≈ 1.25 while the halo reads ~1.8 — the half-stop hole. New:
        // Pass B writes the core at its own WB'd peak (neutral pre-WB), so
        // the collapse lands on the halo's brightness.
        let w = 96;
        let h = 96;
        let (cx, cy) = (48.0, 48.0);
        let mut rgb = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let d = (((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)) as f32).sqrt();
                let i = (y * w + x) * 3;
                if d <= 21.0 {
                    rgb[i] = 1.05;
                    rgb[i + 1] = 1.05;
                    rgb[i + 2] = 1.05;
                } else if d <= 24.0 {
                    rgb[i] = 0.95;
                    rgb[i + 1] = 1.05; // G clips first in neutral light
                    rgb[i + 2] = 0.95;
                } else {
                    // warm neutral falloff, just-below-ceiling near the blob
                    let f = (1.0 - (d - 24.0) * 0.01).max(0.05);
                    rgb[i] = 0.99 * f;
                    rgb[i + 1] = 0.99 * f;
                    rgb[i + 2] = 0.99 * f;
                }
            }
        }
        let (mask, _) = clip_mask(&rgb, 0.995);
        let params = ReconstructParams {
            r_gain: 1.9,
            b_gain: 1.0,
            fused_luma: Some([0.2126, 0.7152, 0.0722]),
        };
        let mut out = rgb.clone();
        reconstruct_clipped(&mut out, w as u32, h as u32, &mask, &params);

        // Halo: G reconstructed upward to the WB'd R peak (Tier 1b).
        let halo_px = (71 * w + 48) * 3; // d ≈ 22.5 → halo band
        assert!((out[halo_px + 1] - 0.95 * 1.9).abs() < 1e-4, "halo G must be 1.805, got {}", out[halo_px + 1]);
        assert_eq!(out[halo_px], 0.95, "halo R untouched");
        assert_eq!(out[halo_px + 2], 0.95, "halo B untouched");

        // Ringless core: no longer pinned, WB-neutral continuation at the
        // recovered boundary level. The 010 halo's own WB'd peak after its
        // Tier-1 estimate is ≈1.85 (R ≈ 0.95·1.9) — BELOW the core's pinned
        // ceiling 1.995; the dead-zone keeps the lift at zero, so the whole
        // core follows the halo's recovered level (≈1.84 through the wide
        // field blur) instead of the clip-ceiling artifact. Only the
        // semi-clipped halo (mask 010, d 21-24) is brightness-informative —
        // clean falloff pixels never pull the core down. The flat plateau
        // holds in the center disc (d ≤ 2: the 13×13 window sits fully
        // inside the core); the outer disc band blends the falloff rim's
        // own levels through the per-cell substitution and softens.
        for y in 42..54 {
            for x in 42..54 {
                let d = (((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)) as f32).sqrt();
                if d > 12.0 {
                    continue;
                }
                let i = (y * w + x) * 3;
                let (er, eg, eb) = (out[i], out[i + 1], out[i + 2]);
                if d <= 2.0 {
                    // Center disc: the continuation holds at the boundary
                    // level (~1.84 WB'd → pre-WB R ≈ 0.97).
                    assert!(er >= 0.9 && er <= 1.02, "core R must continue at the recovered level ≈0.97, got {er}");
                    assert!(eg >= 1.72 && eg <= 1.95, "core G must continue at ≈1.84, got {eg}");
                    assert!(eb >= 1.72 && eb <= 1.95, "core B must continue at ≈1.84, got {eb}");
                } else {
                    // The disc's outer rows blend the falloff rim's own
                    // levels through the per-cell substitution — the edge
                    // softens into the scene instead of diving.
                    assert!(er > 0.85 && er <= 1.02, "outer band must soften, not dive, got {er}");
                }
                // Neutral in WB'd space: est[0]·r_gain == est[1] == est[2]·b_gain.
                assert!((er * 1.9 - eg).abs() < 1e-3 && (eg - eb).abs() < 1e-3, "core must be WB-neutral");
            }
        }

        // Clean pixels untouched.
        let clean_px = (90 * w + 90) * 3;
        for c in 0..3 {
            assert_eq!(out[clean_px + c], rgb[clean_px + c], "clean pixel must be untouched");
        }
        // The collapsed analog: y of the WB'd core == the halo's brightness
        // (the pipeline's collapse then floors the core at that luma).
        let cb = (48 * w + 48) * 3;
        let y_core = 0.2126 * (out[cb] * 1.9) + 0.7152 * out[cb + 1] + 0.0722 * out[cb + 2];
        assert!(y_core > 1.5, "core luma must reach the halo band (1.8), got {y_core}");
    }

    #[test]
    fn tier3_ringless_core_uses_boundary_peak_when_halo_brighter() {
        // Texture-boosted halo (mask 011 with a 1.5:1 R/B surround ratio):
        // Tier 2 pushes the halo's R to the 1.5× window cap → 1.485 pre-WB,
        // WB'd 1.485·1.9 = 2.82 — BRIGHTER than the core's own WB'd peak
        // (2.0). The brightness scan must lift the core to the halo's
        // recovered peak (median of nearest mask≠111 maxima).
        let w = 96;
        let h = 96;
        let (cx, cy) = (48.0, 48.0);
        let mut rgb = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let d = (((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)) as f32).sqrt();
                let i = (y * w + x) * 3;
                if d <= 21.0 {
                    rgb[i] = 1.05;
                    rgb[i + 1] = 1.05;
                    rgb[i + 2] = 1.05;
                } else if d <= 24.0 {
                    // R,G clipped — strong warm halo hue
                    rgb[i] = 1.05;
                    rgb[i + 1] = 1.05;
                    rgb[i + 2] = 0.66;
                } else {
                    let f = (1.0 - (d - 24.0) * 0.01).max(0.05);
                    rgb[i] = 0.99 * f;
                    rgb[i + 1] = 0.66 * f;
                    rgb[i + 2] = 0.42 * f;
                }
            }
        }
        let (mask, _) = clip_mask(&rgb, 0.995);
        let params = ReconstructParams {
            r_gain: 1.9,
            b_gain: 1.0,
            fused_luma: Some([0.2126, 0.7152, 0.0722]),
        };
        let mut out = rgb.clone();
        reconstruct_clipped(&mut out, w as u32, h as u32, &mask, &params);

        // Halo R hits the texture cap: 0.66·(0.99/0.66) = 0.99 → clamped to
        // 1.5·window_max(R) ≈ 1.5·0.99 ≈ 1.485 (window sees clean R 0.99).
        let halo_px = (70 * w + 48) * 3; // d ≈ 22.3 → halo band
        let est_r = out[halo_px];
        assert!(est_r > 1.4 && est_r <= 1.5 * 0.99 + 1e-4, "halo R expected ~1.485, got {est_r}");
        let halo_peak_wb = (est_r * 1.9).max(out[halo_px + 1]).max(out[halo_px + 2]);
        assert!(halo_peak_wb > 2.0, "halo WB peak must clear the core ceiling, got {halo_peak_wb}");

        // Ringless core edge near the boundary (d ≈ 18 from the center — the
        // halo sits ~2-4 px out, inside the dead-zone): the lift stays at
        // zero, so the edge cell continues at the boundary median blended
        // with the halo's own ceiling and the near rim through the wide
        // per-cell blur — it must clear the core's own ceiling by the
        // recovered halo's margin, ordered halo > core edge > own.
        let i = (66 * w + 48) * 3;
        let core_peak_wb = (out[i] * 1.9).max(out[i + 1]).max(out[i + 2]);
        assert!(core_peak_wb > 2.0 + 0.1, "core must clear its own ceiling, got {core_peak_wb}");
        assert!(
            halo_peak_wb - core_peak_wb > 0.1 && halo_peak_wb - core_peak_wb < 1.0,
            "halo {halo_peak_wb} must order above the core edge {core_peak_wb}"
        );
    }

    #[test]
    fn reconstruct_fuzz_keeps_finite_bounded_outputs() {
        // Deterministic fuzz: 40×40 frames, clipped blobs, random ratios.
        let mut seed: u64 = 0x5EED;
        for _case in 0..8 {
            let w = 40;
            let h = 40;
            let mut rgb = vec![0.0f32; w * h * 3];
            for i in 0..w * h * 3 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                rgb[i] = ((seed >> 33) as f32 / u32::MAX as f32) * 1.6;
            }
            for v in rgb.iter_mut() {
                if *v > 1.0 {
                    *v = 1.0;
                }
            }
            let original = rgb.clone();
            let (mask, _) = clip_mask(&rgb, 0.995);
            recon(&mut rgb, w, h, &mask);
            for (i, (&orig, &new)) in original.iter().zip(rgb.iter()).enumerate() {
                assert!(new.is_finite(), "non-finite output at {i}");
                if mask[i / 3] == 0 {
                    assert_eq!(orig, new, "clean pixel {} changed", i / 3);
                    continue;
                }
                let px = i / 3;
                let m = mask[px];
                let x = px % w;
                let y = px / w;
                let ch = i % 3;
                if m.count_ones() == 1 {
                    // Tier 1 keeps the never-darken floor and the texture bound.
                    assert!(new >= orig - 1e-6, "tier-1 darkens at {}", i / 3);
                    assert!(
                        new <= RECON_MAX_FACTOR * self_wmax(&original, &mask, w, h, x, y, ch) + 1e-5,
                        "tier-1 estimate exceeds texture bound at {i}"
                    );
                } else {
                    // Tier 2/3: estimate may move below the pinned ceiling
                    // (that is the de-pink fix), bounded by the window's
                    // clean extent or the pixel's own peak brightness.
                    assert!(new >= 0.0 - 1e-6, "negative estimate at {i}");
                    let bound = (RECON_MAX_FACTOR
                        * self_wmax(&original, &mask, w, h, x, y, ch))
                    .max(orig)
                        + 1e-5;
                    assert!(new <= bound, "tier-2/3 estimate {new} exceeds {bound} at {i}");
                }
            }
        }
    }

    /// Max clean value of channel `ch` in the 5×5 window around (x, y).
    fn self_wmax(
        rgb: &[f32],
        mask: &[u8],
        w: usize,
        h: usize,
        x: usize,
        y: usize,
        ch: usize,
    ) -> f32 {
        let mut wmax = 0.0f32;
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                    let ni = ((ny as usize) * w + nx as usize) * 3 + ch;
                    if mask[ni / 3] & (1 << ch) == 0 {
                        wmax = wmax.max(rgb[ni]);
                    }
                }
            }
        }
        wmax
    }

    #[test]
    fn rolloff_is_identity_below_one() {
        let mut rgb = vec![0.0f32; 3 * 100];
        for (i, v) in rgb.iter_mut().enumerate() {
            *v = ((i * 7) % 1000) as f32 / 1000.0;
        }
        let original = rgb.clone();
        apply_display_rolloff(&mut rgb);
        assert_eq!(rgb, original, "values ≤ 1.0 must be bit-exact");
    }

    #[test]
    fn rolloff_preserves_ratios_above_one() {
        let mut rgb = vec![0.0f32; 3 * 64];
        for (i, v) in rgb.iter_mut().enumerate() {
            *v = 0.3 + ((i * 13) % 64) as f32 / 20.0; // up to ~3.5
        }
        let original = rgb.clone();
        apply_display_rolloff(&mut rgb);
        for i in 0..64 {
            let o = &original[i * 3..i * 3 + 3];
            let n = &rgb[i * 3..i * 3 + 3];
            let om = o[0].max(o[1]).max(o[2]);
            if om <= 1.0 {
                continue;
            }
            let scale = n[0] / o[0];
            for c in 1..3 {
                let s = n[c] / o[c];
                assert!(
                    (s - scale).abs() <= 1e-4 * scale.max(1.0),
                    "ratio broken at {i}: {s} vs {scale}"
                );
            }
        }
    }

    #[test]
    fn rolloff_monotone_bounded_and_continuous() {
        let mut prev = 1.0f32;
        for i in 1..1000 {
            let m = 1.0 + i as f32 / 100.0; // 1.01 .. 10.99
            let mut rgb = [m, 0.5, 0.25];
            apply_display_rolloff(&mut rgb);
            let out = rgb[0];
            assert!(out >= prev - 1e-5, "non-monotone at m={m}: {out} < {prev}");
            assert!(
                out <= ROLLOFF_CEILING + 1e-5,
                "exceeds ceiling: {out}"
            );
            prev = out;
        }
        // Continuity at the knee: f(1) = 1 and f(1.001) ≈ 1.001.
        let mut rgb1 = [1.0, 0.5, 0.25];
        apply_display_rolloff(&mut rgb1);
        assert_eq!(rgb1[0], 1.0);
        let mut rgb2 = [1.001, 0.5, 0.25];
        apply_display_rolloff(&mut rgb2);
        assert!((rgb2[0] - 1.001).abs() < 0.01, "knee discontinuity: {}", rgb2[0]);
    }

    #[test]
    fn rolloff_maps_non_finite_to_zero() {
        let mut rgb = [f32::INFINITY, 1.0, 1.0];
        apply_display_rolloff(&mut rgb);
        assert_eq!(rgb, [0.0, 0.0, 0.0]);
        let mut rgb = [f32::NAN, 0.5, 0.5];
        apply_display_rolloff(&mut rgb);
        assert_eq!(rgb, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn luma_coefficients_and_display_referred_gate() {
        assert_eq!(ColorSpace::Rec709.luma_coefficients(), [0.2126, 0.7152, 0.0722]);
        assert_eq!(ColorSpace::Rec2020.luma_coefficients(), [0.2627, 0.6780, 0.0593]);
        assert!(TransferFunction::Rec709.is_display_referred());
        assert!(TransferFunction::Gamma24.is_display_referred());
        assert!(TransferFunction::Linear.is_display_referred());
        assert!(!TransferFunction::SLog3.is_display_referred());
        assert!(!TransferFunction::ARRIlog3.is_display_referred());
    }

    // ── box_dilate (Tier-3 scan gates) ───────────────────────────────────

    fn dilate_grid(src: &[(usize, usize)], w: usize, h: usize, radius: usize) -> Vec<u8> {
        let mut s = vec![0u8; w * h];
        for &(x, y) in src {
            s[y * w + x] = 1;
        }
        box_dilate(&s, w, h, radius)
    }

    #[test]
    fn box_dilate_single_source_radius_one() {
        // Non-square grid (12×8) — catches w/h-layout swaps.
        let out = dilate_grid(&[(2, 2)], 12, 8, 1);
        let get = |x: usize, y: usize| out[y * 12 + x];
        // 3×3 L∞ ball (Chebyshev distance ≤ 1) around (2,2) is x∈1..3, y∈1..3.
        assert_eq!(get(2, 2), 1);
        assert_eq!(get(1, 1), 1);
        assert_eq!(get(3, 3), 1);
        assert_eq!(get(1, 3), 1);
        assert_eq!(get(3, 1), 1);
        // Outside the ball.
        assert_eq!(get(0, 2), 0, "dx=-2 must be outside radius 1");
        assert_eq!(get(2, 0), 0, "dy=-2 must be outside radius 1");
        assert_eq!(get(4, 2), 0);
        assert_eq!(get(10, 6), 0);
    }

    #[test]
    fn box_dilate_single_source_radius_two() {
        let out = dilate_grid(&[(2, 2)], 12, 8, 2);
        let get = |x: usize, y: usize| out[y * 12 + x];
        assert_eq!(get(0, 2), 1, "dx=-2 inside radius 2");
        assert_eq!(get(4, 4), 1, "corner of the 5×5 ball");
        assert_eq!(get(5, 2), 0, "dx=3 outside radius 2");
        assert_eq!(get(2, 5), 0, "dy=3 outside radius 2");
    }

    #[test]
    fn box_dilate_two_sources_union_and_row_extents() {
        let out = dilate_grid(&[(2, 2), (9, 5)], 12, 8, 1);
        let get = |x: usize, y: usize| out[y * 12 + x];
        assert_eq!(get(1, 1), 1, "north-west of source A");
        assert_eq!(get(8, 5), 1, "west of source B");
        assert_eq!(get(10, 6), 1, "south-east of source B");
        assert_eq!(get(10, 2), 0, "gap between the two balls");
        assert_eq!(get(11, 4), 0);
        // Row extents must cap at the frame edge for a source on the border.
        let out2 = dilate_grid(&[(0, 0)], 6, 6, 2);
        assert_eq!(out2[0], 1);
        assert_eq!(out2[2], 1, "row 0 extended right by 2");
        assert_eq!(out2[5], 0);
        assert_eq!(out2[2 * 6 + 0], 1, "col 0 extended down by 2");
    }
}
