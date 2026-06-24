//! GPU preview pipeline parameter types.
//!
//! `PreviewParams` is the std140-aligned uniform struct consumed by
//! `shaders/preview.wgsl`. It must match the WGSL `PreviewParams` struct
//! layout exactly — see the `pod_params_layout` test below.

use crate::color::{ColorSpace, TransferFunction};

/// Transfer-function discriminant values sent to the WGSL shader.
/// Must match the switch arms in `apply_oetf` / `inverse_oetf` exactly.
pub fn transfer_to_u32(tf: &TransferFunction) -> u32 {
    match tf {
        TransferFunction::Linear => 0,
        TransferFunction::Rec709 => 1,
        TransferFunction::SLog3 => 2,
        TransferFunction::VLog => 3,
        TransferFunction::ARRIlog3 => 4,
        TransferFunction::ARRIlog4 => 5,
        TransferFunction::CLog3 => 6,
        TransferFunction::FLog2 => 7,
        TransferFunction::AppleLog => 8,
        TransferFunction::AppleLog2 => 9,
        TransferFunction::ACESCCT => 10,
        TransferFunction::PQ => 11,
        TransferFunction::HLG => 12,
        TransferFunction::DaVinciIntermediate => 13,
        TransferFunction::Gamma24 => 14,
    }
}

/// Color-space discriminant values sent to the WGSL shader.
/// Alphabetical order matching `ColorSpace::all()` for consistency.
pub fn color_space_to_u32(cs: &ColorSpace) -> u32 {
    match cs {
        ColorSpace::ACESAP1 => 0,
        ColorSpace::AppleWideGamut => 1,
        ColorSpace::ARRIWideGamut3 => 2,
        ColorSpace::ARRIWideGamut4 => 3,
        ColorSpace::CanonCinemaGamut => 4,
        ColorSpace::DaVinciWideGamut => 5,
        ColorSpace::DciP3 => 6,
        ColorSpace::DisplayP3 => 7,
        ColorSpace::FGamut => 8,
        ColorSpace::FGamutC => 9,
        ColorSpace::PanasonicVGamut => 10,
        ColorSpace::Rec2020 => 11,
        ColorSpace::Rec709 => 12,
        ColorSpace::SGamut3 => 13,
        ColorSpace::SGamut3Cine => 14,
        ColorSpace::Srgb => 15,
    }
}

/// Bayer phase discriminant: 0=RGGB, 1=GRBG, 2=GBRG, 3=BGGR.
/// Must match the `bayer_color` function in the WGSL shader.
pub fn bayer_phase_to_u32(pattern: &crate::file::BayerPattern) -> u32 {
    match pattern {
        crate::file::BayerPattern::RGGB | crate::file::BayerPattern::QuadBayerRGGB => 0,
        crate::file::BayerPattern::GRBG | crate::file::BayerPattern::QuadBayerGRBG => 1,
        crate::file::BayerPattern::GBRG | crate::file::BayerPattern::QuadBayerGBRG => 2,
        crate::file::BayerPattern::BGGR | crate::file::BayerPattern::QuadBayerBGGR => 3,
    }
}

/// std140-aligned uniform struct for `shaders/preview.wgsl`.
///
/// Layout rules:
/// - `vec4<f32>` for CCM rows (WGSL requires `vec4` columns for
///   `mat3x3` in uniform address space; the `.xyz` swizzle extracts
///   the 3 meaningful components).
/// - Total size is a multiple of 16 bytes (std140 requirement).
/// - Field order must match the WGSL struct exactly.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PreviewParams {
    pub width: u32,
    pub height: u32,
    pub bayer_width: u32,
    pub bayer_height: u32,
    pub black_level: f32,
    pub white_level: f32,
    pub exposure: f32,
    pub wb_r: f32,
    pub wb_g: f32,
    pub wb_b: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub shadows: f32,
    pub highlights: f32,
    /// Padding to align `ccm_row0` to 16-byte boundary (WGSL `vec4<f32>`
    /// in `uniform` address space requires this).
    pub _align0: f32,
    pub _align1: f32,
    pub ccm_row0: [f32; 4],
    pub ccm_row1: [f32; 4],
    pub ccm_row2: [f32; 4],
    pub color_space: u32,
    pub transfer: u32,
    pub adjust_enabled: u32,
    pub bayer_phase: u32,
    pub compute_histogram: u32,
    pub _pad0: u32,
    pub _pad1: u32,
    pub _pad2: u32,
    pub _pad3: u32,
    pub _pad4: u32,
    pub _pad5: u32,
    pub _pad6: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pod_params_layout() {
        let p = PreviewParams {
            width: 1920, height: 1080, bayer_width: 4096, bayer_height: 2160,
            black_level: 64.0, white_level: 4095.0, exposure: 1.0,
            wb_r: 1.0, wb_g: 1.0, wb_b: 1.0,
            contrast: 1.0, saturation: 1.0, shadows: 0.0, highlights: 0.0,
            _align0: 0.0, _align1: 0.0,
            ccm_row0: [1.0, 0.0, 0.0, 0.0],
            ccm_row1: [0.0, 1.0, 0.0, 0.0],
            ccm_row2: [0.0, 0.0, 1.0, 0.0],
            color_space: 11, transfer: 1,
            adjust_enabled: 0, bayer_phase: 0,
            compute_histogram: 0,
            _pad0: 0, _pad1: 0, _pad2: 0, _pad3: 0, _pad4: 0, _pad5: 0, _pad6: 0,
        };
        let bytes = bytemuck::bytes_of(&p);
        assert_eq!(bytes.len(), std::mem::size_of::<PreviewParams>());
        assert_eq!(std::mem::size_of::<PreviewParams>() % 16, 0,
            "PreviewParams size ({}) must be 16-byte aligned for std140",
            std::mem::size_of::<PreviewParams>());
    }

    #[test]
    fn transfer_roundtrip() {
        for tf in TransferFunction::all() {
            let n = transfer_to_u32(tf);
            assert!(n < 15, "{} maps to {}, expected < 15", tf.name(), n);
        }
    }

    #[test]
    fn color_space_roundtrip() {
        for cs in ColorSpace::all() {
            let n = color_space_to_u32(cs);
            assert!(n < 16, "{} maps to {}, expected < 16", cs.name(), n);
        }
    }
}
