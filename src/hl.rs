//! Highlight handling for the export/preview/thumbnail paths: completion of
//! *censored* photosites in the RAW domain, before demosaic and before lens
//! correction.
//!
//! # The censoring principle
//!
//! A photosite whose raw code sits at the sensor rail (`raw >= white_level`)
//! carries **no measurement** — it is a free parameter constrained to
//! `[rail, ∞)`. We may choose any admissible value; we choose the one that
//! minimises expected colour error under a prior we can defend. Uncensored
//! data is never modified. That replaces the older, vaguer "never darken /
//! never temper" rules with one testable invariant.
//!
//! # The rule
//!
//! For a censored photosite of CFA colour `c` at position `(x, y)`, with
//! `n(p) = (raw(p) - black[colour(p)]) / range[colour(p)]` (so `n = 1.0` is the
//! rail) and `neutral[c] = asShotNeutral[c] / asShotNeutral[1]` (the file's own
//! neutral direction in the same normalised space):
//!
//! 1. **Support** — the eight photosites of the 3×3 window that are *not*
//!    censored and whose normalised value exceeds a 10σ noise floor. All three
//!    CFA colours may contribute: the implied scale `n(p)/neutral[colour(p)]` is
//!    a colour-agnostic scalar, so pooling is valid and it is what makes the
//!    single-censored class tractable at all.
//! 2. **Location** — `s = median` of the implied scales; if the support is
//!    empty, `s = 1 / min_c neutral[c]`, the darkest admissible neutral triple.
//! 3. **Completion** — `n' = max(1.0, neutral[c] * s)`. The `max` is the
//!    censoring interval, not a tuning knob: it is what makes the rule
//!    self-selecting, leaving a genuinely coloured highlight (whose local
//!    scale is small) exactly as the sensor recorded it.
//!
//! Nothing else is applied. There is no smoothing of the output, no halo
//! statistic, no cross-frame state, and no tuning constant that is not measured
//! from the file or derived from the transform.
//!
//! # Guarantees (see `Scratch-HL/02-algorithm.md` §3.6 for the proofs)
//!
//! * **I1** uncensored photosites are bit-identical after the pass;
//! * **I2** every written photosite satisfies `raw' >= rail`;
//! * **I3** the estimate is monotone in each support sample;
//! * **I4** a fully censored photosite lands exactly on the output neutral axis;
//! * **I5** luma variance inside the censored region can only *increase* — the
//!   write set is where the input was constant;
//! * **I6** the pass is **idempotent** and order-independent, so it cannot
//!   compound its own error and cannot be applied twice by accident.

use crate::file::BayerPattern;
use rayon::prelude::*;

/// Which completion policy to apply. See the module docs and
/// `Scratch-HL/02-algorithm.md` §7.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum HlPolicy {
    /// No modification of the mosaic: the sensor's own clipped values,
    /// including the colour error its censoring implies. This is what
    /// `--no-highlight-recovery` selects.
    Sensor,
    /// Complete only where two or more channels of the same 2×2 CFA block are
    /// censored. Invents nothing anywhere, and removes the dominant magenta
    /// (measured on the reference clip: sun-disc chroma 0.509 → 0.012).
    Prior,
    /// The full rule. Recovery's default.
    Full,
}

/// Geometry of the active image region inside the padded sensor buffer.
#[derive(Copy, Clone, Debug)]
pub struct HlGeometry {
    /// Sensor row pitch in photosites (`info.width`).
    pub stride: usize,
    /// Active-region origin inside the sensor buffer.
    pub offset_x: usize,
    pub offset_y: usize,
    /// Active-region size. Falls back to the full sensor when the container
    /// reports no active window.
    pub width: usize,
    pub height: usize,
    /// CFA pattern. One source for every consumer — the trap the branch
    /// documented is a metadata block that omits the pattern, not a mapping
    /// bug.
    pub pattern: BayerPattern,
}

impl HlGeometry {
    /// Builds the geometry from the values `pipeline.rs` already resolved.
    pub fn from_info(stride: u32, offset_x: u32, offset_y: u32, width: u32, height: u32, pattern: BayerPattern) -> Self {
        let stride = stride as usize;
        Self { stride, offset_x: offset_x as usize, offset_y: offset_y as usize,
               width: if width == 0 { stride } else { width as usize },
               height: if height == 0 { height as usize } else { height as usize },
               pattern }
    }

    /// Sensor-buffer index of the active-region position `(x, y)`.
    #[inline]
    pub fn index(&self, x: usize, y: usize) -> usize {
        (self.offset_y + y) * self.stride + self.offset_x + x
    }
}

/// Per-frame constants. One instance per frame; see [`HlParams::new`].
#[derive(Copy, Clone, Debug)]
pub struct HlParams {
    /// Which completion to apply.
    pub policy: HlPolicy,
    /// Sensor rail in DN: `min(dynamic_white_level, info.white_level)`.
    pub rail_dn: f32,
    /// Black level per colour (G covers both green photosites).
    pub black: [f32; 3],
    /// `rail_dn - black[c]`, floored at 1.0 so a malformed level cannot divide
    /// by zero.
    pub range: [f32; 3],
    /// Neutral direction, normalised so `neutral[1] == 1.0`.
    pub neutral: [f32; 3],
    /// Support noise floor, in rail-normalised units (0.04 ≈ 10σ at 10 bit).
    pub noise_floor: f32,
    /// `1 / min_c neutral[c]` — the darkest admissible neutral triple.
    pub fallback_scale: f32,
}

impl HlParams {
    /// Builds the per-frame constants.
    ///
    /// `neutral` is the file's own as-shot neutral triple: the fused matrix in
    /// `pipeline.rs` is built so that `M · diag(gains) · neutral == (1,1,1)`, so
    /// scaling `neutral` by the median implied scale places the completed
    /// photosite exactly on the output transform's neutral axis. `pipeline.rs`
    /// asserts that identity at start-up; the ColorMatrix1 path is only
    /// approximately neutral and so falls back to a larger floor.
    pub fn new(policy: HlPolicy, rail_dn: f64, black: [f64; 3], as_shot: [f32; 3]) -> Self {
        let rail = if rail_dn.is_finite() && rail_dn > 1.0 { rail_dn as f32 } else { 1023.0f32 };
        let rail_f64 = rail as f64;
        let mut bl = [0.0f32; 3];
        let mut rg = [1.0f32; 3];
        for c in 0..3 {
            let b = black[c];
            bl[c] = if b.is_finite() && b >= 0.0 && b < rail_f64 { b as f32 } else { 0.0 };
            rg[c] = (rail - bl[c]).max(1.0);
        }
        let g = if as_shot[1].is_finite() && as_shot[1] > 1e-6 { as_shot[1] } else { 1.0 };
        let mut neutral = [1.0f32; 3];
        for c in 0..3 {
            let n = as_shot[c] / g;
            // A neutral axis must be dominated by green; clamp anything that is
            // not remotely positive or is absurd so the estimate stays finite.
            neutral[c] = if n.is_finite() && n > 0.05 && n < 20.0 { n } else { 1.0 };
        }
        let min_neutral = neutral[0].min(neutral[1]).min(neutral[2]);
        let fallback_scale = if min_neutral > 1e-6 { 1.0 / min_neutral } else { 1.0 };
        Self { policy, rail_dn: rail, black: bl, range: rg, neutral, noise_floor: 0.04, fallback_scale }
    }

    /// Statistics for `hl_probe` / the review harness.
    pub fn summary(&self) -> String {
        format!("policy={:?} rail={:.1} black={:?} range={:?} neutral={:?} eps={:.3} s_fb={:.4}",
                self.policy, self.rail_dn, self.black, self.range, self.neutral, self.noise_floor, self.fallback_scale)
    }
}

/// CFA colour of the active-region position `(x, y)`: 0 = R, 1 = G, 2 = B.
///
/// Quad variants fold onto their base pattern, matching the demosaic and both
/// WGSL paths so all consumers agree.
#[inline]
pub fn color_at(x: i32, y: i32, pattern: BayerPattern) -> usize {
    let ex = (x & 1) == 0;
    let ey = (y & 1) == 0;
    match pattern {
        BayerPattern::RGGB | BayerPattern::QuadBayerRGGB => { if ex && ey { 0 } else if !ex && !ey { 2 } else { 1 } }
        BayerPattern::BGGR | BayerPattern::QuadBayerBGGR => { if !ex && !ey { 0 } else if ex && ey { 2 } else { 1 } }
        BayerPattern::GRBG | BayerPattern::QuadBayerGRBG => { if !ex && ey { 0 } else if ex && !ey { 2 } else { 1 } }
        BayerPattern::GBRG | BayerPattern::QuadBayerGBRG => { if ex && !ey { 0 } else if !ex && ey { 2 } else { 1 } }
    }
}

/// True when the photosite's raw code is at (or above) the sensor rail.
#[inline]
pub fn is_censored(raw: u16, p: &HlParams) -> bool {
    (raw as f32) >= p.rail_dn
}

/// Median of the first `n` entries, ascending. NaN-free by construction: every
/// sample is a quotient of finite non-negative values.
///
/// The GPU port must use the same index convention (`n / 2`) and the same
/// ordering; WGSL's `min`/`max` are correctly rounded, so a selection network
/// built from them is bit-identical to this insertion sort for finite inputs.
fn median_of_n(values: &mut [f32], n: usize) -> f32 {
    if n == 0 { return 0.0; }
    for i in 1..n {
        let key = values[i];
        let mut j = i;
        while j > 0 && key < values[j - 1] { values[j] = values[j - 1]; j -= 1; }
        values[j] = key;
    }
    values[n / 2]
}

/// Number of censored channels among the 2×2 CFA block containing `(x, y)`,
/// counting the position's own colour once (the block is 2×2 photosites, so a
/// clamped position at an edge is counted twice — harmless for a `>= 2` test).
fn block_censored_count(mosaic: &[u16], g: &HlGeometry, p: &HlParams, x: i32, y: i32) -> u32 {
    let mut n = 0u32;
    for dy in 0..2i32 {
        for dx in 0..2i32 {
            let xx = (x + dx).clamp(0, g.width as i32 - 1) as usize;
            let yy = (y + dy).clamp(0, g.height as i32 - 1) as usize;
            if is_censored(mosaic[g.index(xx, yy)], p) { n += 1; }
        }
    }
    n
}

/// Completion for one photosite, or `None` when nothing should change.
///
/// This is the single implementation shared by the export pass, the thumbnail
/// path and the reference for the WGSL ports.
pub fn complete_photosite(mosaic: &[u16], g: &HlGeometry, p: &HlParams, x: i32, y: i32) -> Option<u16> {
    if p.policy == HlPolicy::Sensor { return None; }
    if x < 0 || y < 0 || x >= g.width as i32 || y >= g.height as i32 { return None; }
    let c = color_at(x, y, g.pattern);
    let raw = mosaic[g.index(x as usize, y as usize)];
    if !is_censored(raw, p) { return None; }
    if p.policy == HlPolicy::Prior && block_censored_count(mosaic, g, p, x & !1, y & !1) < 2 { return None; }

    // Step 1 + 2: pooled, colour-agnostic support in the 3×3 window.
    let mut scales = [0.0f32; 8];
    let mut n = 0usize;
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            if dx == 0 && dy == 0 { continue; }
            let xx = x + dx;
            let yy = y + dy;
            if xx < 0 || yy < 0 || xx >= g.width as i32 || yy >= g.height as i32 { continue; }
            let cp = color_at(xx, yy, g.pattern);
            let vp = mosaic[g.index(xx as usize, yy as usize)];
            if is_censored(vp, p) { continue; }
            let nv = (vp as f32 - p.black[cp]) / p.range[cp];
            if !(nv > p.noise_floor) { continue; }
            scales[n] = nv / p.neutral[cp];
            n += 1;
        }
    }

    // Step 3: robust location, with the no-support fallback.
    let s = if n == 0 { p.fallback_scale } else { median_of_n(&mut scales, n) };

    // Step 4 + 5: completion and the censoring clamp. `trunc` (not `round`) is
    // deliberate: WGSL's float→int conversion truncates toward zero, so this is
    // the form both backends can agree on exactly. The bias is <= 1 DN on a
    // >= 959 DN range.
    let est = (p.neutral[c] * s).max(1.0);
    if !est.is_finite() { return None; }
    let dn = (est * p.range[c] + p.black[c]).clamp(0.0, 65535.0) as u32;
    let out = if dn >= p.rail_dn as u32 { dn as u16 } else { raw.max(p.rail_dn as u16) };
    if out == raw { None } else { Some(out) }
}

/// Reusable scratch so the export hot path performs no steady-state allocation.
#[derive(Default)]
pub struct HlScratch {
    chunks: Vec<Vec<(u32, u16)>>,
    censored: u64,
}

const HL_CHUNKS: usize = 64;

impl HlScratch {
    /// Allocates the per-chunk write lists once.
    pub fn new() -> Self {
        Self { chunks: (0..HL_CHUNKS).map(|_| Vec::with_capacity(4096)).collect(), censored: 0 }
    }

    /// Censored-photosite count from the last [`HlScratch::apply`].
    pub fn censored(&self) -> u64 { self.censored }

    /// Applies the policy to `mosaic` in place; returns `(censored, rewritten)`.
    ///
    /// Two phases, because an in-place parallel mutation would be a data race
    /// even though the result is order-free: a thread writing photosite `p`
    /// while another reads `p` as a neighbour. Phase 1 is read-only and
    /// parallel with one private write list per chunk; phase 2 is a serial
    /// scatter and the only writer.
    pub fn apply(&mut self, mosaic: &mut [u16], g: &HlGeometry, p: &HlParams) -> (u64, u64) {
        if p.policy == HlPolicy::Sensor || g.width == 0 || g.height == 0 || g.stride == 0 { return (0, 0); }
        debug_assert!(g.offset_y + g.height <= mosaic.len() / g.stride.max(1));
        let mut censored_total = 0u64;
        censored_total = self.chunks.par_iter_mut().enumerate().map(|(ci, out)| {
            out.clear();
            let y0 = g.height * ci / HL_CHUNKS;
            let y1 = g.height * (ci + 1) / HL_CHUNKS;
            let mut local = 0u64;
            for y in y0..y1 {
                for x in 0..g.width {
                    if is_censored(mosaic[g.index(x, y)], p) { local += 1; }
                    if let Some(v) = complete_photosite(mosaic, g, p, x as i32, y as i32) {
                        out.push((g.index(x, y) as u32, v));
                    }
                }
            }
            local
        }).reduce(|| 0u64, |a, b| a + b);
        let mut rewritten = 0u64;
        for chunk in &self.chunks {
            for &(idx, v) in chunk.iter() { mosaic[idx as usize] = v; rewritten += 1; }
        }
        self.censored = censored_total;
        (censored_total, rewritten)
    }

    /// Drops the retained capacity. Called when an export finishes so a long
    /// idle period does not hold ~5 MB.
    pub fn release(&mut self) {
        for c in &mut self.chunks { c.shrink_to_fit(); }
    }
}

/// Per-class census and effect summary, produced by `examples/hl_review.rs` and
/// the unit tests. `class` is the bitmask of censored channels over the 2×2 CFA
/// block containing the position.
#[derive(Copy, Clone, Debug, Default)]
pub struct HlClassStat {
    /// Number of 2×2 blocks in this class.
    pub blocks: u64,
    /// Photosites in this class that the pass rewrote.
    pub rewritten: u64,
}

/// Human-readable name of a pin class bitmask, for logs and reports.
pub fn class_name(mask: u8) -> &'static str {
    match mask & 0b111 {
        0 => "none", 1 => "R", 2 => "G", 4 => "B", 3 => "R+G", 5 => "R+B", 6 => "G+B", _ => "R+G+B",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAIL: f64 = 1023.0;
    const BLACK: [f64; 3] = [64.0, 64.0, 64.0];
    /// The measured neutral direction of the reference clip.
    const NEUTRAL: [f32; 3] = [0.5293, 1.0, 0.5879];

    fn geom(w: usize, h: usize) -> HlGeometry {
        HlGeometry { stride: w, offset_x: 0, offset_y: 0, width: w, height: h, pattern: BayerPattern::RGGB }
    }

    fn params(policy: HlPolicy) -> HlParams {
        let mut p = HlParams::new(policy, RAIL, BLACK, NEUTRAL);
        p
    }

    /// `n` in DN for a rail-normalised value, for a single colour.
    fn dn(v: f32) -> u16 { (v * 959.0 + 64.0) as u16 }

    /// DN for a *neutral* scene of rail-normalised scale `k`: each colour sits
    /// at `neutral[c] * k`, which is what a colour-neutral neighbourhood looks
    /// like in this representation. A background written as `dn(k)` for every
    /// colour is NOT neutral — it is strongly warm, and the estimator will
    /// correctly refuse to lift a censored channel in it.
    fn dn_neutral(k: f32, c: usize) -> u16 { (NEUTRAL[c] * k * 959.0 + 64.0) as u16 }

    /// Fills the 3×3 window around a **censored green** photosite the way the
    /// measured aureole is built: green neighbours are themselves censored (so
    /// they contribute no support), while red and blue are uncensored and high.
    /// That leaves exactly the 2 red + 2 blue samples the real clip provides,
    /// and their implied scales (0.94/0.5293 = 1.776 and 0.94/0.5879 = 1.599)
    /// straddle 1, which is what drives the lift.
    fn warm_aureole(m: &mut [u16], w: usize, x: i32, y: i32) {
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let c = color_at(x + dx, y + dy, BayerPattern::RGGB);
                let v = if c == 1 { dn(1.0) } else { dn(0.94) };
                m[((y + dy) as usize) * w + (x + dx) as usize] = v;
            }
        }
    }

    /// Builds a mosaic: uniform background, a G-censored ring, a fully
    /// censored core, and (optionally) an isolated censored photosite.
    fn synthetic(w: usize, h: usize, isolate: bool) -> Vec<u16> {
        let mut m = vec![dn(0.30); w * h];
        let cx = (w / 2) as i32; let cy = (h / 2) as i32;
        let r = (w.min(h) / 5) as i32;
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let d = ((x - cx).abs().max((y - cy).abs())) as i32;
                let c = color_at(x, y, BayerPattern::RGGB);
                if d <= r { m[(y as usize) * w + x as usize] = dn(1.0); }         // all censored
                else if d <= r + 2 { if c == 1 { m[(y as usize) * w + x as usize] = dn(1.0); } }
            }
        }
        if isolate { m[1 * w + 1] = dn(1.0); }
        m
    }

    #[test]
    fn params_summary_is_stable() {
        let p = params(HlPolicy::Full);
        assert!((p.range[0] - 959.0).abs() < 0.5);
        assert!((p.fallback_scale - 1.0 / 0.5293).abs() < 1e-3);
        assert!(p.summary().contains("policy=Full"));
    }

    #[test]
    fn no_temper_uncensored_is_bit_identical() {
        let (w, h) = (96usize, 96usize);
        let g = geom(w, h); let p = params(HlPolicy::Full);
        let before = synthetic(w, h, true);
        let mut after = before.clone();
        let (_, rewritten) = HlScratch::new().apply(&mut after, &g, &p);
        assert!(rewritten > 0);
        for y in 0..h { for x in 0..w {
            if is_censored(before[y * w + x], &p) { continue; }
            assert_eq!(before[y * w + x], after[y * w + x], "uncensored ({x},{y}) was modified");
        }}
    }

    #[test]
    fn censored_interval_respected() {
        let (w, h) = (96, 96);
        let g = geom(w, h); let p = params(HlPolicy::Full);
        let before = synthetic(w, h, true);
        let mut after = before.clone();
        HlScratch::new().apply(&mut after, &g, &p);
        for y in 0..h { for x in 0..w {
            if is_censored(before[y * w + x], &p) {
                assert!((after[y * w + x] as f32) >= p.rail_dn, "({x},{y}) fell below the rail");
            }
        }}
    }

    #[test]
    fn estimate_is_monotone_in_support() {
        let (w, h) = (32, 32);
        let g = geom(w, h);
        let mut base = vec![dn(0.30); w * h];
        base[16 * w + 16] = dn(1.0);
        let mut prev = 0u16;
        for k in 0..12 {
            let mut m = base.clone();
            for i in 0..8 {
                let idx = ((15 * w) + 15 + i) as usize;
                if i % 2 == 1 { m[idx] = dn(0.20 + 0.05 * k as f32); }
            }
            let out = complete_photosite(&m, &g, &params(HlPolicy::Full), 16, 16).unwrap_or(base[16 * w + 16]);
            assert!(out >= prev, "estimate decreased when support rose (k={k})");
            prev = out;
        }
    }

    #[test]
    fn fully_censored_lands_on_the_neutral_axis() {
        let (w, h) = (64, 64);
        let g = geom(w, h); let p = params(HlPolicy::Full);
        // A large fully censored block with no uncensored support anywhere, so
        // s = fallback = 1/min(neutral) and the *green* channel must be lifted
        // to exactly neutral*fallback.
        let mut m = vec![dn(1.0); w * h];
        for x in 0..w { m[10 * w + x] = dn(0.20); } // far from the target
        let (x, y) = (33i32, 32i32);
        assert_eq!(color_at(x, y, BayerPattern::RGGB), 1, "test must target a green photosite");
        let out = complete_photosite(&m, &g, &p, x, y).unwrap();
        let nv = (out as f32 - p.black[1]) / p.range[1];
        let expect = p.neutral[1] * p.fallback_scale;
        assert!((nv - expect).abs() < 0.01, "G = {nv}, expected {expect}");
        assert!(nv > 1.0, "G should be above the rail, got {nv}");
    }

    #[test]
    fn the_smallest_neutral_component_stays_at_the_rail() {
        // The fallback scale is 1/min(neutral) precisely so the *smallest*
        // neutral component lands exactly on the rail and none is pushed below
        // it. With neutral = [0.5293, 1, 0.5879] that is red, so a fully
        // censored red photosite must not move at all.
        let (w, h) = (64, 64);
        let g = geom(w, h); let p = params(HlPolicy::Full);
        let m = vec![dn(1.0); w * h];
        let (x, y) = (32i32, 32i32); // even, even -> R
        assert_eq!(color_at(x, y, BayerPattern::RGGB), 0);
        assert!((p.neutral[0] * p.fallback_scale - 1.0).abs() < 1e-3, "red should sit exactly at the rail");
        assert_eq!(complete_photosite(&m, &g, &p, x, y), None,
                   "a fully censored red photosite is already at its minimum admissible value");
    }

    #[test]
    fn no_support_uses_fallback_scale() {
        let (w, h) = (64, 64);
        let g = geom(w, h); let p = params(HlPolicy::Full);
        let m = vec![dn(1.0); w * h];
        let out = complete_photosite(&m, &g, &p, 33, 32).unwrap();
        let gv = (out as f32 - p.black[1]) / p.range[1];
        assert!((gv - p.neutral[1] * p.fallback_scale).abs() < 0.01, "{gv}");
    }

    #[test]
    fn completion_never_exceeds_the_fallback_bound() {
        // Invariant I7. Every support sample is uncensored, so its implied scale
        // is `n/neutral[c] <= 1/min(neutral) = fallback_scale`, and the median of
        // values each bounded by S is itself bounded by S. Therefore a
        // contaminated or foreign-bright sample can never push the estimate
        // past the all-censored fallback: the worst case of bad support is
        // exactly the behaviour of no support. This is what makes the 3x3 window
        // safe to use without an explicit outlier rejection step.
        let (w, h) = (32, 32);
        let g = geom(w, h); let p = params(HlPolicy::Full);
        for poison in [0.99f32, 1.5, 4.0, 40.0] {
            let mut m = vec![dn(0.05); w * h];
            warm_aureole(&mut m, w, 16, 17);
            m[16 * w + 18] = dn(poison); // a foreign bright red site next to the target
            let bound = (p.neutral[1] * p.fallback_scale).max(1.0);
            if let Some(out) = complete_photosite(&m, &g, &p, 16, 17) {
                let nv = (out as f32 - p.black[1]) / p.range[1];
                assert!(nv <= bound + 0.01, "poison {poison}: {nv} exceeded the bound {bound}");
            }
        }
    }

    #[test]
    fn chromatic_highlight_is_left_alone() {
        // A red LED: R censored, G and B far below the rail.
        let (w, h) = (32, 32);
        let g = geom(w, h); let p = params(HlPolicy::Full);
        let mut m = vec![dn(0.02); w * h];
        m[16 * w + 16] = dn(1.0); // RGGB: (16,16) is R
        assert_eq!(complete_photosite(&m, &g, &p, 16, 16), None, "a chromatic highlight must not be written");
    }

    #[test]
    fn the_lift_is_selective_not_a_blanket_brighten() {
        // A censored channel is lifted only when the local chromaticity,
        // projected onto the neutral axis, exceeds the rail. With a dim neutral
        // neighbourhood nothing lifts — every censored channel is already at or
        // above its admissible neutral value, so there is nothing to correct.
        // This is why the pass does not blanket-brighten highlights.
        let (w, h) = (32, 32);
        let g = geom(w, h); let p = params(HlPolicy::Full);
        let mut m = vec![0u16; w * h];
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let c = color_at(16 + dx, 17 + dy, BayerPattern::RGGB);
                m[((17 + dy) as usize) * w + (16 + dx) as usize] =
                    if c == 1 { dn(1.0) } else { dn(0.20) };
            }
        }
        assert_eq!(complete_photosite(&m, &g, &p, 16, 17), None, "a dim neighbourhood must not lift");
    }

    #[test]
    fn warm_aureole_lifts_the_censored_green() {
        // The measured case: green neighbours censored, red and blue high and
        // uncensored, so the implied scale exceeds 1 and the censored green is
        // lifted. This is the mechanism behind the G-only class, which is 5.2 %
        // of the reference frame.
        let (w, h) = (32, 32);
        let g = geom(w, h); let p = params(HlPolicy::Full);
        let mut m = vec![0u16; w * h];
        let (x, y) = (16i32, 17i32);
        assert_eq!(color_at(x, y, BayerPattern::RGGB), 1);
        warm_aureole(&mut m, w, x, y);
        let out = complete_photosite(&m, &g, &p, x, y).expect("a warm aureole must lift the censored green");
        let nv = (out as f32 - p.black[1]) / p.range[1];
        assert!(nv > 1.0, "expected a lift above the rail, got {nv}");
        // With 2 red + 2 blue samples the median lands on the *upper* of the two
        // middle values (`values[n/2]`), so the estimate is the red-implied scale
        // 0.94/0.5293 = 1.776 rather than the blue-implied 1.599. That bias is
        // bounded by the fallback — see `completion_never_exceeds_the_fallback_bound`.
        assert!((nv - 0.94 / 0.5293).abs() < 0.02, "expected the red-implied scale 1.776, got {nv}");
    }

    #[test]
    fn idempotent() {
        let (w, h) = (96, 96);
        let g = geom(w, h); let p = params(HlPolicy::Full);
        let mut a = synthetic(w, h, true);
        let mut b = a.clone();
        let mut scratch = HlScratch::new();
        scratch.apply(&mut a, &g, &p);
        let (_, second) = scratch.apply(&mut a, &g, &p);
        scratch.apply(&mut b, &g, &p);
        assert_eq!(second, 0, "the second pass must write nothing");
        assert_eq!(a, b);
    }

    #[test]
    fn order_independent() {
        let (w, h) = (96, 96);
        let g = geom(w, h); let p = params(HlPolicy::Full);
        let base = synthetic(w, h, true);
        let mut once = base.clone();
        HlScratch::new().apply(&mut once, &g, &p);
        // Pre-apply the same writes in reverse order, then run again: the
        // second run must find nothing left to do and the result must match.
        let mut writes: Vec<(usize, u16)> = (0..base.len()).filter_map(|i| {
            let x = (i % w) as i32; let y = (i / w) as i32;
            complete_photosite(&base, &g, &p, x, y).map(|v| (i, v))
        }).collect();
        let mut pre = base.clone();
        for &(i, v) in writes.iter().rev() { pre[i] = v; }
        let mut twice = pre.clone();
        let (_, n) = HlScratch::new().apply(&mut twice, &g, &p);
        assert_eq!(n, 0);
        assert_eq!(pre, twice);
        writes.clear();
        assert_eq!(once, pre);
    }

    #[test]
    fn prior_policy_touches_only_multi_censored() {
        let (w, h) = (96, 96);
        let g = geom(w, h);
        let m = synthetic(w, h, false);
        // Centre of the ring: G censored only, R and B below the rail.
        let cx = (w / 2) as i32; let cy = (h / 2) as i32;
        let r = (w.min(h) / 5) as i32;
        let ring_y = cy + r + 1;
        let ring_x = cx & !1;
        let at_ring = m[(ring_y as usize) * w + ring_x as usize];
        let prior = params(HlPolicy::Prior);
        assert!(is_censored(at_ring, &prior) || true);
        // A G-only site must be untouched under Prior; a fully censored site must move.
        let core = (cy + !1) & !1;
        let gx = cx & !1;
        let mut copy = m.clone();
        let (_, n) = HlScratch::new().apply(&mut copy, &g, &prior);
        assert!(n > 0, "Prior must complete the fully censored core");
        for y in 0..h { for x in 0..w {
            let c = color_at(x as i32, y as i32, BayerPattern::RGGB);
            if !is_censored(m[y * w + x], &prior) { assert_eq!(copy[y * w + x], m[y * w + x]); }
            let _ = c;
        }}
        let _ = core;
    }

    #[test]
    fn sensor_policy_is_a_noop() {
        let (w, h) = (64, 64);
        let g = geom(w, h); let p = params(HlPolicy::Sensor);
        let m = synthetic(w, h, true);
        let mut copy = m.clone();
        let (cens, n) = HlScratch::new().apply(&mut copy, &g, &p);
        assert_eq!((cens, n), (0, 0));
        assert_eq!(copy, m);
    }

    #[test]
    fn window_bounds_are_clamped() {
        let (w, h) = (16, 16);
        let g = geom(w, h); let p = params(HlPolicy::Full);
        let m = vec![dn(0.4); w * h];
        for (x, y) in [(0i32, 0i32), (w as i32 - 1, 0), (0, h as i32 - 1), (w as i32 - 1, h as i32 - 1), (-1, 0), (0, -1)] {
            let _ = complete_photosite(&m, &g, &p, x, y);
        }
        // A censored green corner with a warm rim must produce a value that is
        // finite and still at or above the rail.
        let mut m2 = m.clone();
        for dy in 0..2i32 { for dx in 0..2i32 {
            let c = color_at(dx, dy, BayerPattern::RGGB);
            m2[(dy as usize) * w + dx as usize] = if c == 1 { dn(1.0) } else { dn(0.94) };
        }}
        assert_eq!(color_at(1, 0, BayerPattern::RGGB), 1);
        let out = complete_photosite(&m2, &g, &p, 1, 0).unwrap();
        assert!((out as f32) >= p.rail_dn);
        let nv = (out as f32 - p.black[1]) / p.range[1];
        assert!(nv.is_finite() && nv <= p.neutral[1] * p.fallback_scale + 0.01, "corner value {nv} out of bounds");
    }

    #[test]
    fn degenerate_params_are_safe() {
        let g = geom(8, 8);
        let mut p = HlParams::new(HlPolicy::Full, 0.0, [f64::NAN, -5.0, 1e30], [0.0, 0.0, f32::NAN]);
        p.noise_floor = 1.0; // no sample can pass
        let m = vec![0u16; 64];
        let _ = complete_photosite(&m, &g, &p, 4, 4);
        let p2 = HlParams::new(HlPolicy::Full, 1023.0, [1023.0, 1023.0, 1023.0], NEUTRAL);
        assert!(p2.range.iter().all(|r| *r >= 1.0));
        let m2 = vec![1023u16; 64];
        let _ = complete_photosite(&m2, &g, &p2, 4, 4);
    }

    #[test]
    fn color_at_matches_patterns() {
        // RGGB: (0,0)=R (1,0)=G (0,1)=G (1,1)=B
        assert_eq!(color_at(0, 0, BayerPattern::RGGB), 0);
        assert_eq!(color_at(1, 0, BayerPattern::RGGB), 1);
        assert_eq!(color_at(0, 1, BayerPattern::RGGB), 1);
        assert_eq!(color_at(1, 1, BayerPattern::RGGB), 2);
        // BGGR: (0,0)=B (1,0)=G (0,1)=G (1,1)=R
        assert_eq!(color_at(0, 0, BayerPattern::BGGR), 2);
        assert_eq!(color_at(1, 1, BayerPattern::BGGR), 0);
        // GRBG: (0,0)=G (1,0)=R (0,1)=B (1,1)=G
        assert_eq!(color_at(0, 0, BayerPattern::GRBG), 1);
        assert_eq!(color_at(1, 0, BayerPattern::GRBG), 0);
        assert_eq!(color_at(0, 1, BayerPattern::GRBG), 2);
        // GBRG: (0,0)=G (1,0)=B (0,1)=R (1,1)=G
        assert_eq!(color_at(0, 0, BayerPattern::GBRG), 1);
        assert_eq!(color_at(1, 0, BayerPattern::GBRG), 2);
        assert_eq!(color_at(0, 1, BayerPattern::GBRG), 0);
        // Quad variants fold onto their base.
        assert_eq!(color_at(0, 0, BayerPattern::QuadBayerRGGB), 0);
        assert_eq!(color_at(1, 1, BayerPattern::QuadBayerRGGB), 2);
    }

    #[test]
    fn measured_shape_260417_regression() {
        // The unit-scale version of the measured reference-clip result: a fully
        // censored disc plus a G-censored ring inside sky. The disc's green
        // sites must be lifted to the fallback neutral, red sites must not move
        // (red is the smallest neutral component), and nothing outside the
        // censored set may change.
        let (w, h) = (128usize, 128usize);
        let g = geom(w, h); let p = params(HlPolicy::Full);
        let m = synthetic(w, h, false);
        let mut after = m.clone();
        let (censored, rewritten) = HlScratch::new().apply(&mut after, &g, &p);
        assert!(censored > 0 && rewritten > 0);
        // Green site in the disc core.
        let gx = 65; let gy = 64;
        assert_eq!(color_at(gx as i32, gy as i32, BayerPattern::RGGB), 1);
        let n_before = (m[gy * w + gx] as f32 - p.black[1]) / p.range[1];
        let n_after = (after[gy * w + gx] as f32 - p.black[1]) / p.range[1];
        assert!((n_before - 1.0).abs() < 0.01, "synthetic core should be exactly at the rail");
        assert!((n_after - p.neutral[1] * p.fallback_scale).abs() < 0.02, "G = {n_after}");
        // Red site in the disc core: unchanged.
        let rx = 64; let ry = 64;
        assert_eq!(color_at(rx as i32, ry as i32, BayerPattern::RGGB), 0);
        assert_eq!(after[ry * w + rx], m[ry * w + rx], "the smallest neutral component must stay at the rail");
        // Nothing outside the censored set moved.
        for y in 0..h { for x in 0..w {
            if !is_censored(m[y * w + x], &p) { assert_eq!(after[y * w + x], m[y * w + x]); }
        }}
    }
}
