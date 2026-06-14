struct Uniforms {
    width: u32, height: u32, filters: u32, gamma_mode: u32,
    black_level: f32, white_level: f32, wb_r: f32, wb_b: f32,
    black_r: f32, black_g: f32, black_b: f32, _black_pad: f32,
    ccm_row0: vec4<f32>, ccm_row1: vec4<f32>, ccm_row2: vec4<f32>,
    phase_x: i32, phase_y: i32,
};

const WB_GAIN_MIN: f32 = 0.1;
const WB_GAIN_MAX: f32 = 10.0;

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

@compute @workgroup_size(16, 16)
fn main(
    @builtin(workgroup_id) wg_id: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let tile_origin_x = i32(wg_id.x * VALID_X);
    let tile_origin_y = i32(wg_id.y * VALID_Y);
    let thread_id = lid.y * 16u + lid.x;

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

    let norm_range = max(uniforms.white_level - uniforms.black_level, 1.0);
    let ccm0 = uniforms.ccm_row0.x; let ccm1 = uniforms.ccm_row0.y; let ccm2 = uniforms.ccm_row0.z;
    let ccm3 = uniforms.ccm_row1.x; let ccm4 = uniforms.ccm_row1.y; let ccm5 = uniforms.ccm_row1.z;
    let ccm6 = uniforms.ccm_row2.x; let ccm7 = uniforms.ccm_row2.y; let ccm8 = uniforms.ccm_row2.z;

    let gm = uniforms.gamma_mode;

    for (var i: u32 = 0u; i < 16u; i++) {
        let idx = thread_id * 16u + i;
        let ly = i32(idx / TILE_X);
        let lx = i32(idx % TILE_X);
        if (lx >= i32(BORDER) && lx < i32(TILE_X) - i32(BORDER) && ly >= i32(BORDER) && ly < i32(TILE_Y) - i32(BORDER)) {
            let gx = tile_origin_x + lx - i32(BORDER);
            let gy = tile_origin_y + ly - i32(BORDER);
            if (gx >= 0 && gx < i32(uniforms.width) && gy >= 0 && gy < i32(uniforms.height)) {

                let rn = shm_r[idx] / norm_range;
                let gn = shm_g[idx] / norm_range;
                let bn = shm_b[idx] / norm_range;

                // 1. Apply White Balance (gains are clamped on the CPU
                //    side at uniform-write time; clamp here too so a
                //    bad uniform cannot blow up the output).
                let wb_r = clamp(uniforms.wb_r, WB_GAIN_MIN, WB_GAIN_MAX);
                let wb_b = clamp(uniforms.wb_b, WB_GAIN_MIN, WB_GAIN_MAX);
                let rw = rn * wb_r;
                let gw = gn;
                let bw = bn * wb_b;

                // 2. Highlight Reconstruction — desaturate toward neutral
                //    when a single channel clips. Per-pixel trigger on
                //    `max_raw > 0.95` (Section 1.6 of optimisation.md).
                let max_raw = max(rn, max(gn, bn));
                let max_wb  = max(rw, max(gw, bw));
                let t_highlight = clamp((max_raw - 0.95) / 0.05, 0.0, 1.0);
                let neutral = min(1.0, max_wb);

                var final_rw = rw;
                var final_gw = gw;
                var final_bw = bw;
                if (t_highlight > 0.0) {
                    final_rw = mix(rw, neutral, t_highlight);
                    final_gw = mix(gw, neutral, t_highlight);
                    final_bw = mix(bw, neutral, t_highlight);
                }

                // 3. Apply CCM (on WB'd values — standard pipeline)
                var rout = final_rw * ccm0 + final_gw * ccm1 + final_bw * ccm2;
                var gout = final_rw * ccm3 + final_gw * ccm4 + final_bw * ccm5;
                var bout = final_rw * ccm6 + final_gw * ccm7 + final_bw * ccm8;

                rout = max(rout, 0.0);
                gout = max(gout, 0.0);
                bout = max(bout, 0.0);

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
                    ro = select((rout * 261.5 + 10.23) / 1023.0, 0.432699 * log10(10.0 * rout + 1.0) + 0.037584, rout >= 0.01);
                    go = select((gout * 261.5 + 10.23) / 1023.0, 0.432699 * log10(10.0 * gout + 1.0) + 0.037584, gout >= 0.01);
                    bo = select((bout * 261.5 + 10.23) / 1023.0, 0.432699 * log10(10.0 * bout + 1.0) + 0.037584, bout >= 0.01);
                } else if (gm == 3u) {
                    ro = select(5.6 * rout + 0.125, 0.241514 * log10(rout + 0.00873) + 0.598206, rout >= 0.01);
                    go = select(5.6 * gout + 0.125, 0.241514 * log10(gout + 0.00873) + 0.598206, gout >= 0.01);
                    bo = select(5.6 * bout + 0.125, 0.241514 * log10(bout + 0.00873) + 0.598206, bout >= 0.01);
                } else if (gm == 4u) {
                    ro = select(5.367655 * rout + 0.092809, 0.247190 * log10(5.555556 * rout + 0.052272) + 0.385537, rout > 0.010591);
                    go = select(5.367655 * gout + 0.092809, 0.247190 * log10(5.555556 * gout + 0.052272) + 0.385537, gout > 0.010591);
                    bo = select(5.367655 * bout + 0.092809, 0.247190 * log10(5.555556 * bout + 0.052272) + 0.385537, bout > 0.010591);
                } else if (gm == 5u) {
                    let neg = (0.097465473 - 0.12512219) / 1.9754798;
                    let pos = (0.15277891 - 0.12512219) / 1.9754798;
                    ro = select(-0.36726845 * log10(max(1e-10, -rout * 14.98325 + 1.0)) + 0.12783901, select(1.9754798 * rout + 0.12512219, 0.36726845 * log10(rout * 14.98325 + 1.0) + 0.12240537, rout <= pos), rout < neg);
                    go = select(-0.36726845 * log10(max(1e-10, -gout * 14.98325 + 1.0)) + 0.12783901, select(1.9754798 * gout + 0.12512219, 0.36726845 * log10(gout * 14.98325 + 1.0) + 0.12240537, gout <= pos), gout < neg);
                    bo = select(-0.36726845 * log10(max(1e-10, -bout * 14.98325 + 1.0)) + 0.12783901, select(1.9754798 * bout + 0.12512219, 0.36726845 * log10(bout * 14.98325 + 1.0) + 0.12240537, bout <= pos), bout < neg);
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
                    ro = select(10.44426855 * rout, 0.07329248 * (log2(rout + 0.0075) + 7.0), rout <= 0.00262409);
                    go = select(10.44426855 * gout, 0.07329248 * (log2(gout + 0.0075) + 7.0), gout <= 0.00262409);
                    bo = select(10.44426855 * bout, 0.07329248 * (log2(bout + 0.0075) + 7.0), bout <= 0.00262409);
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