// hl_complete.wgsl — RAW-domain censored-photosite completion (Full policy)
// Workgroup: 16x16 (256 invocations, one thread per photosite PAIR)
// Inputs: cfa_tex (r16uint texture, read), HlUniforms (uniform)
// Outputs: out_packed (storage u32, read_write; two u16 photosites per word)
//
// Reference implementation: `src/hl.rs::complete_photosite`. The GPU port
// must agree with it to <= 1 LSB per photosite (division is 2.5 ULP on the
// GPU vs correctly rounded on x86, so exact equality is not promised).
//
// Design notes:
//   * One thread owns one horizontally adjacent PAIR and writes one packed
//     word with a single store. There is deliberately NO read-modify-write:
//     two threads sharing a word would be a lost-update race.
//   * Zero `var<workgroup>`, zero `workgroupBarrier`. No cross-invocation
//     communication exists, so barriers would be pure hang surface.
//   * `bayer_color` mirrors `hl::color_at` exactly (active-region coords, no
//     phase adjust). Quad patterns are folded to their base host-side, so
//     only the four base arms exist here. Parity with the CPU pass is the
//     contract; see the odd-offset tripwire in `pipeline.rs`.
//   * Float->u32 conversion truncates toward zero, matching Rust's `as u32`
//     saturating cast on the pre-clamped range. This is deliberate (hl.rs
//     documents the choice); do not "fix" it to round on one side only.

struct HlUniforms {
    dims    : vec4<u32>,  // width, height, base_pattern (0=R,1=Gr,2=Gb,3=B), pitch_words
    range   : vec4<f32>,  // range[R], range[G], range[B], noise_floor
    black   : vec4<f32>,  // black[R], black[G], black[B], rail_dn
    neutral : vec4<f32>,  // neutral[R], neutral[G], neutral[B], fallback_scale
    aux     : vec4<u32>,  // enabled (0 = Sensor identity copy), 0, 0, 0
};

@group(0) @binding(0) var cfa_tex : texture_2d<u32>;
@group(0) @binding(1) var<storage, read_write> out_packed : array<u32>;
@group(0) @binding(2) var<uniform> u : HlUniforms;

fn bayer_color(x: i32, y: i32) -> i32 {
    let ex = (x & 1) == 0;
    let ey = (y & 1) == 0;
    let p = u.dims.z;
    if (p == 0u) {          // RGGB
        if (ex && ey) { return 0; }
        if (!ex && !ey) { return 2; }
        return 1;
    }
    if (p == 3u) {          // BGGR
        if (!ex && !ey) { return 0; }
        if (ex && ey) { return 2; }
        return 1;
    }
    if (p == 1u) {          // GRBG
        if (!ex && ey) { return 0; }
        if (ex && !ey) { return 2; }
        return 1;
    }                        // GBRG
    if (ex && !ey) { return 0; }
    if (!ex && ey) { return 2; }
    return 1;
}

fn load_raw(x: i32, y: i32) -> u32 {
    let cx = clamp(x, 0, i32(u.dims.x) - 1);
    let cy = clamp(y, 0, i32(u.dims.y) - 1);
    return textureLoad(cfa_tex, vec2<i32>(cx, cy), 0).r;
}

/// Insertion sort of the first `n` entries, ascending; returns entry `n/2`.
/// All inputs are finite (quotients of finite non-negative values over
/// host-floored positive ranges), so any correct sort yields the same value
/// at each index, including ties. Matches `hl::median_of_n` exactly.
fn median_of_n(s: ptr<function, array<f32, 8>>, n: u32) -> f32 {
    var i = 1u;
    while (i < n) {
        let key = (*s)[i];
        var j = i;
        while (j > 0u && key < (*s)[j - 1u]) { (*s)[j] = (*s)[j - 1u]; j -= 1u; }
        (*s)[j] = key;
        i += 1u;
    }
    return (*s)[n / 2u];
}

/// Completion for one photosite, or the input unchanged. Mirrors
/// `hl::complete_photosite` (Full policy) step for step.
fn complete_one(x: i32, y: i32) -> u32 {
    let w = i32(u.dims.x);
    let h = i32(u.dims.y);
    let raw = load_raw(x, y);
    if (f32(raw) < u.black.w) { return raw; }
    let c = bayer_color(x, y);
    var scales : array<f32, 8>;
    var n = 0u;
    for (var dy = -1; dy <= 1; dy += 1) {
        for (var dx = -1; dx <= 1; dx += 1) {
            if (dx == 0 && dy == 0) { continue; }
            let xx = x + dx;
            let yy = y + dy;
            if (xx < 0 || yy < 0 || xx >= w || yy >= h) { continue; }
            let vp = load_raw(xx, yy);
            if (f32(vp) >= u.black.w) { continue; }
            let cp = bayer_color(xx, yy);
            let nv = (f32(vp) - u.black[cp]) / u.range[cp];
            if (!(nv > u.range.w)) { continue; }
            scales[n] = nv / u.neutral[cp];
            n += 1u;
        }
    }
    var s = u.neutral.w;
    if (n > 0u) { s = median_of_n(&scales, n); }
    let est = max(1.0, u.neutral[c] * s);
    // No finiteness guard: naga 24 has no isNan/isInf builtins, and none is
    // needed. `HlParams::new` sanitises every input (finite blacks, range >=
    // 1, neutral in (0.05, 20)), so `est` is provably finite: median of
    // finite quotients, scaled by <= 20, offset by <= 65535. The CPU keeps
    // its `is_finite` check as cheap defensiveness on an unreachable path.
    let dn = u32(clamp(est * u.range[c] + u.black[c], 0.0, 65535.0));
    if (dn >= u32(u.black.w)) { return dn; }
    // Matches `raw.max(rail)`: raw is censored, so raw >= rail already.
    return raw;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x0 = i32(gid.x) * 2;
    let y = i32(gid.y);
    let w = i32(u.dims.x);
    let h = i32(u.dims.y);
    if (x0 >= w || y >= h) { return; }
    var lo: u32;
    var hi: u32;
    if (u.aux.x == 0u) {
        lo = load_raw(x0, y);
        hi = load_raw(x0 + 1, y);
    } else {
        lo = complete_one(x0, y);
        hi = select(0u, complete_one(x0 + 1, y), x0 + 1 < w);
    }
    out_packed[u32(y) * u.dims.w + gid.x] = lo | (hi << 16u);
}
