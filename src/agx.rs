use rayon::prelude::*;
use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gamut {
    Ap0, Ap1, P3D65, P3D60, Rec2020, Rec709,
    Awg3, Awg4, Rwg, SGamut3, SGamut3Cine,
    BlackmagicWg, CanonCinema, DaVinciWg, EGamut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    Linear,
    AcesCct,
    ArriLogC3,
    ArriLogC4,
    RedLog3G10,
    SonySLog3,
    SonySLog2,
    BmFilmGen5,
    CanonLog3,
    CanonLog2,
    DaVinciIntermediate,
    FilmlightTLog,
    AgxLogKraken,
    VLog,
    FLog2,
    FLog2C,
    AppleLog2,
    HLG,
    PQ,
    DNG,
    DI,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputTransfer {
    Linear,
    SrgbInverseEotf,
    Bt1886InverseEotf,
}

#[derive(Debug, Clone)]
pub struct AgxConfig {
    pub inset_red: f32,
    pub inset_green: f32,
    pub inset_blue: f32,
    pub rotate_red: f32,
    pub rotate_green: f32,
    pub rotate_blue: f32,
    pub outset_red: f32,
    pub outset_green: f32,
    pub outset_blue: f32,
    pub toe_power: f32,
    pub shoulder_power: f32,
    pub slope: f32,
    pub in_gamut: Gamut,
    pub in_transfer: Transfer,
    pub working_curve: Transfer,
    pub working_mid_grey: f32,
    pub out_gamut: Gamut,
    pub out_transfer: OutputTransfer,
    pub log_output: bool,
}

impl Default for AgxConfig {
    fn default() -> Self {
        Self {
            inset_red: 0.2,
            inset_green: 0.2,
            inset_blue: 0.2,
            rotate_red: 0.0,
            rotate_green: 0.0,
            rotate_blue: 0.0,
            outset_red: 0.0,
            outset_green: 0.0,
            outset_blue: 0.0,
            toe_power: 3.0,
            shoulder_power: 3.25,
            slope: 2.0,
            in_gamut: Gamut::Rec709,
            in_transfer: Transfer::Linear,
            working_curve: Transfer::AgxLogKraken,
            working_mid_grey: 0.606060,
            out_gamut: Gamut::Rec709,
            out_transfer: OutputTransfer::SrgbInverseEotf,
            log_output: false,
        }
    }
}

pub struct AgxPipeline {
    config: AgxConfig,
    inset_mat: [f32; 9],
    outset_mat: [f32; 9],
    gamut_mat: [f32; 9],
    out_gamma: f32,
    log_floor: [f32; 3],
    mid_grey_lin: f32,
}

impl AgxPipeline {
    pub fn new(config: AgxConfig) -> Self {
        let in_chrom = gamut_chromaticities(config.in_gamut);
        let out_chrom = gamut_chromaticities(config.out_gamut);

        let inset_chrom = inset_primaries(
            in_chrom,
            config.inset_red,
            config.inset_green,
            config.inset_blue,
            config.rotate_red,
            config.rotate_green,
            config.rotate_blue,
        );
        let outset_chrom = inset_primaries(
            in_chrom,
            config.outset_red,
            config.outset_green,
            config.outset_blue,
            config.rotate_red,
            config.rotate_green,
            config.rotate_blue,
        );

        let inset_mat = rgb_to_rgb(inset_chrom, in_chrom);
        let outset_mat = rgb_to_rgb(in_chrom, outset_chrom);
        let gamut_mat = rgb_to_rgb(in_chrom, out_chrom);

        let out_gamma = match config.out_transfer {
            OutputTransfer::Linear => 1.0,
            OutputTransfer::SrgbInverseEotf => 2.2,
            OutputTransfer::Bt1886InverseEotf => 2.4,
        };

        let log_floor = log_to_lin([0.0, 0.0, 0.0], config.in_transfer);

        let mid_grey_lin = log_to_lin(
            [config.working_mid_grey, config.working_mid_grey, config.working_mid_grey],
            config.working_curve,
        )[0];

        Self {
            config,
            inset_mat,
            outset_mat,
            gamut_mat,
            out_gamma,
            log_floor,
            mid_grey_lin,
        }
    }

    #[inline]
    pub fn process_pixel(&self, r: f32, g: f32, b: f32) -> [f32; 3] {
        let mut rgb = [r, g, b];

        rgb = log_to_lin(rgb, self.config.in_transfer);

        rgb[0] = rgb[0].max(self.log_floor[0]);
        rgb[1] = rgb[1].max(self.log_floor[1]);
        rgb[2] = rgb[2].max(self.log_floor[2]);

        rgb = mat_mul_vec3(&self.inset_mat, rgb);

        rgb = lin_to_log(rgb, self.config.working_curve);
        let log_rgb = rgb;

        let mg = 0.5;
        let lmg = self.config.working_mid_grey;
        rgb[0] = tone_scale(rgb[0], self.config.shoulder_power, self.config.toe_power, self.config.slope, lmg, mg, 1.0, 0.0);
        rgb[1] = tone_scale(rgb[1], self.config.shoulder_power, self.config.toe_power, self.config.slope, lmg, mg, 1.0, 0.0);
        rgb[2] = tone_scale(rgb[2], self.config.shoulder_power, self.config.toe_power, self.config.slope, lmg, mg, 1.0, 0.0);

        if self.config.log_output {
            return log_rgb;
        }

        rgb[0] = rgb[0].powf(2.2);
        rgb[1] = rgb[1].powf(2.2);
        rgb[2] = rgb[2].powf(2.2);

        let lum = (rgb[0] + rgb[1] + rgb[2]) * (1.0 / 3.0);
        let max_ch = rgb[0].max(rgb[1]).max(rgb[2]);
        if max_ch > 0.85 {
            let t = ((max_ch - 0.85) / 0.15).min(1.0);
            rgb[0] = rgb[0] + (lum - rgb[0]) * t;
            rgb[1] = rgb[1] + (lum - rgb[1]) * t;
            rgb[2] = rgb[2] + (lum - rgb[2]) * t;
        }

        rgb = mat_mul_vec3(&self.outset_mat, rgb);

        rgb = mat_mul_vec3(&self.gamut_mat, rgb);

        rgb = inverse_eotf(rgb, self.out_gamma);

        rgb[0] = rgb[0].max(0.0);
        rgb[1] = rgb[1].max(0.0);
        rgb[2] = rgb[2].max(0.0);

        rgb
    }

    pub fn process_frame(&self, pixels: &mut [f32]) {
        pixels.par_chunks_exact_mut(3).for_each(|chunk| {
            let out = self.process_pixel(chunk[0], chunk[1], chunk[2]);
            chunk[0] = out[0];
            chunk[1] = out[1];
            chunk[2] = out[2];
        });
    }

    pub fn config(&self) -> &AgxConfig {
        &self.config
    }
}

#[inline]
fn mat_mul_vec3(m: &[f32; 9], v: [f32; 3]) -> [f32; 3] {
    [
        v[0] * m[0] + v[1] * m[3] + v[2] * m[6],
        v[0] * m[1] + v[1] * m[4] + v[2] * m[7],
        v[0] * m[2] + v[1] * m[5] + v[2] * m[8],
    ]
}

#[derive(Clone, Copy)]
struct Chromaticities {
    r: [f32; 2],
    g: [f32; 2],
    b: [f32; 2],
    w: [f32; 2],
}

fn gamut_chromaticities(g: Gamut) -> Chromaticities {
    const D65: [f32; 2] = [0.3127, 0.3290];
    match g {
        Gamut::Ap0 => Chromaticities { r: [0.7347, 0.2653], g: [0.0000, 1.0000], b: [0.0001, -0.0770], w: D65 },
        Gamut::Ap1 => Chromaticities { r: [0.7130, 0.2930], g: [0.1650, 0.8300], b: [0.1280, 0.0440], w: D65 },
        Gamut::Rec709 => Chromaticities { r: [0.6400, 0.3300], g: [0.3000, 0.6000], b: [0.1500, 0.0600], w: D65 },
        Gamut::Rec2020 => Chromaticities { r: [0.7080, 0.2920], g: [0.1700, 0.7970], b: [0.1310, 0.0460], w: D65 },
        Gamut::P3D65 => Chromaticities { r: [0.6800, 0.3200], g: [0.2650, 0.6900], b: [0.1500, 0.0600], w: D65 },
        Gamut::P3D60 => Chromaticities { r: [0.6800, 0.3200], g: [0.2650, 0.6900], b: [0.1500, 0.0600], w: D65 },
        Gamut::SGamut3Cine => Chromaticities { r: [0.7660, 0.2750], g: [0.2250, 0.8000], b: [0.0890, -0.0870], w: D65 },
        Gamut::Awg3 => Chromaticities { r: [0.6840, 0.3130], g: [0.2210, 0.8480], b: [0.0861, -0.1020], w: D65 },
        Gamut::Awg4 => Chromaticities { r: [0.6800, 0.3150], g: [0.2200, 0.8500], b: [0.0860, -0.1000], w: D65 },
        Gamut::Rwg => Chromaticities { r: [0.7300, 0.2800], g: [0.1400, 0.8550], b: [0.1000, -0.0900], w: D65 },
        Gamut::SGamut3 => Chromaticities { r: [0.7500, 0.2700], g: [0.2100, 0.8000], b: [0.1000, -0.0500], w: D65 },
        Gamut::BlackmagicWg => Chromaticities { r: [0.7500, 0.2700], g: [0.2100, 0.8000], b: [0.1000, -0.0500], w: D65 },
        Gamut::CanonCinema => Chromaticities { r: [0.7400, 0.2700], g: [0.1700, 0.7900], b: [0.0800, -0.1000], w: D65 },
        Gamut::DaVinciWg => Chromaticities { r: [0.7350, 0.2650], g: [0.2150, 0.8100], b: [0.1200, -0.0500], w: D65 },
        Gamut::EGamut => Chromaticities { r: [0.7300, 0.2800], g: [0.1700, 0.8000], b: [0.1000, -0.0600], w: D65 },
    }
}

fn rgb_to_rgb(src: Chromaticities, dst: Chromaticities) -> [f32; 9] {
    let src_to_xyz = rgb_to_xyz(src);
    let xyz_to_dst = xyz_to_rgb(dst);
    mat_mul_3x3(&xyz_to_dst, &src_to_xyz)
}

fn rgb_to_xyz(c: Chromaticities) -> [f32; 9] {
    let [rx, ry] = c.r;
    let [gx, gy] = c.g;
    let [bx, by] = c.b;
    let [wx, wy] = c.w;

    let rz = 1.0 - rx - ry;
    let gz = 1.0 - gx - gy;
    let bz = 1.0 - bx - by;
    let wz = 1.0 - wx - wy;

    let m = [
        rx, gx, bx,
        ry, gy, by,
        rz, gz, bz,
    ];
    let inv = invert_3x3(&m);
    let s = mat_mul_vec3(&inv, [wx / wy, 1.0, wz / wy]);

    [
        inv[0] * s[0], inv[1] * s[1], inv[2] * s[2],
        inv[3] * s[0], inv[4] * s[1], inv[5] * s[2],
        inv[6] * s[0], inv[7] * s[1], inv[8] * s[2],
    ]
}

fn xyz_to_rgb(c: Chromaticities) -> [f32; 9] {
    invert_3x3(&rgb_to_xyz(c))
}

fn mat_mul_3x3(a: &[f32; 9], b: &[f32; 9]) -> [f32; 9] {
    let mut out = [0.0; 9];
    for i in 0..3 {
        for j in 0..3 {
            out[i * 3 + j] = a[i * 3] * b[j] + a[i * 3 + 1] * b[3 + j] + a[i * 3 + 2] * b[6 + j];
        }
    }
    out
}

fn invert_3x3(m: &[f32; 9]) -> [f32; 9] {
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

fn inset_primaries(
    c: Chromaticities,
    att_r: f32,
    att_g: f32,
    att_b: f32,
    rot_r: f32,
    rot_g: f32,
    rot_b: f32,
) -> Chromaticities {
    fn shift(xy: [f32; 2], w: [f32; 2], attenuation: f32, rotation_deg: f32) -> [f32; 2] {
        let dx = xy[0] - w[0];
        let dy = xy[1] - w[1];
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < 1e-6 {
            return xy;
        }
        let scale = 1.0 - attenuation;
        let angle = rotation_deg * PI / 180.0;
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        let rx = (dx * cos_a - dy * sin_a) * scale;
        let ry = (dx * sin_a + dy * cos_a) * scale;
        [w[0] + rx, w[1] + ry]
    }
    Chromaticities {
        r: shift(c.r, c.w, att_r, rot_r),
        g: shift(c.g, c.w, att_g, rot_g),
        b: shift(c.b, c.w, att_b, rot_b),
        w: c.w,
    }
}

#[inline]
fn log_to_lin(v: [f32; 3], tf: Transfer) -> [f32; 3] {
    let f = |x: f32| -> f32 {
        match tf {
            Transfer::Linear => x,
            Transfer::AcesCct => {
                if x <= 0.155251141552511 {
                    (x - 0.077415122655251) / 10.5402377416545
                } else {
                    2.0f32.powf((x - 0.413588402493492) * 17.52)
                }
            }
            Transfer::SonySLog3 => {
                if x < 0.01125 {
                    (x - 0.030001222851889) / 0.01125
                } else {
                    10.0f32.powf((x - 0.42) / 0.24) * 0.18
                }
            }
            Transfer::SonySLog2 => {
                if x < 0.01018 {
                    (x - 0.0245786) / 0.0183089
                } else {
                    10.0f32.powf((x - 0.384410) / 0.235443) * 0.18
                }
            }
            Transfer::ArriLogC3 => {
                if x <= 0.010591 {
                    (x - 0.005228) / 0.047491
                } else {
                    0.18 * 2.0f32.powf((x - 0.385537) / 0.247190)
                }
            }
            Transfer::ArriLogC4 => {
                if x <= 0.010591 {
                    (x - 0.005228) / 0.047491
                } else {
                    0.18 * 2.0f32.powf((x - 0.385537) / 0.247190)
                }
            }
            Transfer::RedLog3G10 => {
                if x < 0.0 {
                    0.0
                } else {
                    (10.0f32.powf(x * 17.52 - 14.0) - 0.001) / 9.999
                }
            }
            Transfer::VLog => {
                if x <= 0.010592 {
                    (x - 0.005255) / 0.047120
                } else {
                    0.18 * 2.0f32.powf((x - 0.389583) / 0.244949)
                }
            }
            Transfer::FLog2 => {
                if x < 0.0 {
                    0.0
                } else {
                    let a = 0.86445045;
                    let b = 0.5779536;
                    let c = 0.13967949;
                    let d = 0.4174028;
                    ((x - b) / a).powf(1.0 / c) - d
                }
            }
            Transfer::FLog2C => {
                if x < 0.0 {
                    0.0
                } else {
                    let a = 0.86445045;
                    let b = 0.5779536;
                    let c = 0.13967949;
                    let d = 0.4174028;
                    ((x - b) / a).powf(1.0 / c) - d
                }
            }
            Transfer::AppleLog2 => {
                if x < 0.0 {
                    0.0
                } else {
                    1.0 / (1.0 + (-4.0 * x).exp())
                }
            }
            Transfer::BmFilmGen5 => {
                if x < 0.125 {
                    x * 4.0
                } else {
                    1.0 + (x - 0.5).ln() / 0.693147
                }
            }
            Transfer::CanonLog3 => {
                if x < 0.0149 {
                    (x - 0.0721) / 3.5766
                } else {
                    0.18 * 10.0f32.powf((x - 0.406539) / 0.301940)
                }
            }
            Transfer::CanonLog2 => {
                if x < 0.0205 {
                    (x - 0.078516) / 2.4397
                } else {
                    0.18 * 10.0f32.powf((x - 0.419398) / 0.283691)
                }
            }
            Transfer::DaVinciIntermediate => {
                if x < 0.0 {
                    0.0
                } else if x < 0.5 {
                    x * x * 4.0
                } else {
                    1.0 + (x - 0.5).ln() / 0.693147
                }
            }
            Transfer::FilmlightTLog => {
                if x <= 0.0 {
                    0.0
                } else {
                    let a = 1.0 / (10.0_f32.powf(0.002) - 1.0);
                    (a + 1.0).powf(x - 1.0) - a
                }
            }
            Transfer::AgxLogKraken => {
                0.18 * 2.0f32.powf(x * 16.5 - 10.0)
            }
            Transfer::HLG => {
                if x <= 0.5 {
                    x * x * 4.0
                } else {
                    let beta = 1.09929682680944 - 1.0;
                    let gamma = 0.5;
                    ((x + beta - 1.0) / beta).powf(1.0 / gamma)
                }
            }
            Transfer::PQ => {
                if x < 0.0 {
                    0.0
                } else {
                    let n = x.cbrt();
                    let m = (10000.0_f32.powf(1.0 / 2.4) - 1.0) / 10000.0_f32.powf(1.0 / 2.4);
                    ((n - m) / (1.0 - m)).powf(2.4)
                }
            }
            Transfer::DNG => x,
            Transfer::DI => x,
        }
    };
    [f(v[0]), f(v[1]), f(v[2])]
}

#[inline]
fn lin_to_log(v: [f32; 3], tf: Transfer) -> [f32; 3] {
    let f = |x: f32| -> f32 {
        match tf {
            Transfer::Linear => x,
            Transfer::AcesCct => {
                if x <= 0.0078125 {
                    x * 10.5402377416545 + 0.077415122655251
                } else {
                    (x / 2.0).log2() / 17.52 + 0.413588402493492
                }
            }
            Transfer::SonySLog3 => {
                if x < 0.0 {
                    0.030001222851889
                } else {
                    0.24 * (x / 0.18).log10() + 0.42
                }
            }
            Transfer::SonySLog2 => {
                if x < 0.0 {
                    0.0245786
                } else {
                    0.235443 * (x / 0.18).log10() + 0.384410
                }
            }
            Transfer::ArriLogC3 => {
                if x <= 0.005228 {
                    x * 0.047491 + 0.005228
                } else {
                    0.247190 * (x / 0.18).log2() + 0.385537
                }
            }
            Transfer::ArriLogC4 => {
                if x <= 0.005228 {
                    x * 0.047491 + 0.005228
                } else {
                    0.247190 * (x / 0.18).log2() + 0.385537
                }
            }
            Transfer::RedLog3G10 => {
                if x < 0.0 {
                    0.0
                } else {
                    ((x * 9.999 + 0.001).log10() / 17.52) + 14.0
                }
            }
            Transfer::VLog => {
                if x <= 0.005255 {
                    x * 0.047120 + 0.005255
                } else {
                    0.244949 * (x / 0.18).log2() + 0.389583
                }
            }
            Transfer::FLog2 => {
                if x < 0.0 {
                    0.0
                } else {
                    let a = 0.86445045;
                    let b = 0.5779536;
                    let c = 0.13967949;
                    let d = 0.4174028;
                    a * (x + d).powf(c) + b
                }
            }
            Transfer::FLog2C => {
                if x < 0.0 {
                    0.0
                } else {
                    let a = 0.86445045;
                    let b = 0.5779536;
                    let c = 0.13967949;
                    let d = 0.4174028;
                    a * (x + d).powf(c) + b
                }
            }
            Transfer::AppleLog2 => {
                (-(1.0 / x - 1.0).ln()) / 4.0
            }
            Transfer::BmFilmGen5 => {
                if x < 0.5 {
                    x / 4.0
                } else {
                    0.5 + 0.693147 * (x - 1.0).exp()
                }
            }
            Transfer::CanonLog3 => {
                if x < 0.00390625 {
                    x * 3.5766 + 0.0721
                } else {
                    0.301940 * (x / 0.18).log10() + 0.406539
                }
            }
            Transfer::CanonLog2 => {
                if x < 0.00390625 {
                    x * 2.4397 + 0.078516
                } else {
                    0.283691 * (x / 0.18).log10() + 0.419398
                }
            }
            Transfer::DaVinciIntermediate => {
                if x < 0.0 {
                    0.0
                } else if x < 0.25 {
                    x.sqrt() / 2.0
                } else {
                    0.5 + 0.693147 * (x - 1.0).exp()
                }
            }
            Transfer::FilmlightTLog => {
                if x <= 0.0 {
                    0.0
                } else {
                    let a = 1.0 / (10.0_f32.powf(0.002) - 1.0);
                    1.0 + (x / a + 1.0).ln() / (a + 1.0).ln()
                }
            }
            Transfer::AgxLogKraken => {
                if x <= 0.0 {
                    0.0
                } else {
                    let ev = (x / 0.18).log2().clamp(-10.0, 6.5);
                    (ev + 10.0) / 16.5
                }
            }
            Transfer::HLG => {
                if x < 0.0 {
                    0.0
                } else if x <= 0.5 {
                    x.sqrt() / 2.0
                } else {
                    let beta = 1.09929682680944 - 1.0;
                    let gamma = 0.5;
                    beta * (x).powf(gamma) + 1.0 - beta
                }
            }
            Transfer::PQ => {
                if x < 0.0 {
                    0.0
                } else {
                    let m = (10000.0_f32.powf(1.0 / 2.4) - 1.0) / 10000.0_f32.powf(1.0 / 2.4);
                    1.0 - (x.max(0.0).cbrt() * (1.0 - m) - m).abs()
                }
            }
            Transfer::DNG => x,
            Transfer::DI => x,
        }
    };
    [f(v[0]), f(v[1]), f(v[2])]
}

#[inline]
fn inverse_eotf(v: [f32; 3], gamma: f32) -> [f32; 3] {
    if gamma == 1.0 {
        return v;
    }
    let g = 1.0 / gamma;
    [v[0].powf(g), v[1].powf(g), v[2].powf(g)]
}

#[inline]
fn spowf(a: f32, b: f32) -> f32 {
    let s = if a > 0.0 { 1.0 } else if a < 0.0 { -1.0 } else { 0.0 };
    s * a.abs().powf(b)
}

#[inline]
fn tone_scale(x: f32, shoulder: f32, toe: f32, slope: f32, lmg: f32, mg: f32, s0: f32, t0: f32) -> f32 {
    let ss = spowf(
        (spowf(slope * (s0 - lmg) / (1.0 - mg), shoulder) - 1.0) * spowf(slope * (s0 - lmg), -shoulder),
        -1.0 / shoulder,
    );
    let ms = slope * (x - lmg) / ss;
    let fs = ms / spowf(1.0 + spowf(ms, shoulder), 1.0 / shoulder);

    let ts = spowf(
        (spowf(slope * (lmg - t0) / mg, toe) - 1.0) * spowf(slope * (lmg - t0), -toe),
        -1.0 / toe,
    );
    let mr = slope * (x - lmg) / (-ts);
    let ft = mr / spowf(1.0 + spowf(mr, toe), 1.0 / toe);

    if x >= lmg { ss * fs + mg } else { -ts * ft + mg }
}

impl From<crate::color::TransferFunction> for Transfer {
    fn from(tf: crate::color::TransferFunction) -> Self {
        match tf {
            crate::color::TransferFunction::Linear => Transfer::Linear,
            crate::color::TransferFunction::Rec709 => Transfer::Linear,
            crate::color::TransferFunction::SLog3 => Transfer::SonySLog3,
            crate::color::TransferFunction::VLog => Transfer::VLog,
            crate::color::TransferFunction::ARRIlog3 => Transfer::ArriLogC3,
            crate::color::TransferFunction::CLog3 => Transfer::CanonLog3,
            crate::color::TransferFunction::FLog2 => Transfer::FLog2,
            crate::color::TransferFunction::ACESCCT => Transfer::AcesCct,
            crate::color::TransferFunction::HLG => Transfer::HLG,
            crate::color::TransferFunction::PQ => Transfer::PQ,
            crate::color::TransferFunction::DaVinciIntermediate => Transfer::DaVinciIntermediate,
            crate::color::TransferFunction::Gamma24 => Transfer::Linear,
        }
    }
}

impl From<crate::color::ColorSpace> for Gamut {
    fn from(cs: crate::color::ColorSpace) -> Self {
        match cs {
            crate::color::ColorSpace::Rec709 => Gamut::Rec709,
            crate::color::ColorSpace::Rec2020 => Gamut::Rec2020,
            crate::color::ColorSpace::DciP3 => Gamut::P3D65,
            crate::color::ColorSpace::Srgb => Gamut::Rec709,
            crate::color::ColorSpace::SGamut3Cine => Gamut::SGamut3Cine,
            crate::color::ColorSpace::SGamut3 => Gamut::SGamut3,
            crate::color::ColorSpace::ARRIWideGamut3 => Gamut::Awg3,
            crate::color::ColorSpace::ARRIWideGamut4 => Gamut::Awg4,
            crate::color::ColorSpace::CanonCinemaGamut => Gamut::CanonCinema,
            crate::color::ColorSpace::PanasonicVGamut => Gamut::Rwg,
            crate::color::ColorSpace::ACESAP1 => Gamut::Ap1,
            crate::color::ColorSpace::FGamut => Gamut::Rwg,
            crate::color::ColorSpace::FGamutC => Gamut::Ap0,
            crate::color::ColorSpace::DaVinciWideGamut => Gamut::DaVinciWg,
            crate::color::ColorSpace::AppleDisplayP3 => Gamut::P3D65,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agx_pipeline_creation() {
        let cfg = AgxConfig::default();
        let pipe = AgxPipeline::new(cfg);
        assert!(pipe.inset_mat.iter().any(|&v| v != 0.0));
    }

    #[test]
    fn test_mid_grey_pivot() {
        let cfg = AgxConfig::default();
        let pipe = AgxPipeline::new(cfg);
        let out = pipe.process_pixel(0.18, 0.18, 0.18);
        assert!((out[0] - 0.5).abs() < 0.01, "mid grey: expected ~0.5, got {}", out[0]);
    }

    #[test]
    fn test_black_output() {
        let cfg = AgxConfig::default();
        let pipe = AgxPipeline::new(cfg);
        let out = pipe.process_pixel(0.0, 0.0, 0.0);
        assert!(out[0] < 0.001, "black: expected near 0, got {}", out[0]);
    }

    #[test]
    fn test_white_clip() {
        let cfg = AgxConfig::default();
        let pipe = AgxPipeline::new(cfg);
        let out = pipe.process_pixel(10.0, 10.0, 10.0);
        assert!(out[0] < 1.0 && out[0] > 0.9, "white: expected near 1.0, got {}", out[0]);
    }

    #[test]
    fn test_gamut_conversion_rec2020_to_rec709() {
        let mut cfg = AgxConfig::default();
        cfg.in_gamut = Gamut::Rec2020;
        cfg.out_gamut = Gamut::Rec709;
        
        let pipe = AgxPipeline::new(cfg);
        let out = pipe.process_pixel(0.5, 0.5, 0.5);
        
        assert!(out[0] >= 0.0 && out[0] <= 1.0);
        assert!(out[1] >= 0.0 && out[1] <= 1.0);
        assert!(out[2] >= 0.0 && out[2] <= 1.0);
    }
}