// RCD conv pass — compute the 5-tap binomial LPF pyramid, the V/H
// discriminator, and the P/Q discriminator. All three are written to
// storage textures for the fill pass to consume.
//
// Source-of-truth references:
//   * RCD algorithm: dcraw/RawTherapee-style "Refined Chroma Demosaic"
//     based on the LuisSR 2012 paper "Demosaicing with the LMMSE
//     estimator" and the related work on the 5-tap binomial LPF
//     pyramid.
//   * The 5-tap binomial [1, 4, 6, 4, 1] / 16 is the standard
//     noise-robust luminance estimate. The same weights appear in
//     Gaussian-pyramid construction in Burt & Adelson 1983.

struct Uniforms {
    width: u32, height: u32, filters: u32, gamma_mode: u32,
    black_level: f32, white_level: f32, wb_r: f32, wb_b: f32,
    black_r: f32, black_g: f32, black_b: f32, _black_pad: f32,
    ccm_row0: vec4<f32>, ccm_row1: vec4<f32>, ccm_row2: vec4<f32>,
    phase_x: i32, phase_y: i32,
};

@group(0) @binding(0) var cfa_tex: texture_2d<u32>;
@group(0) @binding(1) var cfa_sampler: sampler;
@group(0) @binding(2) var<uniform> uniforms: Uniforms;

@group(0) @binding(3) var vh_out: texture_storage_2d<r32float, write>;
@group(0) @binding(4) var pq_out: texture_storage_2d<r32float, write>;
@group(0) @binding(5) var lp_out: texture_storage_2d<r32float, write>;

fn bayer_color(x: i32, y: i32) -> i32 {
    let sx = x + uniforms.phase_x;
    let sy = y + uniforms.phase_y;
    let shift = u32(((((sy << 1) & 14) + (sx & 1)) << 1));
    let c = (uniforms.filters >> shift) & 3u;
    if (c == 3u) { return 1; }
    return i32(c);
}

fn black_for_channel(c: i32) -> f32 {
    if (c == 0) { return uniforms.black_r; }
    if (c == 1) { return uniforms.black_g; }
    return uniforms.black_b;
}

fn read_cfa(x: i32, y: i32) -> f32 {
    let dims = vec2<i32>(i32(uniforms.width), i32(uniforms.height));
    let cx = clamp(x, 0, dims.x - 1);
    let cy = clamp(y, 0, dims.y - 1);
    let c = bayer_color(x, y);
    let bl = black_for_channel(c);
    return max(0.0, f32(textureLoad(cfa_tex, vec2<i32>(cx, cy), 0).r) - bl);
}

// 5-tap binomial low-pass pyramid (1/16 * [1, 4, 6, 4, 1] applied
// separable: horizontal then vertical) at the half-res position
// (hx, hy). The 5x5 source window is centred on the 2x2 CFA block at
// (2*hx, 2*hy) in full-res coordinates, so it covers full-res pixels
// (2*hx-2, 2*hy-2) .. (2*hx+2, 2*hy+2). read_cfa clamps to the image
// edges so this is safe at the borders.
//
// Cost: 25 texture reads + 24 multiply-adds per call. The conv main()
// calls this 5 times per pixel (1 for the LPF write, 4 for the V/H
// and P/Q neighbour reads), so 125 reads per pixel. This is roughly
// the same cost as the original full-res V/H gradient (24 reads per
// pixel × 2 axes) multiplied by ~3. Trivial on a modern GPU.
fn lp_level1(hx: i32, hy: i32) -> f32 {
    let cx = hx * 2;
    let cy = hy * 2;
    let h0 = (read_cfa(cx - 2, cy - 2) + 4.0 * read_cfa(cx - 1, cy - 2) + 6.0 * read_cfa(cx, cy - 2) + 4.0 * read_cfa(cx + 1, cy - 2) + read_cfa(cx + 2, cy - 2)) / 16.0;
    let h1 = (read_cfa(cx - 2, cy - 1) + 4.0 * read_cfa(cx - 1, cy - 1) + 6.0 * read_cfa(cx, cy - 1) + 4.0 * read_cfa(cx + 1, cy - 1) + read_cfa(cx + 2, cy - 1)) / 16.0;
    let h2 = (read_cfa(cx - 2, cy    ) + 4.0 * read_cfa(cx - 1, cy    ) + 6.0 * read_cfa(cx, cy    ) + 4.0 * read_cfa(cx + 1, cy    ) + read_cfa(cx + 2, cy    )) / 16.0;
    let h3 = (read_cfa(cx - 2, cy + 1) + 4.0 * read_cfa(cx - 1, cy + 1) + 6.0 * read_cfa(cx, cy + 1) + 4.0 * read_cfa(cx + 1, cy + 1) + read_cfa(cx + 2, cy + 1)) / 16.0;
    let h4 = (read_cfa(cx - 2, cy + 2) + 4.0 * read_cfa(cx - 1, cy + 2) + 6.0 * read_cfa(cx, cy + 2) + 4.0 * read_cfa(cx + 1, cy + 2) + read_cfa(cx + 2, cy + 2)) / 16.0;
    return (h0 + 4.0 * h1 + 6.0 * h2 + 4.0 * h3 + h4) / 16.0;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let dims = vec2<i32>(i32(uniforms.width), i32(uniforms.height));
    let gid = vec2<i32>(i32(global_id.x), i32(global_id.y));
    if (gid.x >= dims.x || gid.y >= dims.y) { return; }

    let c = bayer_color(gid.x, gid.y);

    // Half-res coordinates for the 2x2 CFA block containing gid.
    let hx = gid.x / 2;
    let hy = gid.y / 2;

    // Compute the LPF pyramid value for this block. All 4 sites in a
    // 2x2 block map to the same (hx, hy) and compute the same value,
    // so we can do the work once and reuse it for the LPF write, the
    // V/H gradient, and the P/Q gradient.
    let lp_c = lp_level1(hx, hy);

    // LPF pyramid output — write only at G sites (the other 2 sites
    // in the 2x2 block would write the same value, so this is just
    // a 2x redundancy reduction; correctness is unaffected).
    if (c == 1) {
        textureStore(lp_out, vec2<i32>(hx, hy), vec4<f32>(lp_c, 0.0, 0.0, 0.0));
    }

    // V/H gradient on the LPF pyramid (LuisSR step 1). Sampled at
    // half-res so sensor noise and CFA aliasing are averaged out.
    // Inline recomputation (no textureLoad on a write-only storage
    // texture, and no cross-workgroup read-after-write race).
    let lp_up = lp_level1(hx,     hy - 1);
    let lp_dn = lp_level1(hx,     hy + 1);
    let lp_lt = lp_level1(hx - 1, hy    );
    let lp_rt = lp_level1(hx + 1, hy    );
    let v_grad_lpf = abs(lp_c - lp_up) + abs(lp_c - lp_dn);
    let h_grad_lpf = abs(lp_c - lp_lt) + abs(lp_c - lp_rt);
    let vh = v_grad_lpf / (1e-5 + v_grad_lpf + h_grad_lpf);
    textureStore(vh_out, gid, vec4<f32>(vh, 0.0, 0.0, 0.0));

    // P/Q gradient on the LPF pyramid (LuisSR step 4). Diagonal
    // neighbours at half-res. Only needed at R/B sites (the fill
    // shader reads P/Q at half-res regardless of the source site).
    if (c != 1) {
        let lp_nw = lp_level1(hx - 1, hy - 1);
        let lp_se = lp_level1(hx + 1, hy + 1);
        let lp_ne = lp_level1(hx + 1, hy - 1);
        let lp_sw = lp_level1(hx - 1, hy + 1);
        let p_grad_lpf = abs(lp_c - lp_nw) + abs(lp_c - lp_se);
        let q_grad_lpf = abs(lp_c - lp_ne) + abs(lp_c - lp_sw);
        let pq = p_grad_lpf / (1e-5 + p_grad_lpf + q_grad_lpf);
        textureStore(pq_out, vec2<i32>(hx, hy), vec4<f32>(pq, 0.0, 0.0, 0.0));
    }
}
