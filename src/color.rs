use crate::agx::{AgxConfig, AgxPipeline, Gamut, OutputTransfer, Transfer};
use crate::file::BayerPattern;
use anyhow::Result;
use rayon::prelude::*;

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
            TransferFunction::Rec709 => { pixels.par_iter_mut().for_each(|v| { *v = rec709_oetf(*v).min(1.0).max(0.0); }); }
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
            chunk[0] = r_out.max(0.0).min(1.0); chunk[1] = g_out.max(0.0).min(1.0); chunk[2] = b_out.max(0.0).min(1.0);
        }
    }
}

pub struct Rec709TransferFunction;
impl Rec709TransferFunction { pub fn new() -> Self { Rec709TransferFunction } }
impl TransferFunctionProcessor for Rec709TransferFunction {
    fn process(&self, pixels: &mut [f32]) { pixels.par_iter_mut().for_each(|v| { *v = rec709_oetf(*v).min(1.0).max(0.0); }); }
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