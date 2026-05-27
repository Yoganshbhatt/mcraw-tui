struct Uniforms {
    width: u32, height: u32, filters: u32, gamma_mode: u32,
    black_level: f32, white_level: f32, wb_r: f32, wb_b: f32,
    ccm_row0: vec4<f32>, ccm_row1: vec4<f32>, ccm_row2: vec4<f32>,
    phase_x: i32, phase_y: i32,
};

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
    let cx = clamp(gx / 2, 0, i32(uniforms.width) / 2 - 1);
    let cy = clamp(gy, 0, i32(uniforms.height) - 1);
    return textureLoad(pq_tex, vec2<i32>(cx, cy), 0).r;
}

@compute @workgroup_size(16, 16)
fn main(
    @builtin(workgroup_id) wg_id: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let tile_origin_x = i32(wg_id.x * VALID_X);
    let tile_origin_y = i32(wg_id.y * VALID_Y);
    let thread_id = lid.y * 16u + lid.x;

    // Step 0: Fill shared memory with STRICTLY RAW (Black-subtracted) values
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
            val = max(0.0, raw - uniforms.black_level);
        }

        let c = bayer_color(gx, gy);
        if (c == 0) { shm_r[idx] = val; }
        else if (c == 1) { shm_g[idx] = val; }
        else if (c == 2) { shm_b[idx] = val; }
    }
    workgroupBarrier();

    // Step 1: Green at RB positions (Hamilton-Adams Algorithm)
    // CRITICAL FIX: Expanded bounds (2 to 126) to interpolate Green in the border region.
    // This prevents Step 3 from reading 0.0 (uninitialized) Green values at tile edges.
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

    // Step 2: RB at BR locations (Diagonals)
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

    // Step 3: RB at Green locations (Strict Axis Locking)
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

    // Step 4: Professional DNG-Style Color Pipeline
    // Pipeline: Normalize → WB → CCM → Gamma
    // NOTE: Highlight compression is handled by the CCM matrix naturally
    // since it maps camera RGB → output RGB preserving color ratios
    let norm_range = max(uniforms.white_level - uniforms.black_level, 1.0);
    let wb_r = uniforms.wb_r;  // WB gain for red
    let wb_b = uniforms.wb_b;  // WB gain for blue
    let ccm0 = uniforms.ccm_row0.x; let ccm1 = uniforms.ccm_row0.y; let ccm2 = uniforms.ccm_row0.z;
    let ccm3 = uniforms.ccm_row1.x; let ccm4 = uniforms.ccm_row1.y; let ccm5 = uniforms.ccm_row1.z;
    let ccm6 = uniforms.ccm_row2.x; let ccm7 = uniforms.ccm_row2.y; let ccm8 = uniforms.ccm_row2.z;

    for (var i: u32 = 0u; i < 16u; i++) {
        let idx = thread_id * 16u + i;
        let ly = i32(idx / TILE_X);
        let lx = i32(idx % TILE_X);
        if (lx >= i32(BORDER) && lx < i32(TILE_X) - i32(BORDER) && ly >= i32(BORDER) && ly < i32(TILE_Y) - i32(BORDER)) {
            let gx = tile_origin_x + lx - i32(BORDER);
            let gy = tile_origin_y + ly - i32(BORDER);
            if (gx >= 0 && gx < i32(uniforms.width) && gy >= 0 && gy < i32(uniforms.height)) {

                // 1. Normalize strictly RAW data to [0, 1]
                let rn = shm_r[idx] / norm_range;
                let gn = shm_g[idx] / norm_range;
                let bn = shm_b[idx] / norm_range;

                // 2. Apply White Balance gains (make neutral gray neutral)
                let rw = rn * wb_r;
                let gw = gn;  // Green is reference (gain = 1.0)
                let bw = bn * wb_b;

                // 3. Apply the combined matrix (ForwardMatrix + OutputMatrix)
                var rout = rw * ccm0 + gw * ccm1 + bw * ccm2;
                var gout = rw * ccm3 + gw * ccm4 + bw * ccm5;
                var bout = rw * ccm6 + gw * ccm7 + bw * ccm8;

                // 4. Preserve highlights - prevent magenta/pink highlights from CCM clipping
                // When any channel clips to 0, desaturate toward luminance to preserve hue
                let highlight_thresh = 0.95;
                let lum = rout * 0.2126 + gout * 0.7152 + bout * 0.0722;
                let max_chan = max(rout, max(gout, bout));
                if max_chan > highlight_thresh {
                    let preserve = max(0.0, 1.0 - (max_chan - highlight_thresh) * 5.0);
                    let gray = mix(lum, max_chan, preserve);
                    let scale = gray / max_chan;
                    rout = rout * scale;
                    gout = gout * scale;
                    bout = bout * scale;
                }

                // 5. Clip negative values
                rout = max(rout, 0.0);
                gout = max(gout, 0.0);
                bout = max(bout, 0.0);

                // 6. Rec709 OETF (Gamma) - apply after CCM
                let ro = select(4.5 * rout, 1.099 * pow(rout, 0.45) - 0.099, rout >= 0.018);
                let go = select(4.5 * gout, 1.099 * pow(gout, 0.45) - 0.099, gout >= 0.018);
                let bo = select(4.5 * bout, 1.099 * pow(bout, 0.45) - 0.099, bout >= 0.018);

                // 7. Pack to u32 (RGBu8)
                let ri = u32(clamp(ro * 255.0, 0.0, 255.0));
                let gi = u32(clamp(go * 255.0, 0.0, 255.0));
                let bi = u32(clamp(bo * 255.0, 0.0, 255.0));
                let out_idx = u32(gy) * uniforms.width + u32(gx);
                out_buf[out_idx] = ri | (gi << 8u) | (bi << 16u);
            }
        }
    }
}