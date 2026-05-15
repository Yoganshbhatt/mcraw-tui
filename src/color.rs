use crate::agx::{AgxConfig, AgxPipeline, Gamut, OutputTransfer, Transfer};
use crate::file::BayerPattern;
use anyhow::Result;

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
    Rec709,
    Rec2020,
    DciP3,
    Srgb,
    SGamut3Cine,
    SGamut3,
    ARRIWideGamut3,
    ARRIWideGamut4,
    CanonCinemaGamut,
    PanasonicVGamut,
    FGamut,
    FGamutC,
    DaVinciWideGamut,
    ACESAP1,
    AppleDisplayP3,
}

impl ColorSpace {
    pub fn name(&self) -> &'static str {
        match self {
            ColorSpace::Rec709 => "Rec.709",
            ColorSpace::Rec2020 => "Rec.2020",
            ColorSpace::DciP3 => "DCI-P3",
            ColorSpace::Srgb => "sRGB",
            ColorSpace::SGamut3Cine => "S-Gamut3.Cinema",
            ColorSpace::SGamut3 => "S-Gamut3",
            ColorSpace::ARRIWideGamut3 => "ARRI Wide Gamut 3",
            ColorSpace::ARRIWideGamut4 => "ARRI Wide Gamut 4",
            ColorSpace::CanonCinemaGamut => "Canon Cinema Gamut",
            ColorSpace::PanasonicVGamut => "Panasonic V-Gamut",
            ColorSpace::FGamut => "F-Gamut",
            ColorSpace::FGamutC => "F-Gamut C",
            ColorSpace::DaVinciWideGamut => "DaVinci Wide Gamut",
            ColorSpace::ACESAP1 => "ACES AP1",
            ColorSpace::AppleDisplayP3 => "Apple Display P3",
        }
    }

    /// Return the XYZ (D65) → RGB matrix for this color space.
    /// Row-major 3x3: out[i] = sum_j M[i*3+j] * in[j]
    /// Computed from primaries + white point chromaticities.
    /// Return the XYZ (D65) → RGB matrix for this color space.
    /// Row-major 3x3: out[i] = sum_j M[i*3+j] * in[j]
    /// Computed from primaries + white point chromaticities.
    pub fn get_xyz_to_rgb_matrix(&self) -> [f32; 9] {
        match self {
            ColorSpace::Rec709 | ColorSpace::Srgb => xyz_to_rec709(),
            ColorSpace::Rec2020 | ColorSpace::FGamut => {
                // Rec.2020 primaries + D65
                // R(0.708,0.292) G(0.170,0.797) B(0.131,0.046) WP D65(0.3127,0.3290)
                xyz_to_rgb_from_primaries(
                    0.708, 0.292, 0.170, 0.797, 0.131, 0.046,
                    0.3127, 0.3290,
                )
            }
            ColorSpace::DciP3 => {
                // DCI-P3 primaries + DCI white (0.314,0.351)
                // R(0.680,0.320) G(0.265,0.690) B(0.150,0.060)
                xyz_to_rgb_from_primaries(
                    0.680, 0.320, 0.265, 0.690, 0.150, 0.060,
                    0.314, 0.351,
                )
            }
            ColorSpace::AppleDisplayP3 => {
                // Display P3: same primaries as DCI-P3 + D65 white
                xyz_to_rgb_from_primaries(
                    0.680, 0.320, 0.265, 0.690, 0.150, 0.060,
                    0.3127, 0.3290,
                )
            }
            ColorSpace::SGamut3Cine => {
                // S-Gamut3.Cine + D65
                // R(0.76600,0.27500) G(0.22500,0.80000) B(0.08900,-0.08700)
                xyz_to_rgb_from_primaries(
                    0.76600, 0.27500, 0.22500, 0.80000, 0.08900, -0.08700,
                    0.3127, 0.3290,
                )
            }
            ColorSpace::SGamut3 => {
                // S-Gamut3 + D65
                // R(0.7300,0.2800) G(0.1400,0.8550) B(0.1000,-0.0500)
                xyz_to_rgb_from_primaries(
                    0.7300, 0.2800, 0.1400, 0.8550, 0.1000, -0.0500,
                    0.3127, 0.3290,
                )
            }
            ColorSpace::ARRIWideGamut3 => {
                // AWG3 + D65
                // R(0.6840,0.3130) G(0.2210,0.8480) B(0.0861,-0.1020)
                xyz_to_rgb_from_primaries(
                    0.6840, 0.3130, 0.2210, 0.8480, 0.0861, -0.1020,
                    0.3127, 0.3290,
                )
            }
            ColorSpace::ARRIWideGamut4 => {
                // AWG4 + D65
                // R(0.7347,0.2653) G(0.1424,0.8576) B(0.0991,-0.0308)
                xyz_to_rgb_from_primaries(
                    0.7347, 0.2653, 0.1424, 0.8576, 0.0991, -0.0308,
                    0.3127, 0.3290,
                )
            }
            ColorSpace::CanonCinemaGamut => {
                // Canon Cinema Gamut + D65
                // R(0.7400,0.2700) G(0.1700,1.1400) B(0.0800,-0.1000)
                // Note: Official Canon value G y=1.1400 (imaginary primary)
                xyz_to_rgb_from_primaries(
                    0.7400, 0.2700, 0.1700, 1.1400, 0.0800, -0.1000,
                    0.3127, 0.3290,
                )
            }
            ColorSpace::PanasonicVGamut => {
                // V-Gamut + D65
                // R(0.7300,0.2800) G(0.1650,0.8400) B(0.1000,-0.0300)
                xyz_to_rgb_from_primaries(
                    0.7300, 0.2800, 0.1650, 0.8400, 0.1000, -0.0300,
                    0.3127, 0.3290,
                )
            }
            ColorSpace::FGamutC => {
                // F-Gamut C + D65 (≈AP0 but with D65 white, not D60)
                // R(0.7347,0.2653) G(0.0263,0.9737) B(0.1173,-0.0224)
                xyz_to_rgb_from_primaries(
                    0.7347, 0.2653, 0.0263, 0.9737, 0.1173, -0.0224,
                    0.3127, 0.3290,
                )
            }
            ColorSpace::DaVinciWideGamut => {
                // DaVinci Wide Gamut + D65
                // R(0.8000,0.3130) G(0.1682,0.9877) B(0.0790,-0.1155)
                xyz_to_rgb_from_primaries(
                    0.8000, 0.3130, 0.1682, 0.9877, 0.0790, -0.1155,
                    0.3127, 0.3290,
                )
            }
            ColorSpace::ACESAP1 => {
                // ACES AP1 + D60 white point (0.32168,0.33767)
                // R(0.71300,0.29300) G(0.16500,0.83000) B(0.12800,0.04400)
                // Note: Returns XYZ(D65)→AP1 matrix. Input should be D65 XYZ.
                // A D60→D65 adaptation is applied to the white point.
                xyz_to_rgb_from_primaries(
                    0.71300, 0.29300, 0.16500, 0.83000, 0.12800, 0.04400,
                    0.32168, 0.33767,
                )
            }
        }
    }

    /// Iterate all variants in a canonical order (for TUI cycling).
    pub fn all() -> &'static [ColorSpace] {
        &[
            ColorSpace::Rec709,
            ColorSpace::Rec2020,
            ColorSpace::DciP3,
            ColorSpace::Srgb,
            ColorSpace::SGamut3Cine,
            ColorSpace::SGamut3,
            ColorSpace::ARRIWideGamut3,
            ColorSpace::ARRIWideGamut4,
            ColorSpace::CanonCinemaGamut,
            ColorSpace::PanasonicVGamut,
            ColorSpace::FGamut,
            ColorSpace::FGamutC,
            ColorSpace::DaVinciWideGamut,
            ColorSpace::ACESAP1,
            ColorSpace::AppleDisplayP3,
        ]
    }

    /// Return the next variant (wrapping).
    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    /// Return the previous variant (wrapping).
    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferFunction {
    Linear,
    Rec709,
    SLog3,
    VLog,
    ARRIlog3,
    CLog3,
    FLog2,
    ACESCCT,
    PQ,
    HLG,
}

impl TransferFunction {
    pub fn name(&self) -> &'static str {
        match self {
            TransferFunction::Linear => "Linear",
            TransferFunction::Rec709 => "Rec.709",
            TransferFunction::SLog3 => "S-Log3",
            TransferFunction::VLog => "V-Log",
            TransferFunction::ARRIlog3 => "ARRI LogC3",
            TransferFunction::CLog3 => "C-Log3",
            TransferFunction::FLog2 => "F-Log2",
            TransferFunction::ACESCCT => "ACES CCT",
            TransferFunction::PQ => "PQ (ST.2084)",
            TransferFunction::HLG => "HLG (BT.2100)",
        }
    }

    /// Apply the log/linear encoding curve in-place.
    /// Pixels are in linear scene-referred [0.0–1.0+] range.
    pub fn process(&self, pixels: &mut [f32]) {
        match self {
            TransferFunction::Linear => {}

            TransferFunction::Rec709 => {
                for v in pixels.iter_mut() {
                    *v = rec709_oetf(*v).min(1.0).max(0.0);
                }
            }

            // Sony S-Log3 (colour-science / Sony pub.)
            // cut = 0.01125, linear: y = 0.092864 + x*6.6219
            // log: y = 0.594965 + 0.255626*log10(x + 0.01)
            TransferFunction::SLog3 => {
                for v in pixels.iter_mut() {
                    let x = *v;
                    *v = if x >= 0.01125000 {
                        0.594965_f32 + 0.255626_f32 * (x + 0.01).log10()
                    } else {
                        0.092864_f32 + x * 6.62194_f32
                    };
                }
            }

            // Panasonic V-Log (colour-science / Panasonic pub.)
            // cut = 0.01, linear: V = 5.6*x + 0.125
            // log: V = 0.241514*log10(x + 0.00873) + 0.598206
            TransferFunction::VLog => {
                for v in pixels.iter_mut() {
                    let x = *v;
                    *v = if x < 0.01 {
                        5.6_f32 * x + 0.125_f32
                    } else {
                        0.241514_f32 * (x + 0.00873_f32).log10() + 0.598206_f32
                    };
                }
            }

            // ARRI LogC3 EI 800, Linear Scene Exposure Factor (colour-science)
            // cut = 0.010591, linear: t = 5.367655*x + 0.092809
            // log: t = 0.247190*log10(5.555556*x + 0.052272) + 0.385537
            TransferFunction::ARRIlog3 => {
                for v in pixels.iter_mut() {
                    let x = *v;
                    *v = if x > 0.010591_f32 {
                        0.247190_f32 * (5.555556_f32 * x + 0.052272_f32).log10() + 0.385537_f32
                    } else {
                        5.367655_f32 * x + 0.092809_f32
                    };
                }
            }

            // Canon C-Log3 v1.2, 3-segment (colour-science / Canon 2020)
            // Graft points at encoded values 0.097465473 and 0.15277891
            // Shoulder: 0.36726845*log10(-x*14.98325+1) + 0.12783901   (x < neg_graft_lin)
            // Linear:   1.9754798*x + 0.12512219                     (x between grafts)
            // Toe:      -0.36726845*log10(x*14.98325+1) + 0.12240537 (x > pos_graft_lin)
            TransferFunction::CLog3 => {
                for v in pixels.iter_mut() {
                    let x = *v;
                    // Decode the graft points from linear:
                    let neg_graft_lin = (0.097465473_f32 - 0.12512219_f32) / 1.9754798_f32; // -0.014
                    let pos_graft_lin = (0.15277891_f32 - 0.12512219_f32) / 1.9754798_f32; // +0.014
                    *v = if x < neg_graft_lin {
                        -0.36726845_f32 * ((-x * 14.98325_f32 + 1.0_f32).max(1e-10_f32)).log10()
                            + 0.12783901_f32
                    } else if x <= pos_graft_lin {
                        1.9754798_f32 * x + 0.12512219_f32
                    } else {
                        0.36726845_f32 * (x * 14.98325_f32 + 1.0_f32).log10() + 0.12240537_f32
                    };
                }
            }

            // Fujifilm F-Log2 (colour-science / Fujifilm 2022a)
            // cut = 0.000889, linear: out = 8.799461*x + 0.092864
            // log: out = 0.245281*log10(5.555556*x + 0.064829) + 0.384316
            TransferFunction::FLog2 => {
                for v in pixels.iter_mut() {
                    let x = *v;
                    *v = if x >= 0.000889_f32 {
                        0.245281_f32 * (5.555556_f32 * x + 0.064829_f32).log10() + 0.384316_f32
                    } else {
                        8.799461_f32 * x + 0.092864_f32
                    };
                }
            }

            // ACEScct (colour-science / Academy)
            // cut = 0.0078125, linear: V = 10.54023774*x + 0.07290553
            // log: V = (log2(x) + 9.72) / 17.52
            TransferFunction::ACESCCT => {
                for v in pixels.iter_mut() {
                    let x = *v;
                    *v = if x > 0.0078125_f32 {
                        (x.log2() + 9.72_f32) / 17.52_f32
                    } else {
                        10.5402377416545_f32 * x + 0.0729055341958355_f32
                    };
                }
            }

            // SMPTE ST.2084 (PQ) (colour-science)
            // V = ((c1 + c2 * x^m1) / (1 + c3 * x^m1))^m2
            // m1=0.1593017578125, m2=78.84375, c1=0.8359375, c2=18.8515625, c3=18.6875
            TransferFunction::PQ => {
                for v in pixels.iter_mut() {
                    let x = *v;
                    let x_m1 = x.powf(0.1593017578125_f32);
                    *v = ((0.8359375_f32 + 18.8515625_f32 * x_m1)
                        / (1.0_f32 + 18.6875_f32 * x_m1))
                    .powf(78.84375_f32);
                }
            }

            // HLG (BT.2100) with E=1 breakpoint (colour-science)
            // if x <= 1.0: V = sqrt(x)
            // else: V = a*ln(x - b) + c,  a=0.17883277, b=0.28466892, c=0.55991073
            TransferFunction::HLG => {
                for v in pixels.iter_mut() {
                    let x = *v;
                    *v = if x <= 1.0_f32 {
                        x.sqrt()
                    } else {
                        0.17883277_f32 * (x - 0.28466892_f32).ln() + 0.55991073_f32
                    };
                }
            }
        }
    }

    /// Iterate all variants (for TUI cycling).
    pub fn all() -> &'static [TransferFunction] {
        &[
            TransferFunction::Linear,
            TransferFunction::Rec709,
            TransferFunction::SLog3,
            TransferFunction::VLog,
            TransferFunction::ARRIlog3,
            TransferFunction::CLog3,
            TransferFunction::FLog2,
            TransferFunction::ACESCCT,
            TransferFunction::PQ,
            TransferFunction::HLG,
        ]
    }

    /// Return the next variant (wrapping).
    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    /// Return the previous variant (wrapping).
    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }

    /// Returns true if this is a log encoding (anything other than Linear/Rec709).
    pub fn is_log_bypass(&self) -> bool {
        !matches!(self, TransferFunction::Linear | TransferFunction::Rec709)
    }

    /// Returns true if this transfer function requires 10-bit encoding to avoid banding.
    /// Returns true for all log curves and HDR (PQ/HLG); false only for Linear and Rec709.
    pub fn requires_10bit(&self) -> bool {
        !matches!(self, TransferFunction::Linear | TransferFunction::Rec709)
    }
}

#[inline]
pub fn rec709_oetf(x: f32) -> f32 {
    if x < 0.018 {
        4.5 * x
    } else {
        1.099 * x.powf(0.45) - 0.099
    }
}

#[inline]
pub fn rec709_eotf(x: f32) -> f32 {
    if x < 0.0812429 {
        x / 4.5
    } else {
        ((x + 0.099) / 1.099).powf(1.0 / 0.45)
    }
}

#[inline]
pub fn apply_ccm(r: f32, g: f32, b: f32, ccm: &[f32; 9]) -> [f32; 3] {
    [
        r * ccm[0] + g * ccm[1] + b * ccm[2],
        r * ccm[3] + g * ccm[4] + b * ccm[5],
        r * ccm[6] + g * ccm[7] + b * ccm[8],
    ]
}

pub fn identity_ccm() -> [f32; 9] {
    [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
}

pub fn invert_3x3(m: &[f32; 9]) -> [f32; 9] {
    let det = m[0] * (m[4] * m[8] - m[5] * m[7])
            - m[1] * (m[3] * m[8] - m[5] * m[6])
            + m[2] * (m[3] * m[7] - m[4] * m[6]);
    let inv_det = 1.0 / det;
    [
        (m[4] * m[8] - m[5] * m[7]) * inv_det,
        (m[2] * m[7] - m[1] * m[8]) * inv_det,
        (m[1] * m[5] - m[2] * m[4]) * inv_det,
        (m[5] * m[6] - m[3] * m[8]) * inv_det,
        (m[0] * m[8] - m[2] * m[6]) * inv_det,
        (m[2] * m[3] - m[0] * m[5]) * inv_det,
        (m[3] * m[7] - m[4] * m[6]) * inv_det,
        (m[1] * m[6] - m[0] * m[7]) * inv_det,
        (m[0] * m[4] - m[1] * m[3]) * inv_det,
    ]
}

pub fn mat_mul_3x3(a: &[f32; 9], b: &[f32; 9]) -> [f32; 9] {
    let mut out = [0.0; 9];
    for i in 0..3 {
        for j in 0..3 {
            out[i * 3 + j] = a[i * 3] * b[j] + a[i * 3 + 1] * b[3 + j] + a[i * 3 + 2] * b[6 + j];
        }
    }
    out
}

pub fn camera_to_rec709_matrix(color_matrix: &[f32; 9]) -> [f32; 9] {
    let cam_to_xyz_d50 = invert_3x3(color_matrix);

    let d50_to_d65 = [
        0.9555, -0.0230,  0.0633,
       -0.0283,  1.0099,  0.0210,
        0.0123, -0.0205,  1.3300,
    ];

    let cam_to_xyz_d65 = mat_mul_3x3(&d50_to_d65, &cam_to_xyz_d50);

    mat_mul_3x3(&xyz_to_rec709(), &cam_to_xyz_d65)
}

pub fn rec709_to_xyz() -> [f32; 9] {
    [
        0.4124564, 0.3575761, 0.1804375,
        0.2126729, 0.7151522, 0.0721750,
        0.0193339, 0.1191920, 0.9503041,
    ]
}

/* ------------------------------------------------------------------ */
/* CAT16 Chromatic Adaptation Transform (CIE TC 1-90, 2016)           */
/* ------------------------------------------------------------------ */

/// CAT16 cone-response matrix (MCAT16)
const MCAT16: [f32; 9] = [
     0.401288,  0.650173, -0.051461,
    -0.250268,  1.204414,  0.045854,
    -0.002079,  0.048952,  0.953127,
];

/// Inverse of MCAT16
const MCAT16_INV: [f32; 9] = [
     1.86206786, -1.01125463,  0.14918678,
     0.38752654,  0.62144744, -0.00897398,
    -0.01584150, -0.03412294,  1.04996444,
];

/// CIE D50 white point (XYZ Y=1)
pub const D50_XYZ: [f32; 3] = [0.3457 / 0.3585, 1.0, (1.0 - 0.3457 - 0.3585) / 0.3585];
/// CIE D65 white point (XYZ Y=1)
pub const D65_XYZ: [f32; 3] = [0.95047, 1.0, 1.08883];

/// Apply CAT16 chromatic adaptation: adapt XYZ from src_white to dst_white.
pub fn cat16_adapt(xyz: &[f32; 3], src_white: &[f32; 3], dst_white: &[f32; 3]) -> [f32; 3] {
    let [l_s, m_s, s_s] = mat_mul_vec3(&MCAT16, src_white);
    let [l_d, m_d, s_d] = mat_mul_vec3(&MCAT16, dst_white);

    let lms = mat_mul_vec3(&MCAT16, xyz);
    let adapted = [
        lms[0] * (l_d / l_s),
        lms[1] * (m_d / m_s),
        lms[2] * (s_d / s_s),
    ];
    mat_mul_vec3(&MCAT16_INV, &adapted)
}

#[inline]
pub fn mat_mul_vec3(m: &[f32; 9], v: &[f32; 3]) -> [f32; 3] {
    [
        m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
        m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
        m[6] * v[0] + m[7] * v[1] + m[8] * v[2],
    ]
}

/// Build a camera→XYZ matrix from a DNG ColorMatrix (column-major XYZ→Camera).
/// Transposes (column→row), inverts, and optionally applies calibration matrix.
pub fn camera_to_xyz_matrix(color_matrix: &[f32; 9], calibration_matrix: Option<&[f32; 9]>) -> [f32; 9] {
    let row_major = [
        color_matrix[0], color_matrix[3], color_matrix[6],
        color_matrix[1], color_matrix[4], color_matrix[7],
        color_matrix[2], color_matrix[5], color_matrix[8],
    ];
    match calibration_matrix {
        Some(cal) => {
            let eff_cm = mat_mul_3x3(&row_major, cal);
            invert_3x3(&eff_cm)
        }
        None => invert_3x3(&row_major),
    }
}

/// Interpolate between two 3x3 matrices by factor `t` (0 = first, 1 = second).
pub fn interpolate_matrix(a: &[f32; 9], b: &[f32; 9], t: f32) -> [f32; 9] {
    let s = 1.0 - t;
    let mut out = [0.0; 9];
    for i in 0..9 {
        out[i] = a[i] * s + b[i] * t;
    }
    out
}

pub fn xyz_to_rec709() -> [f32; 9] {
    [
        3.2404542, -1.5371385, -0.4985354,
        -0.9689294, 1.8767608, 0.0415560,
        0.0556434, -0.2040259, 1.0572252,
    ]
}

/// Compute an XYZ→RGB matrix from primary chromaticities and white point.
/// All coordinates are CIE 1931 (x,y). White point is (xw, yw).
pub fn xyz_to_rgb_from_primaries(
    xr: f32, yr: f32, xg: f32, yg: f32, xb: f32, yb: f32,
    xw: f32, yw: f32,
) -> [f32; 9] {
    let xr_z = (1.0 - xr - yr) / yr;
    let xg_z = (1.0 - xg - yg) / yg;
    let xb_z = (1.0 - xb - yb) / yb;

    let m = [
        xr / yr,   xg / yg,   xb / yb,
        1.0,       1.0,       1.0,
        xr_z,      xg_z,      xb_z,
    ];

    let wx = xw / yw;
    let wy = 1.0;
    let wz = (1.0 - xw - yw) / yw;

    let det_m = m[0] * (m[4] * m[8] - m[5] * m[7])
              - m[1] * (m[3] * m[8] - m[5] * m[6])
              + m[2] * (m[3] * m[7] - m[4] * m[6]);
    let inv_det = 1.0 / det_m;
    let inv_m = [
        (m[4] * m[8] - m[5] * m[7]) * inv_det,
        (m[2] * m[7] - m[1] * m[8]) * inv_det,
        (m[1] * m[5] - m[2] * m[4]) * inv_det,
        (m[5] * m[6] - m[3] * m[8]) * inv_det,
        (m[0] * m[8] - m[2] * m[6]) * inv_det,
        (m[2] * m[3] - m[0] * m[5]) * inv_det,
        (m[3] * m[7] - m[4] * m[6]) * inv_det,
        (m[1] * m[6] - m[0] * m[7]) * inv_det,
        (m[0] * m[4] - m[1] * m[3]) * inv_det,
    ];

    let sr = inv_m[0] * wx + inv_m[1] * wy + inv_m[2] * wz;
    let sg = inv_m[3] * wx + inv_m[4] * wy + inv_m[5] * wz;
    let sb = inv_m[6] * wx + inv_m[7] * wy + inv_m[8] * wz;

    let rgb_to_xyz = [
        m[0] * sr, m[1] * sg, m[2] * sb,
        m[3] * sr, m[4] * sg, m[5] * sb,
        m[6] * sr, m[7] * sg, m[8] * sb,
    ];

    invert_3x3(&rgb_to_xyz)
}

pub struct BilinearDemosaic {
    pattern: BayerPattern,
}

impl BilinearDemosaic {
    pub fn new(pattern: BayerPattern) -> Self {
        BilinearDemosaic { pattern }
    }

    fn get_pixel(&self, bayer: &[u16], stride_width: u32, x: i32, y: i32) -> f64 {
        if x < 0 || y < 0 || x >= stride_width as i32 {
            return 0.0;
        }
        let idx = (y as usize) * (stride_width as usize) + (x as usize);
        if idx >= bayer.len() {
            return 0.0;
        }
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

    fn is_green_site(&self, x: i32, y: i32, pattern: BayerPattern) -> bool {
        !self.is_red_site(x, y, pattern) && !self.is_blue_site(x, y, pattern)
    }

    fn interp_green_at_red(&self, bayer: &[u16], stride: u32, _height: u32, x: i32, y: i32, pattern: BayerPattern) -> f64 {
        let mut sum = 0.0;
        let mut count = 0.0;

        let positions = [(0, -1), (0, 1), (-1, 0), (1, 0)];
        for (dx, dy) in positions.iter() {
            let px = x + dx;
            let py = y + dy;
            if self.is_green_site(px, py, pattern) {
                sum += self.get_pixel(bayer, stride, px, py);
                count += 1.0;
            }
        }

        if count > 0.0 { sum / count } else { self.get_pixel(bayer, stride, x, y) }
    }

    fn interp_green_at_blue(&self, bayer: &[u16], stride: u32, height: u32, x: i32, y: i32, pattern: BayerPattern) -> f64 {
        self.interp_green_at_red(bayer, stride, height, x, y, pattern)
    }

    fn interp_blue_at_red(&self, bayer: &[u16], stride: u32, _height: u32, x: i32, y: i32, pattern: BayerPattern) -> f64 {
        let mut sum = 0.0;
        let mut count = 0.0;

        let positions = [(-1, -1), (1, -1), (-1, 1), (1, 1)];
        for (dx, dy) in positions.iter() {
            let px = x + dx;
            let py = y + dy;
            if self.is_blue_site(px, py, pattern) {
                sum += self.get_pixel(bayer, stride, px, py);
                count += 1.0;
            }
        }

        if count > 0.0 { sum / count } else { self.get_pixel(bayer, stride, x, y) }
    }

    fn interp_red_at_blue(&self, bayer: &[u16], stride: u32, _height: u32, x: i32, y: i32, pattern: BayerPattern) -> f64 {
        let mut sum = 0.0;
        let mut count = 0.0;

        let positions = [(-1, -1), (1, -1), (-1, 1), (1, 1)];
        for (dx, dy) in positions.iter() {
            let px = x + dx;
            let py = y + dy;
            if self.is_red_site(px, py, pattern) {
                sum += self.get_pixel(bayer, stride, px, py);
                count += 1.0;
            }
        }

        if count > 0.0 { sum / count } else { self.get_pixel(bayer, stride, x, y) }
    }
}

impl Demosaic for BilinearDemosaic {
    fn process(&self, bayer: &[u16], stride_width: u32, offset_x: u32, offset_y: u32, active_width: u32, active_height: u32, pattern: &BayerPattern) -> Result<Vec<f32>> {
        let stride = stride_width as usize;
        let ox = offset_x as i32;
        let oy = offset_y as i32;
        let aw = active_width as usize;
        let ah = active_height as usize;

        let min_len = (stride * (oy as usize + ah - 1) + ox as usize + aw - 1) + 1;
        if bayer.len() < min_len {
            anyhow::bail!("Bayer data too short: len={}, need at least {} for {}x{} active at offset {},{}",
                bayer.len(), min_len, active_width, active_height, offset_x, offset_y);
        }

        let mut rgb = Vec::with_capacity(aw * ah * 3);
        let pat = *pattern;

        for sy in 0..ah as i32 {
            for sx in 0..aw as i32 {
                let x = sx + ox;
                let y = sy + oy;

                let is_red = self.is_red_site(x, y, pat);
                let is_blue = self.is_blue_site(x, y, pat);

                let (r, g, b) = if is_red {
                    (self.get_pixel(bayer, stride_width, x, y),
                     self.interp_green_at_red(bayer, stride_width, active_height, x, y, pat),
                     self.interp_blue_at_red(bayer, stride_width, active_height, x, y, pat))
                } else if is_blue {
                    (self.interp_red_at_blue(bayer, stride_width, active_height, x, y, pat),
                     self.interp_green_at_blue(bayer, stride_width, active_height, x, y, pat),
                     self.get_pixel(bayer, stride_width, x, y))
                } else {
                    let is_top_green = match pat {
                        BayerPattern::RGGB | BayerPattern::BGGR => y % 2 == 0,
                        BayerPattern::GRBG => y % 2 == 0,
                        BayerPattern::GBRG => y % 2 == 1,
                        _ => y % 2 == 0,
                    };
                    if is_top_green {
                        (self.interp_red_at_blue(bayer, stride_width, active_height, x + 1, y, pat),
                         self.get_pixel(bayer, stride_width, x, y),
                         self.interp_blue_at_red(bayer, stride_width, active_height, x - 1, y, pat))
                    } else {
                        (self.interp_red_at_blue(bayer, stride_width, active_height, x - 1, y, pat),
                         self.get_pixel(bayer, stride_width, x, y),
                         self.interp_blue_at_red(bayer, stride_width, active_height, x + 1, y, pat))
                    }
                };

                rgb.push(r as f32);
                rgb.push(g as f32);
                rgb.push(b as f32);
            }
        }

        Ok(rgb)
    }
}

pub struct CcmColorSpaceConverter;

impl CcmColorSpaceConverter {
    pub fn new() -> Self {
        CcmColorSpaceConverter
    }
}

impl Default for CcmColorSpaceConverter {
    fn default() -> Self {
        Self::new()
    }
}

impl ColorSpaceConverter for CcmColorSpaceConverter {
    fn process(&self, pixels: &mut [f32], ccm: &[f32; 9]) {
        for chunk in pixels.chunks_exact_mut(3) {
            let [r_out, g_out, b_out] = apply_ccm(chunk[0], chunk[1], chunk[2], ccm);
            chunk[0] = r_out.max(0.0).min(1.0);
            chunk[1] = g_out.max(0.0).min(1.0);
            chunk[2] = b_out.max(0.0).min(1.0);
        }
    }
}

pub struct Rec709TransferFunction;

impl Rec709TransferFunction {
    pub fn new() -> Self {
        Rec709TransferFunction
    }
}

impl TransferFunctionProcessor for Rec709TransferFunction {
    fn process(&self, pixels: &mut [f32]) {
        for v in pixels.iter_mut() {
            *v = rec709_oetf(*v).min(1.0).max(0.0);
        }
    }
}

pub struct LinearTransferFunction;

impl LinearTransferFunction {
    pub fn new() -> Self {
        LinearTransferFunction
    }
}

impl TransferFunctionProcessor for LinearTransferFunction {
    fn process(&self, _pixels: &mut [f32]) {}
}

pub struct AgxKrakenPipeline {
    demosaic: BilinearDemosaic,
    agx: AgxPipeline,
    output_gamma: f32,
    enable_tonemap: bool,
}

impl AgxKrakenPipeline {
    pub fn new(pattern: BayerPattern) -> Self {
        let config = ColorPipelineConfig::broadcast();
        let demosaic = BilinearDemosaic::new(pattern);
        let agx = AgxPipeline::new(config.tonemap_config.clone());
        let output_gamma = config.output_gamma.gamma();
        let enable_tonemap = config.enable_tonemapping;
        AgxKrakenPipeline { demosaic, agx, output_gamma, enable_tonemap }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputGamma {
    Srgb,
    Bt1886,
    Linear,
}

impl OutputGamma {
    pub fn gamma(&self) -> f32 {
        match self {
            OutputGamma::Srgb => 2.2,
            OutputGamma::Bt1886 => 2.4,
            OutputGamma::Linear => 1.0,
        }
    }
}

pub struct ColorPipelineConfig {
    pub input_color_space: ColorSpace,
    pub input_transfer: TransferFunction,
    pub output_color_space: ColorSpace,
    pub output_transfer: TransferFunction,
    pub output_gamma: OutputGamma,
    pub enable_tonemapping: bool,
    pub tonemap_config: AgxConfig,
}

impl Default for ColorPipelineConfig {
    fn default() -> Self {
        Self {
            input_color_space: ColorSpace::Rec709,
            input_transfer: TransferFunction::Linear,
            output_color_space: ColorSpace::Rec709,
            output_transfer: TransferFunction::Rec709,
            output_gamma: OutputGamma::Bt1886,
            enable_tonemapping: true,
            tonemap_config: AgxConfig::default(),
        }
    }
}

impl ColorPipelineConfig {
    pub fn broadcast() -> Self {
        let mut config = AgxConfig::default();
        config.in_gamut = Gamut::Rec709;
        config.in_transfer = Transfer::Linear;
        config.working_curve = Transfer::AgxLogKraken;
        config.out_gamut = Gamut::Rec709;
        config.out_transfer = OutputTransfer::Bt1886InverseEotf;
        config.toe_power = 3.0;
        config.shoulder_power = 3.25;
        config.slope = 2.0;
        config.working_mid_grey = 0.18;
        config.log_output = false;

        Self {
            input_color_space: ColorSpace::Rec709,
            input_transfer: TransferFunction::Linear,
            output_color_space: ColorSpace::Rec709,
            output_transfer: TransferFunction::Rec709,
            output_gamma: OutputGamma::Bt1886,
            enable_tonemapping: true,
            tonemap_config: config,
        }
    }

    pub fn log_output(log_space: TransferFunction, gamut: ColorSpace) -> Self {
        let mut config = AgxConfig::default();
        config.in_gamut = Gamut::Rec709;
        config.in_transfer = Transfer::Linear;
        config.working_curve = Transfer::AgxLogKraken;
        config.out_gamut = match gamut {
            ColorSpace::Rec709 => Gamut::Rec709,
            ColorSpace::Rec2020 => Gamut::Rec2020,
            ColorSpace::DciP3 | ColorSpace::AppleDisplayP3 => Gamut::P3D65,
            ColorSpace::SGamut3Cine => Gamut::SGamut3Cine,
            ColorSpace::SGamut3 => Gamut::SGamut3,
            ColorSpace::ARRIWideGamut3 | ColorSpace::ARRIWideGamut4 => Gamut::Awg3,
            ColorSpace::CanonCinemaGamut => Gamut::CanonCinema,
            ColorSpace::ACESAP1 => Gamut::Ap1,
            ColorSpace::FGamut | ColorSpace::PanasonicVGamut => Gamut::Rwg,
            ColorSpace::FGamutC => Gamut::Ap0,
            ColorSpace::DaVinciWideGamut => Gamut::DaVinciWg,
            _ => Gamut::Rec709,
        };
        config.out_transfer = OutputTransfer::Linear;
        config.log_output = true;

        Self {
            input_color_space: ColorSpace::Rec709,
            input_transfer: TransferFunction::Linear,
            output_color_space: gamut,
            output_transfer: log_space,
            output_gamma: OutputGamma::Linear,
            enable_tonemapping: false,
            tonemap_config: config,
        }
    }
}

pub fn pipeline_convert_to_u16(pixels: &[f32]) -> Vec<u16> {
    pixels.iter().map(|&v| (v.clamp(0.0, 1.0) * 65535.0) as u16).collect()
}

pub fn normalize_linear(pixels: &mut [f32], black_level: f64, white_level: f64) {
    let range = if white_level > black_level { white_level - black_level } else { 1.0 };
    let inv_range = 1.0 / range;
    for v in pixels.iter_mut() {
        *v = ((*v as f64 - black_level) * inv_range).clamp(0.0, 1.0) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_ccm() {
        let ccm = identity_ccm();
        assert!((ccm[0] - 1.0).abs() < 0.001);
        assert!((ccm[4] - 1.0).abs() < 0.001);
        assert!((ccm[8] - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_apply_ccm() {
        let ccm = identity_ccm();
        let result = apply_ccm(0.5, 0.5, 0.5, &ccm);
        assert!((result[0] - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_rec709_oetf() {
        let dark = rec709_oetf(0.01);
        assert!((dark - 0.045).abs() < 0.001);
        let bright = rec709_oetf(0.5);
        assert!(bright > 0.5);
        assert!(bright <= 1.0);
    }

    #[test]
    fn test_normalize_linear() {
        let mut pixels = vec![100.0, 200.0, 300.0, 400.0, 500.0];
        normalize_linear(&mut pixels, 100.0, 500.0);
        assert!((pixels[0] - 0.0).abs() < 0.001);
        assert!((pixels[4] - 1.0).abs() < 0.001);
        assert!((pixels[2] - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_ccm_converter_no_op() {
        let mut pixels = vec![0.2, 0.4, 0.6, 0.1, 0.2, 0.3];
        let ccm = identity_ccm();
        let converter = CcmColorSpaceConverter;
        converter.process(&mut pixels, &ccm);
        assert!((pixels[0] - 0.2).abs() < 0.001);
        assert!((pixels[4] - 0.2).abs() < 0.001);
    }

    #[test]
    fn test_rec709_transfer() {
        let mut pixels = vec![0.18, 0.5, 1.0];
        let tf = Rec709TransferFunction;
        tf.process(&mut pixels);
        assert!(pixels[0] > 0.3);
        assert!(pixels[1] > 0.6);
        assert!((pixels[2] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_bilinear_demosaic() {
        let pattern = BayerPattern::RGGB;
        let bayer: Vec<u16> = (0..16).map(|i| i * 100).collect();
        let demosaic = BilinearDemosaic::new(pattern);
        let result = demosaic.process(&bayer, 4, 0, 0, 4, 4, &pattern).unwrap();
        assert_eq!(result.len(), 16 * 3);
        assert!(result.iter().all(|&v| v >= 0.0));
    }

    #[test]
    fn test_pipeline_convert_to_u16() {
        let f32_vals = vec![0.0, 0.5, 1.0];
        let u16_vals = pipeline_convert_to_u16(&f32_vals);
        assert_eq!(u16_vals[0], 0);
        assert_eq!(u16_vals[2], 65535);
        assert!((u16_vals[1] as f64 / 65535.0 - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_invert_3x3_identity() {
        let m = identity_ccm();
        let inv = invert_3x3(&m);
        assert!((inv[0] - 1.0).abs() < 0.001);
        assert!((inv[4] - 1.0).abs() < 0.001);
        assert!((inv[8] - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_invert_3x3_inverts_correctly() {
        let xyz_to_rec = xyz_to_rec709();
        let rec_to_xyz = rec709_to_xyz();
        let identity = mat_mul_3x3(&xyz_to_rec, &rec_to_xyz);
        assert!((identity[0] - 1.0).abs() < 0.01);
        assert!((identity[4] - 1.0).abs() < 0.01);
        assert!((identity[8] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_camera_to_rec709_preserves_neutral() {
        let color_matrix = [1.3106, -0.7174, 0.0273, -0.3461, 1.3205, 0.1678, 0.0021, 0.1574, 0.7276];
        let cam_to_rec = camera_to_rec709_matrix(&color_matrix);
        let neutral = apply_ccm(0.5, 0.5, 0.5, &cam_to_rec);
        assert!(neutral[0] > 0.0);
        assert!(neutral[1] > 0.0);
        assert!(neutral[2] > 0.0);
    }
}
