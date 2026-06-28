struct Uniforms {
    width: u32, height: u32, filters: u32, gamma_mode: u32,
    black_level: f32, white_level: f32, wb_r: f32, wb_b: f32,
    black_r: f32, black_g: f32, black_b: f32, _black_pad: f32,
    ccm_row0: vec4<f32>, ccm_row1: vec4<f32>, ccm_row2: vec4<f32>,
    phase_x: i32, phase_y: i32,
    recon_enabled: u32, _recon_pad: u32, recon_threshold: f32, pin_thr: f32,
    recon_luma: vec4<f32>,
};

const WB_GAIN_MIN: f32 = 0.1;
const WB_GAIN_MAX: f32 = 10.0;

// Highlight reconstruction (HL-handling.md §3). Reference-channel floor:
// ratio samples whose reference value is at or below this are excluded
// (below ~1% of the white level, SNR is read-noise dominated).
const RECON_EPS: f32 = 0.01;
// Estimate ceiling factor: never above this × the largest unclipped value
// of the clipped channel in the window (texture bound).
const RECON_MAX_FACTOR: f32 = 1.5;
// Tier-1/2 window radius: 9×9 (matches RECON_WIN_R in color.rs). The
// previous 5×5 window found no clean support more than 2 px inside a
// saturated blob — the population that goes pink.
const RECON_WIN_R: u32 = 4u;
const RECON_WIN_MAX: u32 = 81u;
// Tier-3 ring search: Chebyshev radii 3..=8 (RECON_RING_MAX stays inside
// the BORDER=9 valid region). Nearest fully-clean ring = hue anchor.
const RECON_RING_MIN: u32 = 3u;
const RECON_RING_MAX: u32 = 8u;
const RECON_RING_SAMPLES: u32 = 6u;
const RECON_RING_CAP: u32 = 64u;
// Tier-3 brightness continuation (CPU Pass-B lite): nearest informative
// (mask != 111) radii 1..=16, dead-zone 4px + smoothstep ramp, m' fade.
// Radii beyond BORDER=9 sample fewer valid-region pixels at tile edges —
// median robustness bounds the error (documented lite tolerance; no 13×13
// blur on GPU — single-pixel continuation only).
const RECON_BRIGHT_MAX: u32 = 16u;
const RECON_BRIGHT_DEADZONE: f32 = 4.0;
// Bright-continuation sample count: scan outward until this many
// informative peaks are gathered (CPU Pass-B parity). Deliberately larger
// than RING_SAMPLES: single-sample medians print through as contour rims.
const RECON_BRIGHT_SAMPLES: u32 = 8u;

@group(0) @binding(0) var cfa_tex: texture_2d<u32>;
@group(0) @binding(1) var vh_tex: texture_2d<f32>;
@group(0) @binding(2) var pq_tex: texture_2d<f32>;
@group(0) @binding(3) var lp_tex: texture_2d<f32>; 
@group(0) @binding(4) var<storage, read_write> out_buf: array<u32>;
@group(0) @binding(5) var<uniform> uniforms: Uniforms;

const TILE_X: u32 = 128u;
const TILE_Y: u32 = 32u;
const BORDER: u32 = 9u;
const VALID_X: u32 = TILE_X - 2u * BORDER;
const VALID_Y: u32 = TILE_Y - 2u * BORDER;

var<workgroup> shm_r: array<f32, 128 * 32>;
var<workgroup> shm_g: array<f32, 128 * 32>;
var<workgroup> shm_b: array<f32, 128 * 32>;
// Raw-truth 2×2-block pin registry (user-approved design): one bit per
// channel per tile site, written in the LOAD loop from the pre-demosaic
// CFA value against the flat sensor-ceiling threshold `pin_thr`
// (0.99 × clip_raw in raw units — host-computed, never derived from
// recon_threshold). The collapse gate reads this registry instead of the
// demosaiced plane, so physically pinned photosites are never smoothed
// beneath the mask threshold by interpolation. Sub-threshold photosites
// carry real data and are deliberately NOT flagged.
var<workgroup> shm_pin: array<u32, 128 * 32>;

fn safe_idx(lx: i32, ly: i32) -> u32 {
    let cx = clamp(lx, 0, i32(TILE_X) - 1);
    let cy = clamp(ly, 0, i32(TILE_Y) - 1);
    return u32(cy) * TILE_X + u32(cx);
}

fn safe_sample(lx: i32, ly: i32, tile_origin_x: i32, tile_origin_y: i32, border: i32) -> vec2<i32> {
    var gx = tile_origin_x + lx - border;
    var gy = tile_origin_y + ly - border;
    gx = clamp(gx, 0, i32(uniforms.width) - 1);
    gy = clamp(gy, 0, i32(uniforms.height) - 1);
    return vec2<i32>(gx, gy);
}

fn log10(x: f32) -> f32 {
    return log(x) / 2.302585093;
}

fn bayer_color(x: i32, y: i32) -> i32 {
    let sx = x + uniforms.phase_x;
    let sy = y + uniforms.phase_y;
    let shift = u32(((((sy << 1) & 14) + (sx & 1)) << 1));
    let c = (uniforms.filters >> shift) & 3u;
    if (c == 3u) { return 1; }
    return i32(c);
}

fn read_vh(gx: i32, gy: i32) -> f32 {
    let cx = clamp(gx, 0, i32(uniforms.width) - 1);
    let cy = clamp(gy, 0, i32(uniforms.height) - 1);
    return textureLoad(vh_tex, vec2<i32>(cx, cy), 0).r;
}

fn read_pq(gx: i32, gy: i32) -> f32 {
    // P/Q discriminator is written at half-res in both X and Y by the
    // conv shader (LuisSR step 4).
    let cx = clamp(gx / 2, 0, i32(uniforms.width) / 2 - 1);
    let cy = clamp(gy / 2, 0, i32(uniforms.height) / 2 - 1);
    return textureLoad(pq_tex, vec2<i32>(cx, cy), 0).r;
}

fn read_lp(gx: i32, gy: i32) -> f32 {
    // Read the 5-tap binomial LPF pyramid at half-resolution. The
    // conv shader writes this and uses it for its own V/H and P/Q
    // gradient computation. The fill shader currently does not call
    // this (the V/H and P/Q discriminators are read directly from
    // their storage textures). Kept for future use — e.g. LuisSR's
    // refined green estimate `g_est = g_neighbor + 0.5 * (C -
    // C_neighbor) * vh_discr` where C comes from the LPF pyramid.
    let cx = clamp(gx / 2, 0, i32(uniforms.width) / 2 - 1);
    let cy = clamp(gy / 2, 0, i32(uniforms.height) / 2 - 1);
    return textureLoad(lp_tex, vec2<i32>(cx, cy), 0).r;
}

// ── Highlight reconstruction helpers (HL-handling.md §3) ────────────────
// Operate on normalized pre-WB values in shared memory. The border zone of
// the tile (within BORDER of the edge) is NOT fully demosaiced — its
// non-photosite planes were never filled by the RCD passes — so window
// samples are taken from the valid region only, plus the image-bounds
// check (mirrors the CPU's clamped window at frame edges).

fn chan(v: vec3<f32>, c: u32) -> f32 {
    if (c == 0u) { return v.x; }
    if (c == 1u) { return v.y; }
    return v.z;
}

fn set_chan(v: ptr<function, vec3<f32>>, c: u32, x: f32) {
    if (c == 0u) { (*v).x = x; }
    else if (c == 1u) { (*v).y = x; }
    else { (*v).z = x; }
}

fn wb_gain_of(c: u32) -> f32 {
    if (c == 0u) { return uniforms.wb_r; }
    if (c == 1u) { return 1.0; }
    return uniforms.wb_b;
}

// Insertion sort of the first `n` entries. Called with function-scope
// arrays only; `n` is bounded by the callers (≤ 81).
fn sort_asc(v: ptr<function, array<f32, 81u>>, n: u32) {
    for (var i: u32 = 1u; i < n; i++) {
        var j = i;
        while (j > 0u && (*v)[j] < (*v)[j - 1u]) {
            let t = (*v)[j];
            (*v)[j] = (*v)[j - 1u];
            (*v)[j - 1u] = t;
            j = j - 1u;
        }
    }
}

// Tier-1/2/3 reconstruction for one pixel. `src` is the pixel's own
// normalized pre-WB RGB (read-only); `own_mask` its clip mask (bit 0 = R,
// 1 = G, 2 = B). Returns the reconstructed RGB — pixels with a clean mask
// are returned unchanged.
//
// Tier 1 (one clipped channel): estimates keep the pixel's own pinned
// value as floor (never darken — a single clipped channel still carries
// real hue). Tier 2 (two clipped channels — the asymmetric-clip pink
// case): floor = 0 — pinning the saturated pair at the ceiling is what
// anchors the post-WB/CCM magenta cast. Tier 3 (all clipped): hue anchor
// from the nearest fully-clean Chebyshev ring (radius 3..=8); the pixel
// keeps its own measured peak brightness.
//
// Ceiling = RECON_MAX_FACTOR × the largest unclipped value of the channel
// in the window. Reconstruction results are private registers; shared
// memory is never re-written here.
fn reconstruct_pixel(src: vec3<f32>, own_mask: u32, gx: i32, gy: i32, lx: i32, ly: i32) -> vec3<f32> {
    var win_val: array<vec3<f32>, RECON_WIN_MAX>;
    var win_mask: array<u32, RECON_WIN_MAX>;
    var n_win: u32 = 0u;
    for (var dy: i32 = -i32(RECON_WIN_R); dy <= i32(RECON_WIN_R); dy++) {
        for (var dx: i32 = -i32(RECON_WIN_R); dx <= i32(RECON_WIN_R); dx++) {
            let wlx = lx + dx;
            let wly = ly + dy;
            if (wlx >= i32(BORDER) && wlx < i32(TILE_X) - i32(BORDER) && wly >= i32(BORDER) && wly < i32(TILE_Y) - i32(BORDER)) {
                let wx = gx + dx;
                let wy = gy + dy;
                if (wx >= 0 && wx < i32(uniforms.width) && wy >= 0 && wy < i32(uniforms.height)) {
                    let nidx = u32(wly) * TILE_X + u32(wlx);
                    var m: u32 = 0u;
                    if (shm_r[nidx] >= uniforms.recon_threshold) { m |= 1u; }
                    if (shm_g[nidx] >= uniforms.recon_threshold) { m |= 2u; }
                    if (shm_b[nidx] >= uniforms.recon_threshold) { m |= 4u; }
                    win_val[n_win] = vec3(shm_r[nidx], shm_g[nidx], shm_b[nidx]);
                    win_mask[n_win] = m;
                    n_win++;
                }
            }
        }
    }

    var n_clipped: u32 = 0u;
    if ((own_mask & 1u) != 0u) { n_clipped++; }
    if ((own_mask & 2u) != 0u) { n_clipped++; }
    if ((own_mask & 4u) != 0u) { n_clipped++; }

    var out = src;

    // ── Tier 3: hue anchor from the nearest fully-clean ring ──────────────
    // All channels are at the sensor ceiling — the pixel's own hue is
    // uninformative. Keep the measured peak brightness; chromaticity =
    // median of the nearest clean ring's WB'd chromaticities.
    if (n_clipped == 3u) {
        var chi: array<vec3<f32>, RECON_RING_CAP>;
        var n_ring: u32 = 0u;
        for (var r: u32 = RECON_RING_MIN; r <= RECON_RING_MAX; r++) {
            var stop = false;
            for (var dy: i32 = -i32(RECON_RING_MAX); dy <= i32(RECON_RING_MAX); dy++) {
                for (var dx: i32 = -i32(RECON_RING_MAX); dx <= i32(RECON_RING_MAX); dx++) {
                    let dist = max(abs(dx), abs(dy));
                    if (dist != i32(r)) { continue; }
                    let wlx = lx + dx;
                    let wly = ly + dy;
                    if (wlx >= i32(BORDER) && wlx < i32(TILE_X) - i32(BORDER) && wly >= i32(BORDER) && wly < i32(TILE_Y) - i32(BORDER)) {
                        let wx = gx + dx;
                        let wy = gy + dy;
                        if (wx >= 0 && wx < i32(uniforms.width) && wy >= 0 && wy < i32(uniforms.height)) {
                            let nidx = u32(wly) * TILE_X + u32(wlx);
                            var m: u32 = 0u;
                            if (shm_r[nidx] >= uniforms.recon_threshold) { m |= 1u; }
                            if (shm_g[nidx] >= uniforms.recon_threshold) { m |= 2u; }
                            if (shm_b[nidx] >= uniforms.recon_threshold) { m |= 4u; }
                            if (m == 0u) {
                                let w = vec3(shm_r[nidx], shm_g[nidx], shm_b[nidx]);
                                let ww = vec3(w.x * uniforms.wb_r, w.y, w.z * uniforms.wb_b);
                                let mx = max(ww.x, max(ww.y, ww.z));
                                if (mx > RECON_EPS) {
                                    chi[n_ring] = ww / mx;
                                    n_ring++;
                                    if (n_ring >= RECON_RING_SAMPLES) {
                                        stop = true;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                if (stop) { break; }
            }
            if (stop) { break; }
        }
        var m_chroma = vec3(1.0, 1.0, 1.0);
        if (n_ring >= RECON_RING_SAMPLES) {
            var c0: array<f32, RECON_WIN_MAX>;
            var c1: array<f32, RECON_WIN_MAX>;
            var c2: array<f32, RECON_WIN_MAX>;
            for (var j: u32 = 0u; j < n_ring; j++) {
                c0[j] = chi[j].x;
                c1[j] = chi[j].y;
                c2[j] = chi[j].z;
            }
            sort_asc(&c0, n_ring);
            sort_asc(&c1, n_ring);
            sort_asc(&c2, n_ring);
            m_chroma = vec3(c0[n_ring / 2u], c1[n_ring / 2u], c2[n_ring / 2u]);
        }
        // Brightness continuation (lite): nearest informative pixels
        // (mask != 111 — semi-clipped halo carrying Pass-A-level brightness;
        // fully-clean pixels excluded by requiring mask != 0? No — CPU uses
        // mask != 111 && mask != 0 as informative (semi-clipped only) so dim
        // clean scene never pulls the core down. Mirror exactly.
        let own_max_wb = max(src.x * uniforms.wb_r, max(src.y, src.z * uniforms.wb_b));
        var peaks: array<f32, RECON_WIN_MAX>;
        var peak_d: array<f32, RECON_WIN_MAX>;
        var n_peaks: u32 = 0u;
        for (var r: u32 = 1u; r <= RECON_BRIGHT_MAX; r++) {
            var stop_b = false;
            for (var dy: i32 = -i32(RECON_BRIGHT_MAX); dy <= i32(RECON_BRIGHT_MAX); dy++) {
                for (var dx: i32 = -i32(RECON_BRIGHT_MAX); dx <= i32(RECON_BRIGHT_MAX); dx++) {
                    if (max(abs(dx), abs(dy)) != i32(r)) { continue; }
                    let wlx = lx + dx;
                    let wly = ly + dy;
                    if (wlx >= i32(BORDER) && wlx < i32(TILE_X) - i32(BORDER) && wly >= i32(BORDER) && wly < i32(TILE_Y) - i32(BORDER)) {
                        let wx = gx + dx;
                        let wy = gy + dy;
                        if (wx >= 0 && wx < i32(uniforms.width) && wy >= 0 && wy < i32(uniforms.height)) {
                            let nidx = u32(wly) * TILE_X + u32(wlx);
                            var m: u32 = 0u;
                            if (shm_r[nidx] >= uniforms.recon_threshold) { m |= 1u; }
                            if (shm_g[nidx] >= uniforms.recon_threshold) { m |= 2u; }
                            if (shm_b[nidx] >= uniforms.recon_threshold) { m |= 4u; }
                            if (m == 7u || m == 0u) { continue; }
                            let w = vec3(shm_r[nidx], shm_g[nidx], shm_b[nidx]);
                            let mx = max(w.x * uniforms.wb_r, max(w.y, w.z * uniforms.wb_b));
                            if (mx > RECON_EPS) {
                                peaks[n_peaks] = mx;
                                peak_d[n_peaks] = f32(r);
                                n_peaks++;
                                if (n_peaks >= RECON_BRIGHT_SAMPLES) {
                                    stop_b = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                if (stop_b) { break; }
            }
            if (stop_b) { break; }
        }
        var est_peak = own_max_wb;
        var m_prime = m_chroma;
        if (n_peaks > 0u) {
            sort_asc(&peaks, n_peaks);
            sort_asc(&peak_d, n_peaks);
            let med = peaks[n_peaks / 2u];
            let d_found = peak_d[n_peaks / 2u];
            let t = clamp((d_found - RECON_BRIGHT_DEADZONE) / (f32(RECON_BRIGHT_MAX) - RECON_BRIGHT_DEADZONE), 0.0, 1.0);
            let s = t * t * (3.0 - 2.0 * t);
            est_peak = own_max_wb + (med - own_max_wb) * s;
            m_prime = m_chroma + (vec3(1.0) - m_chroma) * (1.0 - s);
        }
        if (est_peak > RECON_EPS) {
            out = vec3(est_peak * m_prime.x / uniforms.wb_r, est_peak * m_prime.y, est_peak * m_prime.z / uniforms.wb_b);
        }
        return out;
    }

    // ── Tier 1b: G-anchor upward reconstruction (deviation D10) ──────────
    // G is the single clipped channel (own_mask == 010): G is the WB anchor
    // (gain 1.0), so a neutral highlight clipped on G reconstructs UPWARD
    // from the WB'd R/B brightness. Never below the pinned value —
    // genuinely saturated colors (WB'd R/B below the pinned G) keep their
    // real hue via the floor.
    if (own_mask == 2u) {
        let g_up = max(max(src.x * uniforms.wb_r, src.z * uniforms.wb_b), src.y);
        set_chan(&out, 1, g_up);
        return out;
    }

    for (var c: u32 = 0u; c < 3u; c++) {
        if ((own_mask & (1u << c)) == 0u) { continue; }
        // Tier-1 keeps the never-darken floor; Tier-2 does not (the pinned
        // ceiling anchors the magenta cast).
        let floor_v = select(0.0, chan(src, c), n_clipped == 1u);
        var wmax: f32 = 0.0;
        for (var j: u32 = 0u; j < n_win; j++) {
            if ((win_mask[j] & (1u << c)) == 0u) {
                wmax = max(wmax, chan(win_val[j], c));
            }
        }
        let ceil_v = max(RECON_MAX_FACTOR * wmax, floor_v + 1e-6);

        if (n_clipped == 1u) {
            // Tier 1: one estimate per healthy reference channel, averaged.
            var sum: f32 = 0.0;
            var refs: u32 = 0u;
            for (var h: u32 = 0u; h < 3u; h++) {
                if (h == c) { continue; }
                var ratios: array<f32, RECON_WIN_MAX>;
                var nr: u32 = 0u;
                for (var j: u32 = 0u; j < n_win; j++) {
                    if ((win_mask[j] & (1u << c)) != 0u || (win_mask[j] & (1u << h)) != 0u) { continue; }
                    let hv = chan(win_val[j], h);
                    if (hv > RECON_EPS) {
                        ratios[nr] = chan(win_val[j], c) / hv;
                        nr++;
                    }
                }
                if (nr > 0u) {
                    sort_asc(&ratios, nr);
                    sum += chan(src, h) * ratios[nr / 2u];
                    refs++;
                }
            }
            if (refs > 0u) {
                set_chan(&out, c, clamp(sum / f32(refs), floor_v, ceil_v));
            }
        } else {
            // Tier 2: two clipped channels, one healthy anchor.
            var hh: u32 = 0u;
            if ((own_mask & 1u) == 0u) { hh = 0u; }
            else if ((own_mask & 2u) == 0u) { hh = 1u; }
            else { hh = 2u; }
            var c2: u32;
            if (c == 0u) { c2 = select(2u, 1u, (own_mask & 2u) != 0u); }
            else if (c == 1u) { c2 = select(2u, 0u, (own_mask & 1u) != 0u); }
            else { c2 = select(1u, 0u, (own_mask & 1u) != 0u); }

            var ratios: array<f32, RECON_WIN_MAX>;
            var nr: u32 = 0u;
            for (var j: u32 = 0u; j < n_win; j++) {
                if ((win_mask[j] & (1u << c)) != 0u || (win_mask[j] & (1u << hh)) != 0u) { continue; }
                let hv = chan(win_val[j], hh);
                if (hv > RECON_EPS) {
                    ratios[nr] = chan(win_val[j], c) / hv;
                    nr++;
                }
            }
            var stable: bool = nr >= 4u;
            if (nr >= 6u) {
                sort_asc(&ratios, nr);
                let med = ratios[nr / 2u];
                let q1 = ratios[nr / 4u];
                let q3 = ratios[(3u * nr) / 4u];
                if (abs(med) > 1e-6 && (q3 - q1) > 2.0 * abs(med)) { stable = false; }
            }
            if (stable && nr > 0u) {
                if (nr < 6u) { sort_asc(&ratios, nr); }
                set_chan(&out, c, clamp(chan(src, hh) * ratios[nr / 2u], floor_v, ceil_v));
            } else {
                // Luminance-continuity fallback (Resolve "Luminance"
                // style): anchor on the luma of fully-unclipped neighbors
                // plus the clipped-channel ratio from the same neighbors.
                let luma = uniforms.recon_luma.xyz;
                var y_acc: f32 = 0.0;
                var n_y: u32 = 0u;
                var k_ratios: array<f32, RECON_WIN_MAX>;
                var n_k: u32 = 0u;
                for (var j: u32 = 0u; j < n_win; j++) {
                    if (win_mask[j] != 0u) { continue; }
                    let w = win_val[j];
                    let ww = vec3(w.x * uniforms.wb_r, w.y, w.z * uniforms.wb_b);
                    y_acc += luma.x * ww.x + luma.y * ww.y + luma.z * ww.z;
                    n_y++;
                    let c2w = chan(w, c2) * wb_gain_of(c2);
                    if (c2w > RECON_EPS) {
                        k_ratios[n_k] = chan(w, c) * wb_gain_of(c) / c2w;
                        n_k++;
                    }
                }
                if (n_y > 0u && n_k > 0u) {
                    let y_mean = y_acc / f32(n_y);
                    sort_asc(&k_ratios, n_k);
                    let k = k_ratios[n_k / 2u];
                    let hw = chan(src, hh) * wb_gain_of(hh);
                    let denom = luma[c] * k + luma[c2];
                    if (abs(denom) > 1e-12) {
                        let c2w = (y_mean - luma[hh] * hw) / denom;
                        let cw = k * c2w;
                        let est_c = cw / wb_gain_of(c);
                        // WGSL has no isFinite; NaN is the only non-clampable
                        // value (self-comparison), ±Inf clamps to the ceiling.
                        if (est_c == est_c) {
                            set_chan(&out, c, clamp(est_c, floor_v, ceil_v));
                        }
                    }
                }
            }
        }
    }
    return out;
}

@compute @workgroup_size(16, 16)
fn main(
    @builtin(workgroup_id) wg_id: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let tile_origin_x = i32(wg_id.x * VALID_X);
    let tile_origin_y = i32(wg_id.y * VALID_Y);
    let thread_id = lid.y * 16u + lid.x;
    // Normalization ranges (kept for the in-place normalize pass below).
    // Per-channel, matching the CPU's `normalize_linear_per_channel`:
    // each plane divides by `white_level - black_ch`. The raw-truth pin
    // threshold is NOT derived from these: `pin_thr` is the flat
    // sensor-ceiling threshold in raw CFA units (0.99 × clip_raw),
    // computed host-side from the per-frame dynamic white level. Black
    // levels never enter the pin test — a photosite is physically pinned
    // when its raw code sits at the sensor ceiling, full stop.
    let norm_range_r = max(uniforms.white_level - uniforms.black_r, 1.0);
    let norm_range_g = max(uniforms.white_level - uniforms.black_g, 1.0);
    let norm_range_b = max(uniforms.white_level - uniforms.black_b, 1.0);

    for (var i: u32 = 0u; i < 16u; i++) {
        let idx = thread_id * 16u + i;
        let ly = idx / TILE_X;
        let lx = idx % TILE_X;
        let gx = tile_origin_x + i32(lx) - i32(BORDER);
        let gy = tile_origin_y + i32(ly) - i32(BORDER);

        var val = 0.0;
        if (gx >= 0 && gx < i32(uniforms.width) && gy >= 0 && gy < i32(uniforms.height)) {
            let cx = clamp(gx, 0, i32(uniforms.width) - 1);
            let cy = clamp(gy, 0, i32(uniforms.height) - 1);
            let raw = f32(textureLoad(cfa_tex, vec2<i32>(cx, cy), 0).r);
            let c = bayer_color(gx, gy);
            let bl = select(uniforms.black_b,
                            select(uniforms.black_g, uniforms.black_r, c == 0),
                            c == 2);
            val = max(0.0, raw - bl);
            // Raw-truth pin registry: a pre-demosaic CFA photosite whose
            // RAW code sits at/above 0.99×clip_raw (~the sensor ceiling)
            // flags its channel — no black adjustment, no reconstruction
            // coupling. Sub-threshold photosites (e.g. 0.983×WL) are NOT
            // flagged: they carry real, distinct sensor data that WB + CCM
            // may legitimately push into wide-gamut colors — the collapse
            // gate must leave those pixels alone (user-mandated). The
            // collapse gate reads this registry, not the demosaiced plane —
            // physically pinned sensor data cannot be smoothed beneath the
            // mask by interpolation.
            shm_pin[idx] = select(0u, 1u << u32(c), raw >= uniforms.pin_thr);
        } else {
            shm_pin[idx] = 0u;
        }

        let c = bayer_color(gx, gy);
        if (c == 0) { shm_r[idx] = val; }
        else if (c == 1) { shm_g[idx] = val; }
        else if (c == 2) { shm_b[idx] = val; }
    }
    workgroupBarrier();

    for (var i: u32 = 0u; i < 16u; i++) {
        let idx = thread_id * 16u + i;
        let ly = i32(idx / TILE_X);
        let lx = i32(idx % TILE_X);
        if (lx >= 2 && lx < i32(TILE_X) - 2 && ly >= 2 && ly < i32(TILE_Y) - 2) {
            let gx = tile_origin_x + lx - i32(BORDER);
            let gy = tile_origin_y + ly - i32(BORDER);
            let c = bayer_color(gx, gy);
            if (c != 1) {
                let is_red = (c == 0);

                let sc_c = select(shm_b[idx], shm_r[idx], is_red);
                let sc_n2 = select(shm_b[safe_idx(lx, ly-2)], shm_r[safe_idx(lx, ly-2)], is_red);
                let sc_s2 = select(shm_b[safe_idx(lx, ly+2)], shm_r[safe_idx(lx, ly+2)], is_red);
                let sc_w2 = select(shm_b[safe_idx(lx-2, ly)], shm_r[safe_idx(lx-2, ly)], is_red);
                let sc_e2 = select(shm_b[safe_idx(lx+2, ly)], shm_r[safe_idx(lx+2, ly)], is_red);

                let g_n = shm_g[safe_idx(lx, ly-1)];
                let g_s = shm_g[safe_idx(lx, ly+1)];
                let g_w = shm_g[safe_idx(lx-1, ly)];
                let g_e = shm_g[safe_idx(lx+1, ly)];

                let h_est = 0.5 * (g_w + g_e) + 0.25 * (2.0 * sc_c - sc_w2 - sc_e2);
                let v_est = 0.5 * (g_n + g_s) + 0.25 * (2.0 * sc_c - sc_n2 - sc_s2);

                let vh = read_vh(gx, gy);
                let vhn = 0.25 * (read_vh(gx-1, gy-1) + read_vh(gx+1, gy-1) + read_vh(gx-1, gy+1) + read_vh(gx+1, gy+1));
                let vh_discr = select(vh, vhn, abs(0.5 - vh) < abs(0.5 - vhn));

                shm_g[idx] = mix(v_est, h_est, vh_discr);
            }
        }
    }
    workgroupBarrier();

    for (var i: u32 = 0u; i < 16u; i++) {
        let idx = thread_id * 16u + i;
        let ly = i32(idx / TILE_X);
        let lx = i32(idx % TILE_X);
        if (lx >= i32(BORDER) && lx < i32(TILE_X) - i32(BORDER) && ly >= i32(BORDER) && ly < i32(TILE_Y) - i32(BORDER)) {
            let gx = tile_origin_x + lx - i32(BORDER);
            let gy = tile_origin_y + ly - i32(BORDER);
            let c = bayer_color(gx, gy);
            if (c != 1) {
                let is_red = (c == 0);
                let target_is_red = !is_red;

                let opp_nw = select(shm_b[safe_idx(lx-1, ly-1)], shm_r[safe_idx(lx-1, ly-1)], target_is_red);
                let opp_ne = select(shm_b[safe_idx(lx+1, ly-1)], shm_r[safe_idx(lx+1, ly-1)], target_is_red);
                let opp_sw = select(shm_b[safe_idx(lx-1, ly+1)], shm_r[safe_idx(lx-1, ly+1)], target_is_red);
                let opp_se = select(shm_b[safe_idx(lx+1, ly+1)], shm_r[safe_idx(lx+1, ly+1)], target_is_red);

                let g_c = shm_g[idx];
                let g_nw = shm_g[safe_idx(lx-1, ly-1)]; let g_ne = shm_g[safe_idx(lx+1, ly-1)];
                let g_sw = shm_g[safe_idx(lx-1, ly+1)]; let g_se = shm_g[safe_idx(lx+1, ly+1)];

                let diff_nw = opp_nw - g_nw; let diff_ne = opp_ne - g_ne;
                let diff_sw = opp_sw - g_sw; let diff_se = opp_se - g_se;

                let pq = read_pq(gx, gy);
                let pqn = 0.25 * (read_pq(gx-1, gy-1) + read_pq(gx+1, gy-1) + read_pq(gx-1, gy+1) + read_pq(gx+1, gy+1));
                let pq_discr = select(pq, pqn, abs(0.5 - pq) < abs(0.5 - pqn));

                let p_est = 0.5 * (diff_nw + diff_se);
                let q_est = 0.5 * (diff_ne + diff_sw);

                let final_val = g_c + mix(p_est, q_est, pq_discr);
                if (is_red) { shm_b[idx] = final_val; } else { shm_r[idx] = final_val; }
            }
        }
    }
    workgroupBarrier();

    for (var i: u32 = 0u; i < 16u; i++) {
        let idx = thread_id * 16u + i;
        let ly = i32(idx / TILE_X);
        let lx = i32(idx % TILE_X);
        if (lx >= i32(BORDER) && lx < i32(TILE_X) - i32(BORDER) && ly >= i32(BORDER) && ly < i32(TILE_Y) - i32(BORDER)) {
            let gx = tile_origin_x + lx - i32(BORDER);
            let gy = tile_origin_y + ly - i32(BORDER);
            if (bayer_color(gx, gy) == 1) {
                let is_gr = bayer_color(gx - 1, gy) == 0;
                let sg_c = shm_g[idx];

                for (var ch: i32 = 0; ch < 2; ch++) {
                    let is_horizontal = (is_gr && ch == 0) || (!is_gr && ch == 1);
                    
                    let sc_hw = select(shm_b[safe_idx(lx-1, ly)], shm_r[safe_idx(lx-1, ly)], ch == 0);
                    let sc_he = select(shm_b[safe_idx(lx+1, ly)], shm_r[safe_idx(lx+1, ly)], ch == 0);
                    let sc_vn = select(shm_b[safe_idx(lx, ly-1)], shm_r[safe_idx(lx, ly-1)], ch == 0);
                    let sc_vs = select(shm_b[safe_idx(lx, ly+1)], shm_r[safe_idx(lx, ly+1)], ch == 0);
                    
                    let sg_hw = shm_g[safe_idx(lx-1, ly)]; let sg_he = shm_g[safe_idx(lx+1, ly)];
                    let sg_vn = shm_g[safe_idx(lx, ly-1)]; let sg_vs = shm_g[safe_idx(lx, ly+1)];
                    
                    let W_est = sc_hw - sg_hw; let E_est = sc_he - sg_he;
                    let N_est = sc_vn - sg_vn; let S_est = sc_vs - sg_vs;
                    
                    let h_est = 0.5 * (W_est + E_est);
                    let v_est = 0.5 * (N_est + S_est);
                    
                    let final_val = select(sg_c + v_est, sg_c + h_est, is_horizontal);
                    
                    if (ch == 0) { shm_r[idx] = final_val; } else { shm_b[idx] = final_val; }
                }
            }
        }
    }
    workgroupBarrier();

    let ccm0 = uniforms.ccm_row0.x; let ccm1 = uniforms.ccm_row0.y; let ccm2 = uniforms.ccm_row0.z;
    let ccm3 = uniforms.ccm_row1.x; let ccm4 = uniforms.ccm_row1.y; let ccm5 = uniforms.ccm_row1.z;
    let ccm6 = uniforms.ccm_row2.x; let ccm7 = uniforms.ccm_row2.y; let ccm8 = uniforms.ccm_row2.z;

    let gm = uniforms.gamma_mode;
    let recon_on = uniforms.recon_enabled == 1u;
    let recon_thr = uniforms.recon_threshold;

    // Highlight reconstruction prep: normalize the entire tile in place
    // (the RCD fill output is dead after this point). Border positions are
    // normalized too — their non-photosite planes were never demosaiced,
    // but the reconstruction never samples them (valid-region check), so
    // whatever they hold is harmless.
    for (var i: u32 = 0u; i < 16u; i++) {
        let idx = thread_id * 16u + i;
        shm_r[idx] = shm_r[idx] / norm_range_r;
        shm_g[idx] = shm_g[idx] / norm_range_g;
        shm_b[idx] = shm_b[idx] / norm_range_b;
    }
    // The user's barrier protocol (verified): all normalized writes must be
    // visible before ANY thread reads another thread's window. No barrier
    // is needed after the reconstruction — results are private registers.
    workgroupBarrier();

    for (var i: u32 = 0u; i < 16u; i++) {
        let idx = thread_id * 16u + i;
        let ly = i32(idx / TILE_X);
        let lx = i32(idx % TILE_X);
        if (lx >= i32(BORDER) && lx < i32(TILE_X) - i32(BORDER) && ly >= i32(BORDER) && ly < i32(TILE_Y) - i32(BORDER)) {
            let gx = tile_origin_x + lx - i32(BORDER);
            let gy = tile_origin_y + ly - i32(BORDER);
            if (gx >= 0 && gx < i32(uniforms.width) && gy >= 0 && gy < i32(uniforms.height)) {

                var rn = shm_r[idx];
                var gn = shm_g[idx];
                var bn = shm_b[idx];

                // Raw-truth block override (CPU raw_truth_override parity):
                // pinned 2×2 CFA blocks replace the demosaiced triple with
                // the block's own photosite readings (R/B direct, G avg of
                // unpinned sites). Private regs only — no shm write, no
                // barrier. Clean blocks (mcoll==0) untouched (no-temper).
                // Computed here so BOTH the recon mask (m0) and the collapse
                // gate below see sensor truth, matching pipeline.rs order
                // (override → mask → recon → collapse).
                let lxb0 = lx - i32((u32(lx) + 1u) & 1u);
                let lyb0 = ly - i32((u32(ly) + 1u) & 1u);
                let bpi0 = u32(lyb0) * TILE_X + u32(lxb0);
                let mcoll_pre = shm_pin[bpi0] | shm_pin[bpi0 + 1u] | shm_pin[bpi0 + TILE_X] | shm_pin[bpi0 + TILE_X + 1u];
                if (mcoll_pre != 0u) {
                    let gxb = tile_origin_x + lxb0 - i32(BORDER);
                    let gyb = tile_origin_y + lyb0 - i32(BORDER);
                    var r_raw = 0.0; var g_sum = 0.0; var g_n = 0u; var b_raw = 0.0;
                    var g_all = 0.0;
                    for (var by: i32 = 0; by < 2; by++) {
                        for (var bx: i32 = 0; bx < 2; bx++) {
                            let sx = clamp(gxb + bx, 0, i32(uniforms.width) - 1);
                            let sy = clamp(gyb + by, 0, i32(uniforms.height) - 1);
                            let raw = f32(textureLoad(cfa_tex, vec2<i32>(sx, sy), 0).r);
                            let ch = bayer_color(sx, sy);
                            if (ch == 0) { r_raw = raw; }
                            else if (ch == 2) { b_raw = raw; }
                            else {
                                g_all += raw;
                                if (raw < uniforms.pin_thr) { g_sum += raw; g_n += 1u; }
                            }
                        }
                    }
                    var g_raw = g_all * 0.5;
                    if (g_n > 0u) { g_raw = g_sum / f32(g_n); }
                    rn = max(0.0, (r_raw - uniforms.black_r) / norm_range_r);
                    gn = max(0.0, (g_raw - uniforms.black_g) / norm_range_g);
                    bn = max(0.0, (b_raw - uniforms.black_b) / norm_range_b);
                }

                // Highlight reconstruction (HL-handling.md §3): estimate
                // clipped channels from the 9×9 neighborhood in normalized
                // raw space, BEFORE white balance. Tier 3 (all channels
                // clipped) gets the ring-anchor hue. Clean pixels pass
                // through untouched.
                var m0: u32 = 0u;
                if (rn >= recon_thr) { m0 |= 1u; }
                if (gn >= recon_thr) { m0 |= 2u; }
                if (bn >= recon_thr) { m0 |= 4u; }
                if (recon_on && m0 != 0u) {
                    let rec = reconstruct_pixel(vec3(rn, gn, bn), m0, gx, gy, lx, ly);
                    rn = rec.x; gn = rec.y; bn = rec.z;
                }

// Collapse-gate mask (user-approved design): the RAW-TRUTH
                // 2×2-block registry written by the load loop — a photosite
                // at/above 0.99×clip_raw (raw CFA units) anywhere in the
                // pixel's 2×2 CFA block flags its channel. The demosaiced
                // plane (m0) is NOT used here: RCD/bilinear interpolation
                // can average pinned photosites beneath the mask threshold
                // (the GPU magenta residual); the raw CFA cannot lie.
                // Pixels with ≥ 2 pinned channels hold no hue info and
                // collapse to their fused-luma neutral after WB (§3.2,
                // deviation D9) — brightness preserved, no hue invented.
                // This is the guarantee that NO export state (recovery on
                // or off) renders magenta. Pixels whose block carries no
                // pin keep their recorded color — scene-referred, no
                // pre-trigger (user-mandated: sub-threshold photosites
                // like 0.983×WL pass through WB + CCM untouched).
                let lxb = lx - i32((u32(lx) + 1u) & 1u);
                let lyb = ly - i32((u32(ly) + 1u) & 1u);
                let bpi = u32(lyb) * TILE_X + u32(lxb);
                let mcoll = shm_pin[bpi] | shm_pin[bpi + 1u] | shm_pin[bpi + TILE_X] | shm_pin[bpi + TILE_X + 1u];

                // 1. Apply White Balance (gains are clamped on the CPU
                //    side at uniform-write time; clamp here too so a
                //    bad uniform cannot blow up the output).
                let wb_r = clamp(uniforms.wb_r, WB_GAIN_MIN, WB_GAIN_MAX);
                let wb_b = clamp(uniforms.wb_b, WB_GAIN_MIN, WB_GAIN_MAX);
                let rw = rn * wb_r;
                let gw = gn;
                let bw = bn * wb_b;

                // 1b. Clipped-pair neutral collapse (always active when the
                //     raw-truth mask threshold is set; independent of
                //     recon_on and of recon_threshold).
                var rwc = rw;
                var gwc = gw;
                var bwc = bw;
                if (uniforms.pin_thr > 0.0) {
                    let ones = (mcoll & 1u) + ((mcoll >> 1u) & 1u) + ((mcoll >> 2u) & 1u);
                    if (ones >= 2u) {
                        // Neutral direction is [k, k, k]: the fused CCM row
                        // sums are 1.0 (±0.001 by ±CAT construction), so the
                        // matrix's input-space neutral [1,1,1] maps to output
                        // neutral. WB gains are applied separately above; the
                        // old [k·wb_r, k, k·wb_b] re-applied them inside the
                        // CCM and exited with R≈B≈3.4·G — magenta.
                        let luma = uniforms.recon_luma.xyz;
                        let y = luma.x * rw + luma.y * gw + luma.z * bw;
                        let luma_neutral = luma.x + luma.y + luma.z;
                        var k: f32 = 0.0;
                        if (abs(luma_neutral) > 1e-6) { k = max(y / luma_neutral, 0.0); }
                        rwc = k; gwc = k; bwc = k;
                    } else if (mcoll == 2u) {
                        // G-pinned single clip (G is the WB anchor, gain 1.0):
                        // the WB'd R/B overshoot the pinned G and the CCM's
                        // negative secondaries flip the ratio magenta. Cap
                        // the unclipped channels at the WB'd pinned G —
                        // brightness from the measured anchor, no hue
                        // invented (D10).
                        rwc = min(rwc, gwc);
                        bwc = min(bwc, gwc);
                    }
                }

                // 2. Apply CCM
                var rout = rwc * ccm0 + gwc * ccm1 + bwc * ccm2;
                var gout = rwc * ccm3 + gwc * ccm4 + bwc * ccm5;
                var bout = rwc * ccm6 + gwc * ccm7 + bwc * ccm8;

                rout = max(rout, 0.0);
                gout = max(gout, 0.0);
                bout = max(bout, 0.0);

                // 3. Hue-preserving display rolloff (HL-handling.md §4).
                //    Display-referred only (Linear, Rec.709, Gamma24);
                //    identity below 1.0, C1-continuous rational shoulder
                //    above, uniform scale — R:G:B ratios invariant.
                if (gm == 0u || gm == 1u || gm == 12u) {
                    let m = max(rout, max(gout, bout));
                    if (m > 1.0) {
                        let s = (1.0 + (m - 1.0) / (1.0 + 20.0 * (m - 1.0))) / m;
                        rout *= s;
                        gout *= s;
                        bout *= s;
                    }
                }

                var ro: f32; var go: f32; var bo: f32;

                // ----------------------------------------------------------------
                // OETF (linear → log) — mirror of `TransferFunction::process`
                // in `src/color.rs`. The mapping `gm` index -> transfer
                // function is defined in `src/gpu.rs::transfer_to_gamma_mode`.
                //
                // Source-of-truth references (see `color.rs` for the full
                // table):
                //   gm==0  Linear
                //   gm==1  Rec.709       — ITU-R BT.709-6
                //   gm==2  S-Log3        — Sony "S-Log3 Technical Summary" (Sept 2014)
                //   gm==3  V-Log         — Panasonic V-Log/V-Gamut Reference Manual (2014)
                //   gm==4  ARRI LogC3    — ARRI LogC-3 spec (2020), EI 800
                //   gm==5  Canon C-Log3  — Canon C-Log3 characteristics (2016)
                //   gm==6  F-Log2        — Fujifilm F-Log2 Data Sheet (2021)
                //   gm==7  ACEScct       — AMPAS ACEScc specification (TB-2022-002)
                //   gm==8  PQ ST.2084    — ITU-R BT.2100-2
                //   gm==9  HLG           — ITU-R BT.2100-2
                //   gm==10 DaVinci Intermediate — Blackmagic white paper
                //   gm==11 Apple Log / Apple Log 2 — Apple "Apple Log Profile White Paper" (Sept 2023)
                //   gm==12 Display gamma 1/2.4 (Rec.1886 EOTF approximation)
                //   gm==13 ARRI LogC4 — ARRI "LogC4 Encoding Function" (Cooper & Brendel, 2022)
                // ----------------------------------------------------------------
                if (gm == 0u) { ro = rout; go = gout; bo = bout; }
                else if (gm == 1u) {
                    ro = select(4.5 * rout, 1.099 * pow(rout, 0.45) - 0.099, rout >= 0.018);
                    go = select(4.5 * gout, 1.099 * pow(gout, 0.45) - 0.099, gout >= 0.018);
                    bo = select(4.5 * bout, 1.099 * pow(bout, 0.45) - 0.099, bout >= 0.018);
                } else if (gm == 2u) {
                    // Sony S-Log3 (correct per "S-Log3 Technical Summary", Sept 2014)
                    let slog3_cut = 0.01125;
                    let slog3_linear_slope = (171.2102946929 - 95.0) / slog3_cut;
                    let slog3_a = 420.0;
                    let slog3_b = 261.5;
                    ro = select((rout * slog3_linear_slope + 95.0) / 1023.0,
                                (slog3_a + slog3_b * log10(max(1e-10, (rout + 0.01) / 0.19))) / 1023.0,
                                rout >= slog3_cut);
                    go = select((gout * slog3_linear_slope + 95.0) / 1023.0,
                                (slog3_a + slog3_b * log10(max(1e-10, (gout + 0.01) / 0.19))) / 1023.0,
                                gout >= slog3_cut);
                    bo = select((bout * slog3_linear_slope + 95.0) / 1023.0,
                                (slog3_a + slog3_b * log10(max(1e-10, (bout + 0.01) / 0.19))) / 1023.0,
                                bout >= slog3_cut);
                } else if (gm == 3u) {
                    ro = select(5.6 * rout + 0.125, 0.241514 * log10(rout + 0.00873) + 0.598206, rout >= 0.01);
                    go = select(5.6 * gout + 0.125, 0.241514 * log10(gout + 0.00873) + 0.598206, gout >= 0.01);
                    bo = select(5.6 * bout + 0.125, 0.241514 * log10(bout + 0.00873) + 0.598206, bout >= 0.01);
                } else if (gm == 4u) {
                    ro = select(5.367655 * rout + 0.092809, 0.247190 * log10(5.555556 * rout + 0.052272) + 0.385537, rout > 0.010591);
                    go = select(5.367655 * gout + 0.092809, 0.247190 * log10(5.555556 * gout + 0.052272) + 0.385537, gout > 0.010591);
                    bo = select(5.367655 * bout + 0.092809, 0.247190 * log10(5.555556 * bout + 0.052272) + 0.385537, bout > 0.010591);
                } else if (gm == 5u) {
                    // Canon C-Log3 (Canon "C-Log3 characteristics", 2016).
                    // Three segments: negative (log), linear mid, positive (log).
                    let neg = (0.097465473 - 0.12512219) / 1.9754798;
                    let pos = (0.15277891 - 0.12512219) / 1.9754798;
                    ro = select(select(-0.36726845 * log10(max(1e-10, -rout * 14.98325 + 1.0)) + 0.12783901, 1.9754798 * rout + 0.12512219, rout >= neg), 0.36726845 * log10(rout * 14.98325 + 1.0) + 0.12240537, rout > pos);
                    go = select(select(-0.36726845 * log10(max(1e-10, -gout * 14.98325 + 1.0)) + 0.12783901, 1.9754798 * gout + 0.12512219, gout >= neg), 0.36726845 * log10(gout * 14.98325 + 1.0) + 0.12240537, gout > pos);
                    bo = select(select(-0.36726845 * log10(max(1e-10, -bout * 14.98325 + 1.0)) + 0.12783901, 1.9754798 * bout + 0.12512219, bout >= neg), 0.36726845 * log10(bout * 14.98325 + 1.0) + 0.12240537, bout > pos);
                } else if (gm == 6u) {
                    ro = select(8.799461 * rout + 0.092864, 0.245281 * log10(5.555556 * rout + 0.064829) + 0.384316, rout >= 0.000889);
                    go = select(8.799461 * gout + 0.092864, 0.245281 * log10(5.555556 * gout + 0.064829) + 0.384316, gout >= 0.000889);
                    bo = select(8.799461 * bout + 0.092864, 0.245281 * log10(5.555556 * bout + 0.064829) + 0.384316, bout >= 0.000889);
                } else if (gm == 7u) {
                    ro = select(10.54023774 * rout + 0.07290553, (log2(rout) + 9.72) / 17.52, rout > 0.0078125);
                    go = select(10.54023774 * gout + 0.07290553, (log2(gout) + 9.72) / 17.52, gout > 0.0078125);
                    bo = select(10.54023774 * bout + 0.07290553, (log2(bout) + 9.72) / 17.52, bout > 0.0078125);
                } else if (gm == 8u) {
                    let m1 = 0.1593017578125; let m2 = 78.84375;
                    let c1 = 0.8359375; let c2 = 18.8515625; let c3 = 18.6875;
                    let xm1_r = pow(rout, m1); let xm1_g = pow(gout, m1); let xm1_b = pow(bout, m1);
                    ro = pow((c1 + c2 * xm1_r) / (1.0 + c3 * xm1_r), m2);
                    go = pow((c1 + c2 * xm1_g) / (1.0 + c3 * xm1_g), m2);
                    bo = pow((c1 + c2 * xm1_b) / (1.0 + c3 * xm1_b), m2);
                } else if (gm == 9u) {
                    ro = select(0.17883277 * log(max(1e-12, 12.0 * rout - 0.28466892)) + 0.55991073, sqrt(3.0 * rout), rout < (1.0 / 12.0));
                    go = select(0.17883277 * log(max(1e-12, 12.0 * gout - 0.28466892)) + 0.55991073, sqrt(3.0 * gout), gout < (1.0 / 12.0));
                    bo = select(0.17883277 * log(max(1e-12, 12.0 * bout - 0.28466892)) + 0.55991073, sqrt(3.0 * bout), bout < (1.0 / 12.0));
                } else if (gm == 10u) {
                    // DaVinci Intermediate (BMD white paper).
                    // select(f, t, cond) = cond ? t : f
                    // Linear below knee, log above.
                    ro = select(0.07329248 * (log2(rout + 0.0075) + 7.0), 10.44426855 * rout, rout <= 0.00262409);
                    go = select(0.07329248 * (log2(gout + 0.0075) + 7.0), 10.44426855 * gout, gout <= 0.00262409);
                    bo = select(0.07329248 * (log2(bout + 0.0075) + 7.0), 10.44426855 * bout, bout <= 0.00262409);
                } else if (gm == 11u) {
                    let R0 = -0.05641088; let RT = 0.01; let C = 47.28711236;
                    let BETA = 0.00964052; let GAMMA = 0.08550479; let DELTA = 0.69336945;
                    ro = select(0.0, select(C * (rout - R0) * (rout - R0), GAMMA * log2(rout + BETA) + DELTA, rout >= RT), rout < R0);
                    go = select(0.0, select(C * (gout - R0) * (gout - R0), GAMMA * log2(gout + BETA) + DELTA, gout >= RT), gout < R0);
                    bo = select(0.0, select(C * (bout - R0) * (bout - R0), GAMMA * log2(bout + BETA) + DELTA, bout >= RT), bout < R0);
                } else if (gm == 12u) {
                    // Display gamma 1/2.4 (Rec.1886 EOTF approximation).
                    ro = pow(max(rout, 0.0), 1.0 / 2.4);
                    go = pow(max(gout, 0.0), 1.0 / 2.4);
                    bo = pow(max(bout, 0.0), 1.0 / 2.4);
                } else if (gm == 13u) {
                    // ARRI LogC4 (Cooper & Brendel, 2022). EI-independent.
                    // a = (2^18 - 16) / 117.45
                    // b = (1023 - 95) / 1023
                    // c = 95 / 1023
                    // s = (7 * ln 2 * 2^(7 - 14*c/b)) / (a * b)
                    // t = (2^(-14*c/b + 6) - 64) / a
                    let l4_a = 2231.8263091;
                    let l4_b = 0.9071358749;
                    let l4_c = 0.0928641251;
                    let l4_s = 0.1135972086;
                    let l4_t = -0.0180569961;
                    ro = select((rout - l4_t) / l4_s, ((log2(l4_a * rout + 64.0) - 6.0) / 14.0) * l4_b + l4_c, rout >= l4_t);
                    go = select((gout - l4_t) / l4_s, ((log2(l4_a * gout + 64.0) - 6.0) / 14.0) * l4_b + l4_c, gout >= l4_t);
                    bo = select((bout - l4_t) / l4_s, ((log2(l4_a * bout + 64.0) - 6.0) / 14.0) * l4_b + l4_c, bout >= l4_t);
                } else { ro = rout; go = gout; bo = bout; }

                let ri = u32(clamp(ro * 65535.0, 0.0, 65535.0));
                let gi = u32(clamp(go * 65535.0, 0.0, 65535.0));
                let bi = u32(clamp(bo * 65535.0, 0.0, 65535.0));
                let out_idx = (u32(gy) * uniforms.width + u32(gx)) * 2u;
                out_buf[out_idx] = ri | (gi << 16u);
                out_buf[out_idx + 1u] = bi;
            }
        }
    }
}
