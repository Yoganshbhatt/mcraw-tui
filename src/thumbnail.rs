use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::Result;
use lru::LruCache;

use crate::preview::pipeline::pipeline::{GpuPreviewPipeline, Ready};
use crate::preview::pipeline::params::PreviewParams;
use crate::preview::pipeline::params::{color_space_to_u32, transfer_to_u32};
use crate::terminal::{protocol, TerminalProtocol};

pub const THUMBNAIL_WIDTH: u32 = 320;
pub const THUMBNAIL_HEIGHT: u32 = 180;
pub const CACHE_MAX_ENTRIES: usize = 1000;
pub const CACHE_MAX_BYTES: usize = 50 * 1024 * 1024;

/// Compute aspect-ratio-preserving output dims that fit inside THUMBNAIL_WIDTH × THUMBNAIL_HEIGHT.
/// This is the same logic as build_preview_params in app.rs — keep in sync.
pub fn aspect_fit(raw_w: u32, raw_h: u32) -> (u32, u32) {
    let raw_aspect = raw_w as f64 / raw_h as f64;
    let target_aspect = THUMBNAIL_WIDTH as f64 / THUMBNAIL_HEIGHT as f64;
    if raw_aspect > target_aspect {
        let h = (THUMBNAIL_WIDTH as f64 / raw_aspect) as u32;
        (THUMBNAIL_WIDTH, h.max(1))
    } else {
        let w = (THUMBNAIL_HEIGHT as f64 * raw_aspect) as u32;
        (w.max(1), THUMBNAIL_HEIGHT)
    }
}

fn build_params(
    width: u32,
    height: u32,
    raw_width: u32,
    raw_height: u32,
    black_level: f32,
    white_level: f32,
    bayer_phase: u32,
) -> PreviewParams {
    PreviewParams {
        width,
        height,
        bayer_width: raw_width,
        bayer_height: raw_height,
        black_level,
        white_level,
        exposure: 0.0,
        wb_r: 1.0, wb_g: 1.0, wb_b: 1.0,
        contrast: 1.0,
        saturation: 1.0,
        shadows: 0.0,
        highlights: 0.0,
        _align0: 0.0, _align1: 0.0,
        ccm_row0: [1.0, 0.0, 0.0, 0.0],
        ccm_row1: [0.0, 1.0, 0.0, 0.0],
        ccm_row2: [0.0, 0.0, 1.0, 0.0],
        color_space: color_space_to_u32(&crate::color::ColorSpace::Rec709),
        transfer: transfer_to_u32(&crate::color::TransferFunction::Gamma24),
        adjust_enabled: 0,
        bayer_phase,
        compute_histogram: 0,
        _pad0: 0, _pad1: 0, _pad2: 0, _pad3: 0, _pad4: 0, _pad5: 0, _pad6: 0,
    }
}

static FALLBACK_PLACEHOLDER: &[u8] = include_bytes!("../assets/placeholder.sixel");

#[derive(Clone)]
pub struct CachedThumbnail {
    pub sixel: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub encode_time: Instant,
}

impl CachedThumbnail {
    pub fn byte_size(&self) -> usize {
        self.sixel.len()
    }
}

pub struct ThumbnailCache {
    inner: Mutex<LruCache<PathBuf, CachedThumbnail>>,
    current_bytes: std::sync::atomic::AtomicUsize,
    pub placeholder: Vec<u8>,
}

impl ThumbnailCache {
    pub fn new() -> Self {
        Self::new_with_placeholder(None)
    }

    pub fn new_with_placeholder(custom_path: Option<&std::path::Path>) -> Self {
        let placeholder = match custom_path {
            Some(p) => match std::fs::read(p) {
                Ok(data) => {
                    tracing::info!("loaded custom placeholder from {}", p.display());
                    data
                }
                Err(e) => {
                    tracing::warn!("failed to load custom placeholder {}: {}; using bundled", p.display(), e);
                    FALLBACK_PLACEHOLDER.to_vec()
                }
            }
            None => FALLBACK_PLACEHOLDER.to_vec(),
        };

        Self {
            inner: Mutex::new(LruCache::new(NonZeroUsize::new(CACHE_MAX_ENTRIES).unwrap())),
            current_bytes: std::sync::atomic::AtomicUsize::new(0),
            placeholder,
        }
    }

    pub fn get(&self, path: &PathBuf) -> Option<CachedThumbnail> {
        let mut cache = self.inner.lock().unwrap();
        cache.get(path).cloned()
    }

    pub fn insert(&self, path: PathBuf, thumbnail: CachedThumbnail) {
        let size = thumbnail.byte_size();
        let mut cache = self.inner.lock().unwrap();

        // Enforce byte cap: evict until we fit
        let mut evict_bytes = 0usize;
        while self.current_bytes.load(std::sync::atomic::Ordering::Relaxed) + size > CACHE_MAX_BYTES && !cache.is_empty() {
            if let Some((_, evicted)) = cache.pop_lru() {
                evict_bytes += evicted.byte_size();
            }
        }

        if let Some(old) = cache.put(path, thumbnail) {
            self.current_bytes.fetch_sub(old.byte_size(), std::sync::atomic::Ordering::Relaxed);
        }

        self.current_bytes.fetch_add(size, std::sync::atomic::Ordering::Relaxed);
        if evict_bytes > 0 {
            self.current_bytes.fetch_sub(evict_bytes, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub fn clear(&self) {
        let mut cache = self.inner.lock().unwrap();
        cache.clear();
        self.current_bytes.store(0, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

impl Default for ThumbnailCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Strip alpha channel from RGBA for Kitty f=24 raw RGB.
fn rgba_to_rgb(rgba: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
    for chunk in rgba.chunks(4) {
        rgb.push(chunk[0]);
        rgb.push(chunk[1]);
        rgb.push(chunk[2]);
    }
    rgb
}

/// Encode an RGBA buffer as Kitty graphics protocol raw RGB (`f=24`).
///
/// Uses `a=t` (transmit only, image ID=0, replace=1) to send the pixel
/// data without displaying. The caller must emit a subsequent `a=p` (place)
/// command to display the image at the cursor position — this two-step
/// approach works around WezTerm on Windows where `a=T` (transmit+display)
/// ignores the cursor position and snaps to pixel (0,0).
fn kitty_encode(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    use base64::Engine;
    let rgb = rgba_to_rgb(rgba);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&rgb);
    let header = format!("\x1b_Ga=t,i=0,r=1,f=24,s={},v={},m=0;", width, height);
    let mut out = header.into_bytes();
    out.extend_from_slice(b64.as_bytes());
    out.extend_from_slice(b"\x1b\\");
    out
}

/// Encode an RGBA buffer to the terminal's image protocol (sixel or Kitty).
/// Returns raw bytes ready to write directly to stdout.
fn encode_rgba_to_terminal(rgba: &[u8], width: usize, height: usize) -> Result<Vec<u8>> {
    match protocol() {
        TerminalProtocol::Kitty => Ok(kitty_encode(rgba, width, height)),
        TerminalProtocol::Sixel => {
            let s = icy_sixel::sixel_encode(rgba, width, height, &icy_sixel::EncodeOptions::default())
                .map_err(|e| anyhow::anyhow!("sixel encode: {}", e))?;
            Ok(s.into_bytes())
        }
        TerminalProtocol::TextFallback => {
            Err(anyhow::anyhow!("Terminal does not support image display (sixel/kitty)"))
        }
    }
}

pub fn compute_thumbnail(
    pipeline: &mut GpuPreviewPipeline<Ready>,
    bayer: &[u16],
    raw_width: u32,
    raw_height: u32,
    black_level: f32,
    white_level: f32,
    bayer_phase: u32,
) -> Result<CachedThumbnail> {
    if matches!(protocol(), TerminalProtocol::TextFallback) {
        return Err(anyhow::anyhow!("Terminal does not support image display (sixel/kitty)"));
    }
    let (width, height) = aspect_fit(raw_width, raw_height);

    let params = build_params(width, height, raw_width, raw_height, black_level, white_level, bayer_phase);

    let (rgba, w, h) = pipeline.process_and_readback(bayer, &params)?;

    let encoded = encode_rgba_to_terminal(&rgba, w as usize, h as usize)?;

    Ok(CachedThumbnail {
        sixel: encoded,
        width: w,
        height: h,
        encode_time: Instant::now(),
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// CPU thumbnail pipeline — pixel-exact port of shaders/preview.wgsl
// ═══════════════════════════════════════════════════════════════════════════════

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn load_bayer(bayer: &[u16], raw_w: u32, x: i32, y: i32) -> f32 {
    let cx = x.clamp(0, raw_w as i32 - 1);
    let cy = y.clamp(0, (bayer.len() as u32 / raw_w).saturating_sub(1) as i32);
    bayer[(cy as u32 * raw_w + cx as u32) as usize] as f32
}

fn bayer_color(x: i32, y: i32, phase: u32) -> i32 {
    let even_row = (y & 1) == 0;
    let even_col = (x & 1) == 0;
    match phase {
        0 => { // RGGB
            if even_row { if even_col { 0 } else { 1 } }
            else        { if even_col { 1 } else { 2 } }
        }
        1 => { // GRBG
            if even_row { if even_col { 1 } else { 0 } }
            else        { if even_col { 2 } else { 1 } }
        }
        2 => { // GBRG
            if even_row { if even_col { 1 } else { 2 } }
            else        { if even_col { 0 } else { 1 } }
        }
        _ => { // BGGR
            if even_row { if even_col { 2 } else { 1 } }
            else        { if even_col { 1 } else { 0 } }
        }
    }
}

fn demosaic_bilinear(bayer: &[u16], raw_w: u32, _raw_h: u32, phase: u32, x: i32, y: i32) -> [f32; 3] {
    let c = bayer_color(x, y, phase);
    let center = load_bayer(bayer, raw_w, x, y);
    let n  = load_bayer(bayer, raw_w, x, y - 1);
    let s  = load_bayer(bayer, raw_w, x, y + 1);
    let w  = load_bayer(bayer, raw_w, x - 1, y);
    let e  = load_bayer(bayer, raw_w, x + 1, y);
    let nw = load_bayer(bayer, raw_w, x - 1, y - 1);
    let ne = load_bayer(bayer, raw_w, x + 1, y - 1);
    let sw = load_bayer(bayer, raw_w, x - 1, y + 1);
    let se = load_bayer(bayer, raw_w, x + 1, y + 1);

    let (r, g, b) = if c == 0 { // R site
        (center, (n + s + w + e) * 0.25, (nw + ne + sw + se) * 0.25)
    } else if c == 2 { // B site
        ((nw + ne + sw + se) * 0.25, (n + s + w + e) * 0.25, center)
    } else { // G site
        let horiz_color = bayer_color(x - 1, y, phase);
        let _vert_color = bayer_color(x, y - 1, phase);
        if horiz_color == 0 { // R is horizontal
            ((w + e) * 0.5, center, (n + s) * 0.5)
        } else { // B is horizontal
            ((n + s) * 0.5, center, (w + e) * 0.5)
        }
    };
    [r, g, b]
}

fn apply_oetf(r: f32, g: f32, b: f32, tf: u32) -> [f32; 3] {
    let oetf_ch = |x: f32| -> f32 {
        match tf {
            0 => x,
            14 => x.max(0.0).powf(1.0 / 2.4),
            1 => {
                if x < 0.018 { 4.5 * x }
                else { 1.099 * x.max(0.0).powf(0.45) - 0.099 }
            }
            2 => {
                if x >= 0.01 { 0.432699 * (10.0 * x + 1.0).log10() + 0.037584 }
                else { (x * 261.5 + 10.23) / 1023.0 }
            }
            3 => {
                if x < 0.01 { 5.6 * x + 0.125 }
                else { 0.241514 * (x + 0.00873).log10() + 0.598206 }
            }
            4 => {
                if x > 0.010591 { 0.247190 * (5.555556 * x + 0.052272).log10() + 0.385537 }
                else { 5.367655 * x + 0.092809 }
            }
            5 => {
                let a: f32 = (262144.0 - 16.0) / 117.45;
                let b_rev: f32 = (1023.0 - 95.0) / 1023.0;
                let c_rev: f32 = 95.0 / 1023.0;
                let s_rev = (7.0 * 0.6931471805599453 * f32::exp2(7.0 - 14.0 * c_rev / b_rev)) / (a * b_rev);
                let t_rev = (f32::exp2(14.0 * (-c_rev / b_rev) + 6.0) - 64.0) / a;
                if x >= t_rev {
                    ((a * x + 64.0).log2() - 6.0) / 14.0 * b_rev + c_rev
                } else {
                    (x - t_rev) / s_rev
                }
            }
            6 => {
                let neg_graft = (0.097465473 - 0.12512219) / 1.9754798;
                let pos_graft = (0.15277891 - 0.12512219) / 1.9754798;
                if x < neg_graft {
                    -0.36726845 * ((-x * 14.98325 + 1.0).max(1e-10)).log10() + 0.12783901
                } else if x <= pos_graft {
                    1.9754798 * x + 0.12512219
                } else {
                    0.36726845 * (x * 14.98325 + 1.0).log10() + 0.12240537
                }
            }
            7 => {
                if x >= 0.000889 { 0.245281 * (5.555556 * x + 0.064829).log10() + 0.384316 }
                else { 8.799461 * x + 0.092864 }
            }
            8 | 9 => {
                if x < -0.05641088 { 0.0 }
                else if x < 0.01 { 47.28711236 * (x + 0.05641088) * (x + 0.05641088) }
                else { 0.08550479 * (x + 0.00964052).log2() + 0.69336945 }
            }
            10 => {
                if x > 0.0078125 { (x.log2() + 9.72) / 17.52 }
                else { 10.5402377416545 * x + 0.0729055341958355 }
            }
            11 => {
                let m1 = 0.1593017578125;
                let m2 = 78.84375;
                let c1 = 0.8359375;
                let c2 = 18.8515625;
                let c3 = 18.6875;
                let x_m1 = x.max(0.0).powf(m1);
                (c1 + c2 * x_m1).max(0.0) / (1.0 + c3 * x_m1).max(1e-10).powf(m2)
            }
            12 => {
                if x < 1.0 / 12.0 { (3.0 * x.max(0.0)).sqrt() }
                else { 0.17883277 * (12.0 * x - 0.28466892).max(1e-10).ln() + 0.55991073 }
            }
            13 => {
                if x <= 0.00262409 { x * 10.44426855 }
                else { 0.07329248 * ((x + 0.0075).log2() + 7.0) }
            }
            _ => x,
        }
    };
    [oetf_ch(r), oetf_ch(g), oetf_ch(b)]
}

fn inverse_oetf(r: f32, g: f32, b: f32, tf: u32) -> [f32; 3] {
    let inv_ch = |y: f32| -> f32 {
        match tf {
            0 => y,
            14 => y.max(0.0).powf(2.4),
            1 => {
                if y < 0.081 { y / 4.5 }
                else { ((y + 0.099) / 1.099).powf(1.0 / 0.45) }
            }
            2 => {
                let knee_val = (0.01 * 261.5 + 10.23) / 1023.0;
                if y >= knee_val { ((10.0f32).powf((y - 0.037584) / 0.432699) - 1.0) / 10.0 }
                else { (y * 1023.0 - 10.23) / 261.5 }
            }
            3 => {
                if y < 0.181 { (y - 0.125) / 5.6 }
                else { (10.0f32).powf((y - 0.598206) / 0.241514) - 0.00873 }
            }
            4 => {
                let knee_val = 5.367655 * 0.010591 + 0.092809;
                if y >= knee_val { ((10.0f32).powf((y - 0.385537) / 0.247190) - 0.052272) / 5.555556 }
                else { (y - 0.092809) / 5.367655 }
            }
            5 => {
                let a: f32 = (262144.0 - 16.0) / 117.45;
                let b_rev: f32 = (1023.0 - 95.0) / 1023.0;
                let c_rev: f32 = 95.0 / 1023.0;
                let s_rev = (7.0 * 0.6931471805599453 * f32::exp2(7.0 - 14.0 * c_rev / b_rev)) / (a * b_rev);
                let t_rev = (f32::exp2(14.0 * (-c_rev / b_rev) + 6.0) - 64.0) / a;
                if y >= 0.0 { (f32::exp2(14.0 * ((y - c_rev) / b_rev) + 6.0) - 64.0) / a }
                else { y * s_rev + t_rev }
            }
            6 => {
                let neg_graft = (0.097465473 - 0.12512219) / 1.9754798;
                let pos_graft = (0.15277891 - 0.12512219) / 1.9754798;
                let knee_lo = 0.12512219 + neg_graft * 1.9754798;
                let knee_hi = 0.12512219 + pos_graft * 1.9754798;
                if y < knee_lo {
                    ((10.0f32).powf(-(y - 0.12783901) / 0.36726845) - 1.0) / (-14.98325)
                } else if y <= knee_hi {
                    (y - 0.12512219) / 1.9754798
                } else {
                    ((10.0f32).powf((y - 0.12240537) / 0.36726845) - 1.0) / 14.98325
                }
            }
            7 => {
                let knee_val = 8.799461 * 0.000889 + 0.092864;
                if y >= knee_val { ((10.0f32).powf((y - 0.384316) / 0.245281) - 0.064829) / 5.555556 }
                else { (y - 0.092864) / 8.799461 }
            }
            8 | 9 => {
                if y <= 0.0 { -0.05641088 }
                else {
                    let knee_val = 47.28711236 * (0.01 + 0.05641088) * (0.01 + 0.05641088);
                    if y < knee_val { (y / 47.28711236).sqrt() - 0.05641088 }
                    else { (2.0f32).powf((y - 0.69336945) / 0.08550479) - 0.00964052 }
                }
            }
            10 => {
                let cutoff = 10.5402377416545 * 0.0078125 + 0.0729055341958355;
                if y > cutoff { (2.0f32).powf(y * 17.52 - 9.72) }
                else { (y - 0.0729055341958355) / 10.5402377416545 }
            }
            11 => {
                let m1 = 0.1593017578125;
                let m2 = 78.84375;
                let c1 = 0.8359375;
                let c2 = 18.8515625;
                let c3 = 18.6875;
                let v = y.max(0.0);
                let v_m2 = v.powf(1.0 / m2);
                let num = (v_m2 - c1).max(0.0);
                let den = c2 - c3 * v_m2;
                if den > 0.0 { (num / den).powf(1.0 / m1) }
                else { 0.0 }
            }
            12 => {
                let knee_out: f32 = f32::sqrt(3.0 / 12.0);
                if y <= knee_out { y * y / 3.0 }
                else { ((y - 0.55991073) / 0.17883277).exp() + 0.28466892 / 12.0 }
            }
            13 => {
                let cut_out = 0.00262409 * 10.44426855;
                if y <= cut_out { y / 10.44426855 }
                else { (2.0f32).powf(y / 0.07329248 - 7.0) - 0.0075 }
            }
            _ => y,
        }
    };
    [inv_ch(r), inv_ch(g), inv_ch(b)]
}

fn srgb_oetf(r: f32, g: f32, b: f32) -> [f32; 3] {
    let srgb_ch = |x: f32| -> f32 {
        if x <= 0.0031308 { x * 12.92 }
        else { 1.055 * x.max(0.0).powf(1.0 / 2.4) - 0.055 }
    };
    [srgb_ch(r), srgb_ch(g), srgb_ch(b)]
}

fn apply_tone_curve(r: f32, g: f32, b: f32, shadows: f32, highlights: f32) -> [f32; 3] {
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let shadow_weight = 1.0 - smoothstep(0.0, 0.35, luma);
    let mut rt = r + shadows * shadow_weight;
    let mut gt = g + shadows * shadow_weight;
    let mut bt = b + shadows * shadow_weight;
    let hi_weight = smoothstep(0.5, 1.0, luma);
    rt = rt + highlights * hi_weight * rt;
    gt = gt + highlights * hi_weight * gt;
    bt = bt + highlights * hi_weight * bt;
    [rt.max(0.0), gt.max(0.0), bt.max(0.0)]
}

fn xyz_to_rec709(x: f32, y: f32, z: f32) -> [f32; 3] {
    [
        3.2404542 * x + -1.9692660 * y + 0.0556434 * z,
        -1.5371385 * x + 1.8760108 * y + -0.2040259 * z,
        -0.4985314 * x + 0.0415560 * y + 1.0572252 * z,
    ]
}

fn working_to_xyz(r: f32, g: f32, b: f32, cs: u32) -> [f32; 3] {
    match cs {
        0 => [ // ACESAP1
            0.6954522414 * r + 0.1406786965 * g + 0.1638690622 * b,
            0.0447945634 * r + 0.8596711185 * g + 0.0955343182 * b,
            -0.0055258826 * r + 0.0040252104 * g + 1.0015006723 * b,
        ],
        1 => [ // Apple Wide Gamut
            1.99650669 * r + -0.04380294 * g + 0.04729625 * b,
            0.50573456 * r + 0.86522867 * g + -0.37096323 * b,
            0.00612684 * r + -0.00089651 * g + 0.99476967 * b,
        ],
        2 => [ // ARRIWideGamut3
            0.688161 * r + 0.150181 * g + 0.161658 * b,
            0.047434 * r + 0.807529 * g + 0.145037 * b,
            -0.002103 * r + -0.004533 * g + 1.006636 * b,
        ],
        3 => [ // ARRIWideGamut4
            0.732690 * r + 0.143327 * g + 0.123983 * b,
            0.044200 * r + 0.878486 * g + 0.077314 * b,
            -0.001988 * r + -0.003142 * g + 1.005130 * b,
        ],
        5 => [ // DaVinciWideGamut
            0.8000 * r + 0.3130 * g + -0.1130 * b,
            0.1682 * r + 0.9877 * g + -0.1559 * b,
            0.0790 * r + -0.1155 * g + 1.0365 * b,
        ],
        6 | 7 => [ // DciP3 / DisplayP3
            0.4865709 * r + 0.2656677 * g + 0.1982242 * b,
            0.2289746 * r + 0.6917385 * g + 0.0792869 * b,
            0.0 * r + 0.0451136 * g + 1.0439444 * b,
        ],
        11 => [ // Rec2020
            0.6369580 * r + 0.1446169 * g + 0.1688810 * b,
            0.2627002 * r + 0.6779981 * g + 0.0593017 * b,
            0.0 * r + 0.0280727 * g + 1.0609052 * b,
        ],
        _ => [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)],
    }
}

fn gamut_clip_to_srgb(r: f32, g: f32, b: f32, cs: u32) -> [f32; 3] {
    if cs == 12 || cs == 15 { // Rec709 or Srgb
        [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]
    } else {
        let xyz = working_to_xyz(r, g, b, cs);
        let srgb = xyz_to_rec709(xyz[0], xyz[1], xyz[2]);
        [srgb[0].clamp(0.0, 1.0), srgb[1].clamp(0.0, 1.0), srgb[2].clamp(0.0, 1.0)]
    }
}

/// Generate a thumbnail on the CPU, producing sixel-encoded RGBA bytes.
/// This is a pixel-exact port of the WGSL preview shader — same pipeline order,
/// same constants, same bilinear demosaic. Use for outputs ≤ 800×600 where the
/// GPU dispatch + readback overhead dominates.
pub fn cpu_thumbnail(
    bayer: &[u16],
    params: &PreviewParams,
) -> Result<(Vec<u8>, u32, u32)> {
    if matches!(protocol(), TerminalProtocol::TextFallback) {
        return Err(anyhow::anyhow!("Terminal does not support image display (sixel/kitty)"));
    }
    let out_w = params.width;
    let out_h = params.height;
    let raw_w = params.bayer_width;
    let raw_h = params.bayer_height;
    let black = params.black_level;
    let range = (params.white_level - black).max(0.001);
    let exp_gain = (2.0f32).powf(params.exposure);
    let adjust = params.adjust_enabled != 0;
    let phase = params.bayer_phase;

    let mut rgba = vec![0u8; (out_w * out_h * 4) as usize];

    // Single-pass: for each output pixel, map to bayer coordinate and process
    // the full color pipeline in the exact same order as the WGSL shader.
    for y in 0..out_h {
        for x in 0..out_w {
            let src_x = (x * raw_w) / out_w;
            let src_y = (y * raw_h) / out_h;

            // 1. Bilinear demosaic
            let mut rgb = demosaic_bilinear(bayer, raw_w, raw_h, phase, src_x as i32, src_y as i32);

            // 2. Normalize
            rgb[0] = (rgb[0] - black) / range;
            rgb[1] = (rgb[1] - black) / range;
            rgb[2] = (rgb[2] - black) / range;

            // 3. Exposure
            rgb[0] *= exp_gain;
            rgb[1] *= exp_gain;
            rgb[2] *= exp_gain;

            // 4. White balance
            rgb[0] *= params.wb_r;
            rgb[1] *= params.wb_g;
            rgb[2] *= params.wb_b;

            // 5. Camera Color Matrix (CCM)
            // NOTE: WGSL mat3x3(ccm_row0, ccm_row1, ccm_row2) is column-major:
            //   column 0 = ccm_row0, column 1 = ccm_row1, column 2 = ccm_row2
            // So ccm_row0[0] * r + ccm_row1[0] * g + ccm_row2[0] * b
            let (cr, cg, cb) = (rgb[0], rgb[1], rgb[2]);
            rgb[0] = params.ccm_row0[0] * cr + params.ccm_row1[0] * cg + params.ccm_row2[0] * cb;
            rgb[1] = params.ccm_row0[1] * cr + params.ccm_row1[1] * cg + params.ccm_row2[1] * cb;
            rgb[2] = params.ccm_row0[2] * cr + params.ccm_row1[2] * cg + params.ccm_row2[2] * cb;

            // 6. Grading adjustments (only if adjust_enabled)
            if adjust {
                rgb = apply_tone_curve(rgb[0], rgb[1], rgb[2], params.shadows, params.highlights);
                // Contrast pivot at 0.18 mid-grey
                rgb[0] = ((rgb[0] - 0.18) * params.contrast + 0.18).max(0.0);
                rgb[1] = ((rgb[1] - 0.18) * params.contrast + 0.18).max(0.0);
                rgb[2] = ((rgb[2] - 0.18) * params.contrast + 0.18).max(0.0);
                // Saturation
                let luma = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
                rgb[0] = luma + (rgb[0] - luma) * params.saturation;
                rgb[1] = luma + (rgb[1] - luma) * params.saturation;
                rgb[2] = luma + (rgb[2] - luma) * params.saturation;
            }

            // 7. Apply selected OETF
            let encoded = apply_oetf(rgb[0], rgb[1], rgb[2], params.transfer);

            // 8. Display compensation: decode OETF → linear
            let linear_for_display = inverse_oetf(encoded[0], encoded[1], encoded[2], params.transfer);

            // 9. Gamut clip to sRGB
            let srgb_linear = gamut_clip_to_srgb(linear_for_display[0], linear_for_display[1], linear_for_display[2], params.color_space);

            // 10. sRGB OETF for display
            let display = srgb_oetf(srgb_linear[0], srgb_linear[1], srgb_linear[2]);

            // 11. Pack to RGBA8
            let idx = ((y * out_w + x) * 4) as usize;
            rgba[idx] = (display[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            rgba[idx + 1] = (display[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            rgba[idx + 2] = (display[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            rgba[idx + 3] = 255;
        }
    }

    // 12. Encode to terminal protocol
    let encoded = encode_rgba_to_terminal(&rgba, out_w as usize, out_h as usize)?;

    Ok((encoded, out_w, out_h))
}
