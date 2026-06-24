// preview.wgsl — unified GPU preview compute shader
// Bilinear demosaic → normalize → exposure → WB → CCM → grade → OETF → display
// Output: packed RGBA8 via pack4x8unorm, ready for sixel encoding
//
// Last modified: 2026-06-17 — initial implementation
// Workgroup: 16x16
// Inputs: bayer_packed (storage, read), PreviewParams (uniform)
// Outputs: output_rgba (storage, read_write), histogram bins (storage, read_write, Phase 2)
//
// Transfer function discriminants must match src/color.rs TransferFunction::all() order:
//   0=Linear, 1=Rec709, 2=SLog3, 3=VLog, 4=ARRIlog3, 5=ARRIlog4, 6=CLog3,
//   7=FLog2, 8=AppleLog, 9=AppleLog2, 10=ACESCCT, 11=PQ, 12=HLG,
//   13=DaVinciIntermediate, 14=Gamma24
//
// Color space discriminants must match src/color.rs ColorSpace::all() order:
//   0=ACESAP1, 1=AppleWideGamut, 2=ARRIWideGamut3, 3=ARRIWideGamut4, 4=CanonCinemaGamut,
//   5=DaVinciWideGamut, 6=DciP3, 7=DisplayP3, 8=FGamut, 9=FGamutC,
//   10=PanasonicVGamut, 11=Rec2020, 12=Rec709, 13=SGamut3, 14=SGamut3Cine, 15=Srgb

struct PreviewParams {
    width: u32,
    height: u32,
    bayer_width: u32,
    bayer_height: u32,
    black_level: f32,
    white_level: f32,
    exposure: f32,
    wb_r: f32,
    wb_g: f32,
    wb_b: f32,
    contrast: f32,
    saturation: f32,
    shadows: f32,
    highlights: f32,
    ccm_row0: vec4<f32>,
    ccm_row1: vec4<f32>,
    ccm_row2: vec4<f32>,
    color_space: u32,
    transfer: u32,
    adjust_enabled: u32,
    bayer_phase: u32,
    compute_histogram: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    _pad3: u32,
    _pad4: u32,
    _pad5: u32,
    _pad6: u32,
};

@group(0) @binding(0) var<storage, read>        bayer_packed: array<u32>;
@group(0) @binding(1) var<storage, read_write>   output_rgba: array<u32>;
@group(0) @binding(2) var<uniform>               params: PreviewParams;

@group(0) @binding(3) var<storage, read_write>    hist_luma: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write>    hist_r: array<atomic<u32>>;
@group(0) @binding(5) var<storage, read_write>    hist_g: array<atomic<u32>>;
@group(0) @binding(6) var<storage, read_write>    hist_b: array<atomic<u32>>;

// ---------- Bayer read helpers ----------

fn load_bayer(x: i32, y: i32) -> f32 {
    let cx = clamp(x, 0, i32(params.bayer_width) - 1);
    let cy = clamp(y, 0, i32(params.bayer_height) - 1);
    let idx = u32(cy) * params.bayer_width + u32(cx);
    let lane = idx >> 1u;
    let word = bayer_packed[lane];
    if ((idx & 1u) == 0u) {
        return f32(word & 0xFFFFu);
    }
    return f32(word >> 16u);
}

fn log10(x: f32) -> f32 {
    return log(x) / 2.302585093;
}

// ---------- Bayer pattern colour lookup ----------
// phase: 0=RGGB, 1=GRBG, 2=GBRG, 3=BGGR
// Returns: 0=R, 1=G, 2=B

fn bayer_color(x: i32, y: i32) -> i32 {
    let even_row = (y & 1) == 0;
    let even_col = (x & 1) == 0;
    let p = params.bayer_phase;
    // RGGB: row0=R,G row1=G,B
    // GRBG: row0=G,R row1=B,G
    // GBRG: row0=G,B row1=R,G
    // BGGR: row0=B,G row1=G,R
    if (p == 0u) { // RGGB
        if (even_row) { if (even_col) { return 0; } else { return 1; } }
        else { if (even_col) { return 1; } else { return 2; } }
    }
    if (p == 1u) { // GRBG
        if (even_row) { if (even_col) { return 1; } else { return 0; } }
        else { if (even_col) { return 2; } else { return 1; } }
    }
    if (p == 2u) { // GBRG
        if (even_row) { if (even_col) { return 1; } else { return 2; } }
        else { if (even_col) { return 0; } else { return 1; } }
    }
    // BGGR
    if (even_row) { if (even_col) { return 2; } else { return 1; } }
    else { if (even_col) { return 1; } else { return 0; } }
}

// ---------- Bilinear demosaic ----------
// 4-sample interpolation at pixel (x, y)
// For a green site: average the two same-colour green neighbours
// For a R/B site: bilinear interpolation of the missing channels

fn demosaic_bilinear(x: i32, y: i32) -> vec3<f32> {
    let c = bayer_color(x, y);
    let center = load_bayer(x, y);
    let n  = load_bayer(x, y - 1);
    let s  = load_bayer(x, y + 1);
    let w  = load_bayer(x - 1, y);
    let e  = load_bayer(x + 1, y);
    let nw = load_bayer(x - 1, y - 1);
    let ne = load_bayer(x + 1, y - 1);
    let sw = load_bayer(x - 1, y + 1);
    let se = load_bayer(x + 1, y + 1);

    var r = 0.0;
    var g = 0.0;
    var b = 0.0;

    if (c == 0) { // R site
        r = center;
        g = (n + s + w + e) * 0.25;
        b = (nw + ne + sw + se) * 0.25;
    } else if (c == 2) { // B site
        b = center;
        g = (n + s + w + e) * 0.25;
        r = (nw + ne + sw + se) * 0.25;
    } else { // G site
        g = center;
        let horiz_color = bayer_color(x - 1, y);
        let vert_color = bayer_color(x, y - 1);
        if (horiz_color == 0) { // R is horizontal
            r = (w + e) * 0.5;
            b = (n + s) * 0.5;
        } else { // B is horizontal
            b = (w + e) * 0.5;
            r = (n + s) * 0.5;
        }
    }

    return vec3<f32>(r, g, b);
}

// ---------- OETF implementations ----------
// All match src/color.rs TransferFunction coefficients exactly.
// Discriminants: 0=Linear, 1=Rec709, 2=SLog3, 3=VLog, 4=ARRIlog3,
//   5=ARRIlog4, 6=CLog3, 7=FLog2, 8=AppleLog, 9=AppleLog2,
//   10=ACESCCT, 11=PQ, 12=HLG, 13=DaVinciIntermediate, 14=Gamma24

fn apply_oetf(linear: vec3<f32>, tf: u32) -> vec3<f32> {
    if (tf == 0u) { return linear; } // Linear
    if (tf == 14u) { return pow(max(linear, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)); } // Gamma24
    if (tf == 1u) { // Rec709
        let cutoff = 0.018;
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let v = linear[i];
            if (v < cutoff) {
                result[i] = 4.5 * v;
            } else {
                result[i] = 1.099 * pow(max(v, 0.0), 0.45) - 0.099;
            }
        }
        return result;
    }
    if (tf == 2u) { // SLog3
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let x = linear[i];
            if (x >= 0.01) {
                result[i] = 0.432699 * log10(10.0 * x + 1.0) + 0.037584;
            } else {
                result[i] = (x * 261.5 + 10.23) / 1023.0;
            }
        }
        return result;
    }
    if (tf == 3u) { // VLog
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let x = linear[i];
            if (x < 0.01) {
                result[i] = 5.6 * x + 0.125;
            } else {
                result[i] = 0.241514 * log10(x + 0.00873) + 0.598206;
            }
        }
        return result;
    }
    if (tf == 4u) { // ARRIlog3
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let x = linear[i];
            if (x > 0.010591) {
                result[i] = 0.247190 * log10(5.555556 * x + 0.052272) + 0.385537;
            } else {
                result[i] = 5.367655 * x + 0.092809;
            }
        }
        return result;
    }
    if (tf == 5u) { // ARRIlog4
        // Constants from arri_logc4_constants()
        let a = (262144.0 - 16.0) / 117.45; // (1<<18 - 16) / 117.45
        let b_rev = (1023.0 - 95.0) / 1023.0;
        let c_rev = 95.0 / 1023.0;
        let s_rev = (7.0 * 0.6931471805599453 * exp2(7.0 - 14.0 * c_rev / b_rev)) / (a * b_rev);
        let t_rev = (exp2(14.0 * (-c_rev / b_rev) + 6.0) - 64.0) / a;
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let x = linear[i];
            if (x >= t_rev) {
                result[i] = ((log2(a * x + 64.0) - 6.0) / 14.0) * b_rev + c_rev;
            } else {
                result[i] = (x - t_rev) / s_rev;
            }
        }
        return result;
    }
    if (tf == 6u) { // CLog3
        let neg_graft = (0.097465473 - 0.12512219) / 1.9754798;
        let pos_graft = (0.15277891 - 0.12512219) / 1.9754798;
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let x = linear[i];
            if (x < neg_graft) {
                result[i] = -0.36726845 * log10(max(-x * 14.98325 + 1.0, 1e-10)) + 0.12783901;
            } else if (x <= pos_graft) {
                result[i] = 1.9754798 * x + 0.12512219;
            } else {
                result[i] = 0.36726845 * log10(x * 14.98325 + 1.0) + 0.12240537;
            }
        }
        return result;
    }
    if (tf == 7u) { // FLog2
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let x = linear[i];
            if (x >= 0.000889) {
                result[i] = 0.245281 * log10(5.555556 * x + 0.064829) + 0.384316;
            } else {
                result[i] = 8.799461 * x + 0.092864;
            }
        }
        return result;
    }
    // AppleLog and AppleLog2 share the same formula
    if (tf == 8u || tf == 9u) {
        const R0 = -0.05641088;
        const RT = 0.01;
        const C_AP = 47.28711236;
        const BETA = 0.00964052;
        const GAMMA_A = 0.08550479;
        const DELTA = 0.69336945;
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let x = linear[i];
            if (x < R0) {
                result[i] = 0.0;
            } else if (x < RT) {
                result[i] = C_AP * (x - R0) * (x - R0);
            } else {
                result[i] = GAMMA_A * log2(x + BETA) + DELTA;
            }
        }
        return result;
    }
    if (tf == 10u) { // ACESCCT
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let x = linear[i];
            if (x > 0.0078125) {
                result[i] = (log2(x) + 9.72) / 17.52;
            } else {
                result[i] = 10.5402377416545 * x + 0.0729055341958355;
            }
        }
        return result;
    }
    if (tf == 11u) { // PQ (ST.2084)
        const M1: f32 = 0.1593017578125;
        const M2: f32 = 78.84375;
        const C1: f32 = 0.8359375;
        const C2: f32 = 18.8515625;
        const C3: f32 = 18.6875;
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let x = max(linear[i], 0.0);
            let x_m1 = pow(x, M1);
            let num = C1 + C2 * x_m1;
            let den = 1.0 + C3 * x_m1;
            result[i] = pow(max(num / den, 0.0), M2);
        }
        return result;
    }
    if (tf == 12u) { // HLG
        const HLG_KNEE: f32 = 1.0 / 12.0;
        const HLG_A: f32 = 0.17883277;
        const HLG_B: f32 = 0.28466892;
        const HLG_C: f32 = 0.55991073;
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let x = linear[i];
            if (x < HLG_KNEE) {
                result[i] = sqrt(3.0 * max(x, 0.0));
            } else {
                result[i] = HLG_A * log(max(12.0 * x - HLG_B, 1e-10)) + HLG_C;
            }
        }
        return result;
    }
    if (tf == 13u) { // DaVinciIntermediate
        const DVI_CUT: f32 = 0.00262409;
        const DVI_LIN: f32 = 10.44426855;
        const DVI_SLOPE: f32 = 0.07329248;
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let x = linear[i];
            if (x <= DVI_CUT) {
                result[i] = x * DVI_LIN;
            } else {
                result[i] = DVI_SLOPE * (log2(x + 0.0075) + 7.0);
            }
        }
        return result;
    }
    return linear;
}

// ---------- Inverse OETF (display compensation) ----------
// Decodes the working-space coded value back to linear for gamut mapping

fn inverse_oetf(encoded: vec3<f32>, tf: u32) -> vec3<f32> {
    if (tf == 0u) { return encoded; } // Linear
    if (tf == 14u) { // Gamma24 inverse
        return pow(max(encoded, vec3<f32>(0.0)), vec3<f32>(2.4));
    }
    if (tf == 1u) { // Rec709 inverse
        let cutoff = 0.081; // 4.5 * 0.018
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let v = encoded[i];
            if (v < cutoff) {
                result[i] = v / 4.5;
            } else {
                result[i] = pow((v + 0.099) / 1.099, 1.0 / 0.45);
            }
        }
        return result;
    }
    if (tf == 2u) { // SLog3 inverse
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let v = encoded[i];
            let knee_val = (0.01 * 261.5 + 10.23) / 1023.0;
            if (v >= knee_val) {
                result[i] = (pow(10.0, (v - 0.037584) / 0.432699) - 1.0) / 10.0;
            } else {
                result[i] = (v * 1023.0 - 10.23) / 261.5;
            }
        }
        return result;
    }
    if (tf == 3u) { // VLog inverse
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let v = encoded[i];
            if (v < 0.181) { // 5.6 * 0.01 + 0.125
                result[i] = (v - 0.125) / 5.6;
            } else {
                result[i] = pow(10.0, (v - 0.598206) / 0.241514) - 0.00873;
            }
        }
        return result;
    }
    if (tf == 4u) { // ARRIlog3 inverse
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let v = encoded[i];
            let knee_val = 5.367655 * 0.010591 + 0.092809;
            if (v >= knee_val) {
                result[i] = (pow(10.0, (v - 0.385537) / 0.247190) - 0.052272) / 5.555556;
            } else {
                result[i] = (v - 0.092809) / 5.367655;
            }
        }
        return result;
    }
    if (tf == 5u) { // ARRIlog4 inverse
        let a = (262144.0 - 16.0) / 117.45;
        let b_rev = (1023.0 - 95.0) / 1023.0;
        let c_rev = 95.0 / 1023.0;
        let s_rev = (7.0 * 0.6931471805599453 * exp2(7.0 - 14.0 * c_rev / b_rev)) / (a * b_rev);
        let t_rev = (exp2(14.0 * (-c_rev / b_rev) + 6.0) - 64.0) / a;
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let v = encoded[i];
            if (v >= 0.0) {
                result[i] = (exp2(14.0 * ((v - c_rev) / b_rev) + 6.0) - 64.0) / a;
            } else {
                result[i] = v * s_rev + t_rev;
            }
        }
        return result;
    }
    // For other log curves: approximate inverse by evaluating the forward
    // OETF at a grid and picking the nearest. But for display compensation
    // we use a simpler approach: apply the sRGB OETF directly since
    // the display is sRGB, and these spaces are designed to look correct
    // when viewed on a monitor with appropriate LUT.
    // CLog3, FLog2, AppleLog, ACESCCT, PQ, HLG, DaVinciIntermediate:
    // Use simplified analytical inverses where practical.

    if (tf == 6u) { // CLog3 inverse (approximate)
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let v = encoded[i];
            let neg_graft = (0.097465473 - 0.12512219) / 1.9754798;
            let pos_graft = (0.15277891 - 0.12512219) / 1.9754798;
            if (v < 0.12512219 + neg_graft * 1.9754798) {
                result[i] = (pow(10.0, -(v - 0.12783901) / 0.36726845) - 1.0) / (-14.98325);
            } else if (v <= 0.12512219 + pos_graft * 1.9754798) {
                result[i] = (v - 0.12512219) / 1.9754798;
            } else {
                result[i] = (pow(10.0, (v - 0.12240537) / 0.36726845) - 1.0) / 14.98325;
            }
        }
        return result;
    }
    if (tf == 7u) { // FLog2 inverse
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let v = encoded[i];
            let knee_val = 8.799461 * 0.000889 + 0.092864;
            if (v >= knee_val) {
                result[i] = (pow(10.0, (v - 0.384316) / 0.245281) - 0.064829) / 5.555556;
            } else {
                result[i] = (v - 0.092864) / 8.799461;
            }
        }
        return result;
    }
    if (tf == 8u || tf == 9u) { // AppleLog / AppleLog2 inverse
        const R0: f32 = -0.05641088;
        const RT: f32 = 0.01;
        const C_AP: f32 = 47.28711236;
        const BETA: f32 = 0.00964052;
        const GAMMA_A: f32 = 0.08550479;
        const DELTA: f32 = 0.69336945;
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let v = encoded[i];
            let knee_val = C_AP * (RT - R0) * (RT - R0);
            if (v <= 0.0) {
                result[i] = R0;
            } else if (v < knee_val) {
                let disc = sqrt(max(v / C_AP, 0.0));
                result[i] = disc + R0;
            } else {
                result[i] = exp2((v - DELTA) / GAMMA_A) - BETA;
            }
        }
        return result;
    }
    if (tf == 10u) { // ACESCCT inverse
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let v = encoded[i];
            if (v > 10.5402377416545 * 0.0078125 + 0.0729055341958355) {
                result[i] = exp2(v * 17.52 - 9.72);
            } else {
                result[i] = (v - 0.0729055341958355) / 10.5402377416545;
            }
        }
        return result;
    }
    if (tf == 11u) { // PQ inverse (ST.2084 EOTF)
        const M1: f32 = 0.1593017578125;
        const M2: f32 = 78.84375;
        const C1: f32 = 0.8359375;
        const C2: f32 = 18.8515625;
        const C3: f32 = 18.6875;
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let v = max(encoded[i], 0.0);
            let v_m2 = pow(v, 1.0 / M2);
            let num = max(v_m2 - C1, 0.0);
            let den = C2 - C3 * v_m2;
            if (den > 0.0) {
                result[i] = pow(num / den, 1.0 / M1);
            } else {
                result[i] = 0.0;
            }
        }
        return result;
    }
    if (tf == 12u) { // HLG inverse OETF
        const HLG_KNEE: f32 = 1.0 / 12.0;
        const HLG_A: f32 = 0.17883277;
        const HLG_B: f32 = 0.28466892;
        const HLG_C: f32 = 0.55991073;
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let v = encoded[i];
            let knee_out = sqrt(3.0 * HLG_KNEE); // ~0.5
            if (v <= knee_out) {
                result[i] = v * v / 3.0;
            } else {
                result[i] = (exp((v - HLG_C) / HLG_A) + HLG_B) / 12.0;
            }
        }
        return result;
    }
    if (tf == 13u) { // DaVinciIntermediate inverse
        const DVI_CUT_OUT: f32 = 0.00262409 * 10.44426855;
        const DVI_SLOPE: f32 = 0.07329248;
        var result = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let v = encoded[i];
            if (v <= DVI_CUT_OUT) {
                result[i] = v / 10.44426855;
            } else {
                result[i] = exp2(v / DVI_SLOPE - 7.0) - 0.0075;
            }
        }
        return result;
    }
    return encoded;
}

// ---------- sRGB OETF for terminal display ----------

fn srgb_oetf(linear: vec3<f32>) -> vec3<f32> {
    var result = vec3<f32>(0.0);
    for (var i = 0u; i < 3u; i = i + 1u) {
        let v = linear[i];
        if (v <= 0.0031308) {
            result[i] = v * 12.92;
        } else {
            result[i] = 1.055 * pow(max(v, 0.0), 1.0 / 2.4) - 0.055;
        }
    }
    return result;
}

// ---------- Gamut clipping to sRGB ----------
// For preview display: convert from working space to XYZ, then XYZ to
// sRGB linear, then clamp. Uses pre-baked matrices stored as
// per-color-space constants.
// The CCM in PreviewParams already transforms camera-native → working space.
// We need: working space → XYZ → sRGB.
// Stored as two 3x3 matrices per color space: to_xyz and from_xyz_to_srgb.
// For now, the simplest approach: if the working space IS sRGB/Rec709,
// just clamp. For other spaces, we need the conversion matrices.

fn gamut_clip_to_srgb(linear: vec3<f32>, cs: u32) -> vec3<f32> {
    // For Rec709 and sRGB working spaces, no conversion needed
    if (cs == 12u || cs == 15u) { // Rec709, Srgb
        return clamp(linear, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    // For other color spaces, apply the working-space → XYZ → sRGB
    // transform. We use the same approach as color.rs:
    //   1. Working RGB → XYZ using the space's primaries
    //   2. XYZ → sRGB linear using sRGB primaries
    let xyz = working_to_xyz(linear, cs);
    let srgb = xyz_to_rec709(xyz);
    return clamp(srgb, vec3<f32>(0.0), vec3<f32>(1.0));
}

// Working-space RGB → CIE XYZ (D65 adapted).
// Matrices computed from the same primary/chromaticity data in color.rs.
fn working_to_xyz(rgb: vec3<f32>, cs: u32) -> vec3<f32> {
    // Inline the most common working spaces used in production:
    // Rec2020 (cs=10), DCI-P3 (cs=5), DisplayP3 (cs=6),
    // ARRIWideGamut3 (cs=1), ARRIWideGamut4 (cs=2), DaVinciWideGamut (cs=4),
    // ACESAP1 (cs=0), SGamut3 (cs=12), SGamut3Cine (cs=13),
    // CanonCinemaGamut (cs=3), FGamut (cs=7), FGamutC (cs=8),
    // PanasonicVGamut (cs=9)
    // Each is a 3x3 row-major multiply.

    if (cs == 0u) { // ACESAP1 (AP0 primaries in AP1 container)
        return mat3x3<f32>(
            vec3<f32>(0.6954522414, 0.1406786965, 0.1638690622),
            vec3<f32>(0.0447945634, 0.8596711185, 0.0955343182),
            vec3<f32>(-0.0055258826, 0.0040252104, 1.0015006723)
        ) * rgb;
    }
    if (cs == 1u) { // Apple Wide Gamut (white=D65, R=0.725,0.301 G=0.221,0.814 B=0.068,-0.076)
        return mat3x3<f32>(
            vec3<f32>(1.99650669, -0.04380294, 0.04729625),
            vec3<f32>(0.50573456, 0.86522867, -0.37096323),
            vec3<f32>(0.00612684, -0.00089651, 0.99476967)
        ) * rgb;
    }
    if (cs == 2u) { // ARRIWideGamut3
        return mat3x3<f32>(
            vec3<f32>(0.688161, 0.150181, 0.161658),
            vec3<f32>(0.047434, 0.807529, 0.145037),
            vec3<f32>(-0.002103, -0.004533, 1.006636)
        ) * rgb;
    }
    if (cs == 3u) { // ARRIWideGamut4
        return mat3x3<f32>(
            vec3<f32>(0.732690, 0.143327, 0.123983),
            vec3<f32>(0.044200, 0.878486, 0.077314),
            vec3<f32>(-0.001988, -0.003142, 1.005130)
        ) * rgb;
    }
    if (cs == 5u) { // DaVinciWideGamut
        return mat3x3<f32>(
            vec3<f32>(0.8000, 0.3130, -0.1130),
            vec3<f32>(0.1682, 0.9877, -0.1559),
            vec3<f32>(0.0790, -0.1155, 1.0365)
        ) * rgb;
    }
    if (cs == 6u) { // DciP3
        return mat3x3<f32>(
            vec3<f32>(0.4865709, 0.2656677, 0.1982175),
            vec3<f32>(0.2289746, 0.6917385, 0.0792869),
            vec3<f32>(0.0, 0.0451136, 1.0439444)
        ) * rgb;
    }
    if (cs == 7u) { // DisplayP3
        return mat3x3<f32>(
            vec3<f32>(0.4865709, 0.2656677, 0.1982242),
            vec3<f32>(0.2289746, 0.6917385, 0.0792869),
            vec3<f32>(0.0, 0.0451136, 1.0439444)
        ) * rgb;
    }
    if (cs == 11u) { // Rec2020
        return mat3x3<f32>(
            vec3<f32>(0.6369580, 0.1446169, 0.1688810),
            vec3<f32>(0.2627002, 0.6779981, 0.0593017),
            vec3<f32>(0.0, 0.0280727, 1.0609052)
        ) * rgb;
    }
    // SGamut3, SGamut3Cine, CanonCinemaGamut, FGamut, FGamutC,
    // PanasonicVGamut: use the CCM output as-is with simple clamp.
    // These less common spaces will get a slightly inaccurate gamut
    // display but it won't cause visible artifacts for preview.
    return clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn xyz_to_rec709(xyz: vec3<f32>) -> vec3<f32> {
    return mat3x3<f32>(
        vec3<f32>(3.2404542, -0.9692660, 0.0556434),
        vec3<f32>(-1.5371385, 1.8760108, -0.2040259),
        vec3<f32>(-0.4985314, 0.0415560, 1.0572252)
    ) * xyz;
}

// ---------- Shadows / Highlights tone curve ----------
// Parametric lift/gain model (simplified DaVinci Resolve style):
//   shadows(): lifts blacks, compresses shadows
//   highlights(): compresses highlights, pulls down whites

fn apply_tone_curve(linear: vec3<f32>, shadows: f32, highlights: f32) -> vec3<f32> {
    // Shadows: parametric lift. Positive = lift blacks, negative = crush blacks.
    // Blend based on proximity to black (1 - luminance).
    let luma = dot(linear, vec3<f32>(0.2126, 0.7152, 0.0722));
    let shadow_weight = 1.0 - smoothstep(0.0, 0.35, luma);
    var result = linear + shadows * shadow_weight;

    // Highlights: parametric gain. Positive = brighten highlights, negative = darken.
    let hi_weight = smoothstep(0.5, 1.0, luma);
    result = result + highlights * hi_weight * result;

    return max(result, vec3<f32>(0.0));
}

// ---------- Main compute shader ----------

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let out_x = gid.x;
    let out_y = gid.y;
    if (out_x >= params.width || out_y >= params.height) { return; }

    // Map output pixel to bayer coordinate (handles downscaling for TUI)
    let src_x = i32((out_x * params.bayer_width) / params.width);
    let src_y = i32((out_y * params.bayer_height) / params.height);

    // 1. Demosaic
    var rgb = demosaic_bilinear(src_x, src_y);

    // 2. Normalize: subtract black, divide by (white - black)
    let range = max(params.white_level - params.black_level, 0.001);
    rgb = (rgb - vec3<f32>(params.black_level)) / vec3<f32>(range);

    // 3. Exposure (2^exposure gain)
    rgb = rgb * exp2(params.exposure);

    // 4. White balance
    rgb = rgb * vec3<f32>(params.wb_r, params.wb_g, params.wb_b);

    // 5. Camera Color Matrix (CCM): camera-native → working space
    let ccm = mat3x3<f32>(
        params.ccm_row0.xyz,
        params.ccm_row1.xyz,
        params.ccm_row2.xyz
    );
    rgb = ccm * rgb;

    // 6. Grading adjustments (only if enabled)
    if (params.adjust_enabled != 0u) {
        rgb = apply_tone_curve(rgb, params.shadows, params.highlights);
        // Contrast (pivot at 0.18 mid-grey)
        rgb = max(vec3<f32>(0.0), (rgb - 0.18) * params.contrast + 0.18);
        // Saturation
        let luma = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        rgb = mix(vec3<f32>(luma), rgb, params.saturation);
    }

    // 7. Apply selected OETF
    let encoded = apply_oetf(rgb, params.transfer);

    // 8. Display compensation for sRGB terminal:
    //    Decode the OETF → linear → gamut-clip to sRGB → sRGB OETF
    let linear_for_display = inverse_oetf(encoded, params.transfer);
    let srgb_linear = gamut_clip_to_srgb(linear_for_display, params.color_space);
    let display = srgb_oetf(srgb_linear);

    // 9. Histogram atomics (Phase 2 scopes)
    if (params.compute_histogram != 0u) {
        let luma = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        let bin_l = u32(clamp(luma, 0.0, 1.0) * 63.0);
        let bin_r = u32(clamp(rgb.r, 0.0, 1.0) * 63.0);
        let bin_g = u32(clamp(rgb.g, 0.0, 1.0) * 63.0);
        let bin_b = u32(clamp(rgb.b, 0.0, 1.0) * 63.0);
        atomicAdd(&hist_luma[bin_l], 1u);
        atomicAdd(&hist_r[bin_r], 1u);
        atomicAdd(&hist_g[bin_g], 1u);
        atomicAdd(&hist_b[bin_b], 1u);
    }

    // 10. Pack to RGBA8 and write
    let pixel = pack4x8unorm(vec4<f32>(display, 1.0));
    output_rgba[out_y * params.width + out_x] = pixel;
}
