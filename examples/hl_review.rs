//! Highlight review harness: runs the real export colour chain on a real
//! `.mcraw` frame twice — once with the sensor's clipped values untouched, once
//! with `src/hl.rs`'s completion applied — and produces (a) a machine-checkable
//! report and (b) images you can look at.
//!
//! ```text
//! cargo run --release --example hl_review -- <file.mcraw> [options]
//!
//!   --out DIR        output directory (default: hl-review-<stem>)
//!   --frame N        frame index to analyse (default: 0)
//!   --cs NAME        working colour space  (default: ARRI Wide Gamut 3)
//!   --tf NAME        transfer function     (default: ARRI LogC3)
//!   --view NAME      display view for the PNGs: log | rec709 | graded
//!                    (default: rec709)
//!   --policy NAME    completion policy: sensor | prior | full
//!                    (default: full; sensor = the --no-highlight-recovery
//!                    export behaviour: bit-exact sensor truth; prior = the
//!                    internal middle arm, multi-censored blocks only)
//!   --crop X,Y,W,H   1:1 crop rectangle in active-region pixels
//!                    (default: auto — centred on the largest censored blob)
//!   --max-dim N      longest PNG edge (default: 1600)
//!   --no-images      report only
//! ```
//!
//! Images written (all 8-bit PNG, sRGB-encoded for viewing):
//!
//! * `overview-before.png` / `overview-after.png` — the whole frame
//! * `crop-before.png` / `crop-after.png` — a 1:1 crop over the highlight
//! * `diff-xNN.png` — `|after − before|` amplified, so the change is visible
//! * `report.txt` — every number, per pin class
//!
//! Exits non-zero if any invariant fails, so it is usable as a gate.

use mcraw_tui::color::*;
use rayon::prelude::*;
use mcraw_tui::decoder::Decoder;
use mcraw_tui::file::{BayerPattern, McrawFileInfo};
use mcraw_tui::hl::{self, HlGeometry, HlParams, HlPolicy};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

const REC709_LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

struct Opts {
    file: PathBuf,
    out: Option<PathBuf>,
    frame: usize,
    cs: String,
    tf: String,
    view: String,
    policy: String,
    crop: Option<(u32, u32, u32, u32)>,
    max_dim: u32,
    images: bool,
}

fn parse_opts() -> Result<Opts, String> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 2 { return Err("usage: hl_review <file.mcraw> [options]".into()); }
    let mut o = Opts { file: PathBuf::from(&a[1]), out: None, frame: 0,
                       cs: "ARRI Wide Gamut 3".into(), tf: "ARRI LogC3".into(),
                       view: "rec709".into(), policy: "full".into(), crop: None, max_dim: 1600, images: true };
    let mut i = 2;
    while i < a.len() {
        let need = |i: usize| -> Result<&String, String> { a.get(i).ok_or_else(|| format!("{} needs a value", a[i])) };
        match a[i].as_str() {
            "--out" => { o.out = Some(PathBuf::from(need(i + 1)?)); i += 2; }
            "--frame" => { o.frame = need(i + 1)?.parse().map_err(|_| "bad --frame".to_string())?; i += 2; }
            "--cs" => { o.cs = need(i + 1)?.clone(); i += 2; }
            "--tf" => { o.tf = need(i + 1)?.clone(); i += 2; }
            "--view" => { o.view = need(i + 1)?.clone(); i += 2; }
            "--policy" => { o.policy = need(i + 1)?.clone(); i += 2; }
            "--crop" => {
                let v = need(i + 1)?;
                let p: Vec<u32> = v.split(',').map(|s| s.parse().unwrap_or(0)).collect();
                if p.len() != 4 { return Err("--crop needs X,Y,W,H".into()); }
                o.crop = Some((p[0], p[1], p[2], p[3]));
                i += 2;
            }
            "--max-dim" => { o.max_dim = need(i + 1)?.parse().unwrap_or(1600); i += 2; }
            "--no-images" => { o.images = false; i += 1; }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(o)
}

/// Everything the harness needs from a frame, resolved once.
struct Frame {
    bayer: Vec<u16>,
    as_shot: [f32; 3],
    bl: [f64; 3],
    wl: f64,
}

fn color_of(x: u32, y: u32, p: BayerPattern) -> usize { hl::color_at(x as i32, y as i32, p) }

/// Per-2x2-block census of which channels are censored.
fn pin_census(bayer: &[u16], g: &HlGeometry, params: &HlParams) -> Vec<u8> {
    let (w, h) = (g.width, g.height);
    let bx = w.div_ceil(2);
    let byn = h.div_ceil(2);
    let mut out = vec![0u8; bx * byn];
    for j in 0..byn {
        for i in 0..bx {
            let mut m = 0u8;
            for (dy, dx) in [(0usize, 0usize), (0, 1), (1, 0), (1, 1)] {
                let x = (i * 2 + dx).min(w - 1);
                let y = (j * 2 + dy).min(h - 1);
                let c = color_of(x as u32, y as u32, g.pattern);
                if hl::is_censored(bayer[g.index(x, y)], params) { m |= 1 << c; }
            }
            out[j * bx + i] = m;
        }
    }
    out
}

/// Largest connected run of >=2-censored blocks, returned as an active-region
/// rectangle big enough to see the transition.
fn largest_blob_rect(census: &[u8], w: usize, h: usize) -> Option<(u32, u32, u32, u32)> {
    let (bx, byn) = (w.div_ceil(2), h.div_ceil(2));
    let mut seen = vec![false; census.len()];
    let mut best: Option<(usize, (usize, usize, usize, usize))> = None;
    let mut stack: Vec<usize> = Vec::new();
    for s in 0..census.len() {
        if census[s].count_ones() < 2 || seen[s] { continue; }
        let (mut i0, mut j0, mut i1, mut j1) = (s % bx, s / bx, s % bx, s / bx);
        let mut n = 0usize;
        seen[s] = true;
        stack.push(s);
        while let Some(p) = stack.pop() {
            n += 1;
            let (pi, pj) = (p % bx, p / bx);
            i0 = i0.min(pi); i1 = i1.max(pi); j0 = j0.min(pj); j1 = j1.max(pj);
            for (di, dj) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let ni = pi as i32 + di; let nj = pj as i32 + dj;
                if ni < 0 || nj < 0 || ni >= bx as i32 || nj >= byn as i32 { continue; }
                let q = nj as usize * bx + ni as usize;
                if census[q].count_ones() >= 2 && !seen[q] { seen[q] = true; stack.push(q); }
            }
        }
        if best.is_none() || n > best.unwrap().0 { best = Some((n, (i0, j0, i1, j1))); }
    }
    let (_, (i0, j0, i1, j1)) = best?;
    // Photosite coords, padded by 3% of the blob so the transition is in frame.
    let (px0, py0) = (i0 * 2, j0 * 2);
    let (px1, py1) = (((i1 + 1) * 2).min(w), ((j1 + 1) * 2).min(h));
    let bw = (px1 - px0).max(8); let bh = (py1 - py0).max(8);
    let pad = ((bw.max(bh) as f32 * 0.03) as usize).max(4);
    let mut x0 = px0.saturating_sub(pad);
    let mut y0 = py0.saturating_sub(pad);
    let mut cw = (bw + 2 * pad).min(w - x0);
    let mut ch = (bh + 2 * pad).min(h - y0);
    // Minimum 512px window centred on the blob: a sparse specular field would
    // otherwise produce a postage stamp with no surrounding context to judge
    // the transition against. The overview always covers the full frame anyway.
    const MIN_CROP: usize = 512;
    if cw < MIN_CROP || ch < MIN_CROP {
        let cx = (px0 + px1) / 2; let cy = (py0 + py1) / 2;
        cw = MIN_CROP.min(w); ch = MIN_CROP.min(h);
        x0 = cx.saturating_sub(cw / 2).min(w - cw);
        y0 = cy.saturating_sub(ch / 2).min(h - ch);
    }
    Some((x0 as u32, y0 as u32, cw as u32, ch as u32))
}

fn main() {
    let o = match parse_opts() { Ok(o) => o, Err(e) => { eprintln!("hl_review: {e}"); std::process::exit(2); } };
    match run(&o) {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => { eprintln!("hl_review: {e:#}"); std::process::exit(3); }
    }
}

fn run(o: &Opts) -> Result<bool, anyhow::Error> {
    let info = McrawFileInfo::from_path(&o.file)?;
    let stride = info.width as usize;
    let ox = info.active_offset_x as usize;
    let oy = info.active_offset_y as usize;
    let w = if info.active_width > 0 { info.active_width as usize } else { stride };
    let h = if info.active_height > 0 { info.active_height as usize } else { info.height as usize };
    let pattern = info.bayer_pattern;
    let geom = HlGeometry { stride, offset_x: ox, offset_y: oy, width: w, height: h, pattern };

    let dec = Decoder::new(&o.file)?;
    let ts = dec.timestamps()?;
    if o.frame >= ts.len() { return Err(anyhow::anyhow!("--frame {} out of range ({} frames)", o.frame, ts.len())); }
    let mut bayer = vec![0u16; stride * info.height as usize];
    dec.load_frame_into(ts[o.frame], &mut bayer)?;
    let meta = dec.load_frame_metadata(ts[o.frame]).ok();
    let as_shot = meta.as_ref().map(|m| m.as_shot_neutral).unwrap_or([1.0, 1.0, 1.0]);
    let bls = match meta.as_ref().and_then(|m| m.dynamic_black_level) {
        Some(b) => [b[0] as f64, b[1] as f64, b[2] as f64, b[3] as f64],
        None => info.black_level_per_channel,
    };
    let wl = meta.as_ref().and_then(|m| m.dynamic_white_level).map(|v| v as f64).unwrap_or(info.white_level);
    let bl = [bls[0], (bls[1] + bls[2]) / 2.0, if info.black_level_count >= 4 { bls[3] } else { bls[0] }];
    let frame = Frame { bayer, as_shot, bl, wl };

    let policy = match o.policy.as_str() {
        "sensor" => HlPolicy::Sensor,
        "prior" => HlPolicy::Prior,
        "full" => HlPolicy::Full,
        other => return Err(anyhow::anyhow!("unknown --policy '{other}' (sensor|prior|full)")),
    };
    let params = HlParams::new(policy, frame.wl, frame.bl, frame.as_shot);
    let sensor = HlParams::new(HlPolicy::Sensor, frame.wl, frame.bl, frame.as_shot);

    // ---- colour chain, replicated from pipeline.rs (fused matrix + WB + OETF)
    let cs = ColorSpace::all().iter().copied().find(|c| c.name() == o.cs)
        .ok_or_else(|| anyhow::anyhow!("unknown colour space {}", o.cs))?;
    let tf = TransferFunction::all().iter().copied().find(|c| c.name() == o.tf)
        .ok_or_else(|| anyhow::anyhow!("unknown transfer function {}", o.tf))?;
    let fused = fused_matrix(&info, cs);

    let census = pin_census(&frame.bayer, &geom, &params);
    let n_censored: u64 = frame.bayer.iter().filter(|&&v| hl::is_censored(v, &params)).count() as u64;

    // ---- run the completion on a copy (this is the shipping code path)
    let mut completed = frame.bayer.clone();
    let mut scratch = hl::HlScratch::new();
    let t0 = std::time::Instant::now();
    let (_c, rewritten) = scratch.apply(&mut completed, &geom, &params);
    let pass_ms = t0.elapsed().as_secs_f64() * 1e3;

    // ---- render both through the chain. The `after` image mirrors the
    // export chain exactly, including the ON/display imbalance hedge for
    // Full policy (the `before` image is raw truth and never hedged).
    let hedge_after = policy == HlPolicy::Full && o.view != "log";
    let before = render(&frame.bayer, &geom, &frame, &fused, cs, tf, &o.view, false);
    let after = render(&completed, &geom, &frame, &fused, cs, tf, &o.view, hedge_after);

    // ---- report
    let out_dir = o.out.clone().unwrap_or_else(|| {
        let stem = o.file.file_stem().unwrap_or_default().to_string_lossy().to_string();
        PathBuf::from(format!("hl-review-{stem}"))
    });
    std::fs::create_dir_all(&out_dir)?;
    let mut rep = String::new();
    let mut ok = true;
    let mut say = |rep: &mut String, s: &str| { rep.push_str(s); rep.push('\n'); };

    say(&mut rep, "=== hl_review report ===");
    say(&mut rep, &format!("file      {}", o.file.display()));
    say(&mut rep, &format!("frame     {} of {}", o.frame, ts.len()));
    say(&mut rep, &format!("geometry  {}x{} active, stride {}, offset ({},{}), pattern {:?}", w, h, stride, ox, oy, pattern));
    say(&mut rep, &format!("levels    wl={} bl={:?} asShotNeutral={:?}", frame.wl, frame.bl, frame.as_shot));
    say(&mut rep, &format!("colour    cs={} tf={} view={}", cs.name(), tf.name(), o.view));
    say(&mut rep, &format!("params    {}", params.summary()));
    say(&mut rep, &format!("fused     [{:.4},{:.4},{:.4} | {:.4},{:.4},{:.4} | {:.4},{:.4},{:.4}]",
        fused[0], fused[1], fused[2], fused[3], fused[4], fused[5], fused[6], fused[7], fused[8]));

    // neutral identity
    let inv = invert_3x3(&fused);
    let n = mat_mul_vec3(&inv, &[1.0, 1.0, 1.0]);
    let g = if n[1].abs() > 1e-6 { n[1] } else { 1.0 };
    let dev = (n[0] / g - 1.0).abs().max((n[1] / g - 1.0).abs()).max((n[2] / g - 1.0).abs());
    say(&mut rep, &format!("neutral   M^-1(1,1,1) normalised = [{:.5},{:.5},{:.5}]  deviation {:.2e} {}",
        n[0] / g, 1.0, n[2] / g, dev, if dev < 1e-2 { "OK" } else { "SUSPECT (ColorMatrix1 path?)" }));
    if dev >= 1e-2 { ok = false; }

    say(&mut rep, "");
    say(&mut rep, "--- census (2x2 blocks) ---");
    let mut counts: BTreeMap<u8, u64> = BTreeMap::new();
    for &m in &census { *counts.entry(m).or_insert(0) += 1; }
    let total_blocks = census.len() as f64;
    for (m, n) in &counts {
        say(&mut rep, &format!("  {:<7} {:>10}  {:>6.2}%", hl::class_name(*m), n, 100.0 * *n as f64 / total_blocks));
    }
    say(&mut rep, &format!("photosites censored: {} of {} ({:.2}%)", n_censored, frame.bayer.len(),
        100.0 * n_censored as f64 / frame.bayer.len() as f64));
    say(&mut rep, &format!("photosites rewritten: {} ({:.2}% of censored)  pass={:.1} ms", rewritten,
        if n_censored > 0 { 100.0 * rewritten as f64 / n_censored as f64 } else { 0.0 }, pass_ms));

    // invariants
    say(&mut rep, "");
    say(&mut rep, "--- invariants ---");
    // I2: nothing below the rail
    let mut below = 0u64;
    for (i, (&a, &b)) in frame.bayer.iter().zip(completed.iter()).enumerate() {
        let _ = i;
        if b < a && (b as f64) < frame.wl - 0.5 { below += 1; }
    }
    let (c_ok, c_msg) = if below == 0 { (true, "no photo below the sensor rail".to_string()) }
                        else { (false, format!("{below} photosites were written BELOW the rail")) };
    say(&mut rep, &format!("  [{}] censoring interval: {}", if c_ok { "PASS" } else { "FAIL" }, c_msg));
    ok &= c_ok;

    // I1: no temper outside the censored set
    let mut tampered = 0u64;
    for y in 0..h {
        for x in 0..w {
            let i = geom.index(x, y);
            if hl::is_censored(frame.bayer[i], &params) { continue; }
            if frame.bayer[i] != completed[i] { tampered += 1; }
        }
    }
    let (c_ok, c_msg) = if tampered == 0 { (true, "uncensored photo sites are bit-identical".to_string()) }
                        else { (false, format!("{tampered} UNCENSORED photosites were modified")) };
    say(&mut rep, &format!("  [{}] no tempering: {}", if c_ok { "PASS" } else { "FAIL" }, c_msg));
    ok &= c_ok;

    // I7: bounded by the fallback
    let bound = (params.neutral[1] * params.fallback_scale).max(1.0);
    let mut worst = 0f32;
    for y in 0..h {
        for x in 0..w {
            let i = geom.index(x, y);
            if !hl::is_censored(frame.bayer[i], &params) { continue; }
            let c = color_of(x as u32, y as u32, pattern);
            let nrm = (completed[i] as f32 - params.black[c]) / params.range[c];
            let b = (params.neutral[c] * params.fallback_scale).max(1.0);
            worst = worst.max(nrm / b);
        }
    }
    let (c_ok, c_msg) = if worst <= 1.02 { (true, format!("max completion is {worst:.3} x the no-support bound")) }
                        else { (false, format!("completion reached {worst:.3} x the bound (expected <= 1.0)")) };
    say(&mut rep, &format!("  [{}] fallback bound: {}", if c_ok { "PASS" } else { "FAIL" }, c_msg));
    ok &= c_ok;
    let _ = bound;

    // idempotence
    let mut twice = completed.clone();
    let mut s2 = hl::HlScratch::new();
    let (_, again) = s2.apply(&mut twice, &geom, &params);
    let (c_ok, c_msg) = if again == 0 { (true, "second pass writes nothing".to_string()) }
                        else { (false, format!("second pass wrote {again} photosites — NOT idempotent")) };
    say(&mut rep, &format!("  [{}] idempotence: {}", if c_ok { "PASS" } else { "FAIL" }, c_msg));
    ok &= c_ok;

    // per-class chroma / luma, in the linear working space
    say(&mut rep, "");
    say(&mut rep, "--- per-class effect (linear working space, pre-OETF) ---");
    let bx = w.div_ceil(2); let byn = h.div_ceil(2);
    let mut acc: BTreeMap<u8, (u64, [f64; 3], [f64; 3], f64, f64, f64, f64)> = BTreeMap::new();
    for j in 0..byn {
        for i in 0..bx {
            let m = census[j * bx + i];
            let x = (i * 2).min(w - 1); let y = (j * 2).min(h - 1);
            let lb = linear_at(&frame.bayer, &geom, &params, &fused, x, y, pattern);
            let la = linear_at(&completed, &geom, &params, &fused, x, y, pattern);
            let e = acc.entry(m).or_insert((0, [0.0; 3], [0.0; 3], 0.0, 0.0, 0.0, 0.0));
            e.0 += 1;
            for c in 0..3 { e.1[c] += lb[c]; e.2[c] += la[c]; }
            e.3 += chroma(&lb);
            e.4 += chroma(&la);
            e.5 += REC709_LUMA[0] as f64 * lb[0] + REC709_LUMA[1] as f64 * lb[1] + REC709_LUMA[2] as f64 * lb[2];
            e.6 += REC709_LUMA[0] as f64 * la[0] + REC709_LUMA[1] as f64 * la[1] + REC709_LUMA[2] as f64 * la[2];
        }
    }
    say(&mut rep, "  class    n        chroma before -> after      luma before -> after   hue shift");
    for (m, e) in &acc {
        let n = e.0 as f64;
        let cb = e.3 / n; let ca = e.4 / n;
        let lb = e.5 / n; let la = e.6 / n;
        let mb = (e.1[0] / n).max(e.1[1] / n).max(e.1[2] / n).max(1e-9);
        let ma = (e.2[0] / n).max(e.2[1] / n).max(e.2[2] / n).max(1e-9);
        let hb = (e.1[1] / n) / mb; let ha = (e.2[1] / n) / ma;
        let hue_shift = (ha - hb).abs();
        say(&mut rep, &format!("  {:<7} {:<8} {:.4} -> {:.4}          {:.4} -> {:.4}      {:+.4}{}",
            hl::class_name(*m), e.0, cb, ca, lb, la, ha - hb,
            if *m == 0 { "" } else if ca < cb * 0.5 { "   <- magenta removed" } else if ca > cb { "   <- WORSE" } else { "" }));
        if *m != 0 && ca > cb + 0.01 { ok = false; }
    }

    // spatial detail inside the censored region: the anti-milkiness check
    say(&mut rep, "");
    say(&mut rep, "--- detail preservation inside censored blocks ---");
    say(&mut rep, "  (local sd of output luma, 3x3 block neighbourhoods; a recovery");
    say(&mut rep, "   that smooths would reduce it. Measured on the linear triple.)");
    let (db, da) = local_sd(&frame.bayer, &completed, &geom, &params, &fused, &census, pattern, bx, byn);
    say(&mut rep, &format!("  mean local sd: before {:.5}  after {:.5}  ratio {:.3}", db, da, da / db.max(1e-9)));
    let (c_ok, c_msg) = if da >= db { (true, "detail preserved or added".to_string()) }
                        else { (false, format!("detail LOST: sd fell {db:.5} -> {da:.5}")) };
    say(&mut rep, &format!("  [{}] no smoothing: {}", if c_ok { "PASS" } else { "FAIL" }, c_msg));
    ok &= c_ok;

    // ---- images
    if o.images {
        let crop = o.crop.unwrap_or_else(|| largest_blob_rect(&census, w, h).unwrap_or((0, 0, w.min(1024) as u32, h.min(1024) as u32)));
        say(&mut rep, "");
        say(&mut rep, &format!("--- images (crop {}x{} at {},{}) ---", crop.2, crop.3, crop.0, crop.1));
        write_png(&out_dir.join("overview-before.png"), &before, w, h, o.max_dim, None)?;
        write_png(&out_dir.join("overview-after.png"), &after, w, h, o.max_dim, None)?;
        write_png(&out_dir.join("crop-before.png"), &before, w, h, o.max_dim, Some(crop))?;
        write_png(&out_dir.join("crop-after.png"), &after, w, h, o.max_dim, Some(crop))?;
        // amplified difference over the crop
        let gain = 24.0f32;
        let mut diff: Vec<f32> = vec![0.0; (crop.2 * crop.3 * 3) as usize];
        for y in 0..crop.3 {
            for x in 0..crop.2 {
                let si = ((crop.1 + y) as usize * w + (crop.0 + x) as usize) * 3;
                let di = (y * crop.2 + x) as usize * 3;
                for c in 0..3 {
                    diff[di + c] = ((after[si + c] - before[si + c]).abs() * gain).clamp(0.0, 1.0);
                }
            }
        }
        write_png_raw(&out_dir.join("diff-amplified.png"), &diff, crop.2, crop.3)?;
        say(&mut rep, &format!("  diff gain: x{}", gain));
    }

    let rp = out_dir.join("report.txt");
    std::fs::File::create(&rp)?.write_all(rep.as_bytes())?;
    println!("{rep}");
    println!("wrote {}", rp.display());
    if o.images { println!("images in {}", out_dir.display()); }
    if ok { println!("\nALL INVARIANTS PASS"); } else { println!("\n*** INVARIANT FAILURE — see above ***"); }
    Ok(ok)
}

/// Demosaic one pixel pair and push it through WB + CCM, giving the linear
/// working-space triple for that pixel.
fn linear_at(raw: &[u16], g: &HlGeometry, p: &HlParams, fused: &[f32; 9],
             x: usize, y: usize, pattern: BayerPattern) -> [f64; 3] {
    // 2x2 block -> RGB via bilinear demosaic of just that neighbourhood
    let get = |xx: i32, yy: i32| -> (usize, f32) {
        let cx = xx.clamp(0, g.width as i32 - 1) as usize;
        let cy = yy.clamp(0, g.height as i32 - 1) as usize;
        let c = color_of(cx as u32, cy as u32, pattern);
        let v = (raw[g.index(cx, cy)] as f32 - p.black[c]) / p.range[c];
        (c, v)
    };
    let mut acc = [0f32; 3]; let mut cnt = [0u32; 3];
    for (dy, dx) in [(-1i32, -1i32), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1)] {
        let (c, v) = get(x as i32 + dx, y as i32 + dy);
        acc[c] += v; cnt[c] += 1;
    }
    let mut norm = [0f32; 3];
    for c in 0..3 { norm[c] = if cnt[c] > 0 { acc[c] / cnt[c] as f32 } else { 0.0 }; }
    // WB: gains are the reciprocal of the neutral triple.
    let gr = 1.0 / p.neutral[0]; let gb = 1.0 / p.neutral[2];
    let wb = [norm[0] * gr, norm[1], norm[2] * gb];
    let out = mat_mul_vec3(fused, &wb);
    [out[0].max(0.0) as f64, out[1].max(0.0) as f64, out[2].max(0.0) as f64]
}

fn chroma(c: &[f64; 3]) -> f64 {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    if mx <= 1e-6 { 0.0 } else { (mx - mn) / mx }
}

fn local_sd(before: &[u16], after: &[u16], g: &HlGeometry, p: &HlParams, fused: &[f32; 9],
            census: &[u8], pattern: BayerPattern, bx: usize, byn: usize) -> (f64, f64) {
    let mut sb = 0f64; let mut sa = 0f64; let mut n = 0f64;
    let luma = |raw: &[u16], x: usize, y: usize| -> f64 {
        let c = linear_at(raw, g, p, fused, x, y, pattern);
        REC709_LUMA[0] as f64 * c[0] + REC709_LUMA[1] as f64 * c[1] + REC709_LUMA[2] as f64 * c[2]
    };
    for j in 1..byn.saturating_sub(1) {
        for i in 1..bx.saturating_sub(1) {
            if census[j * bx + i].count_ones() == 0 { continue; }
            let x = i * 2; let y = j * 2;
            let mut mb = 0f64; let mut vb = 0f64; let mut ma = 0f64; let mut va = 0f64; let mut k = 0f64;
            for (dy, dx) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let lb = luma(before, (x as i32 + dx) as usize, (y as i32 + dy) as usize);
                let la = luma(after, (x as i32 + dx) as usize, (y as i32 + dy) as usize);
                mb += lb; ma += la; vb += lb * lb; va += la * la; k += 1.0;
            }
            sb += (vb / k - (mb / k).powi(2)).max(0.0).sqrt();
            sa += (va / k - (ma / k).powi(2)).max(0.0).sqrt();
            n += 1.0;
        }
    }
    (sb / n.max(1.0), sa / n.max(1.0))
}

/// Replicates `pipeline.rs`'s fused-matrix construction.
fn fused_matrix(info: &McrawFileInfo, cs: ColorSpace) -> [f32; 9] {
    let to_f32 = |o: Option<[f64; 9]>| o.map(|m| { let mut a = [0f32; 9]; for i in 0..9 { a[i] = m[i] as f32; } a });
    let fm1 = to_f32(info.camera_metadata.forward_matrix1);
    let fm2 = to_f32(info.camera_metadata.forward_matrix2);
    let cam_to_xyz: [f32; 9] = if let (Some(a), Some(b)) = (fm1, fm2) {
        let avg = interpolate_matrix(&a, &b, 0.5);
        let rs = [avg[0] + avg[1] + avg[2], avg[3] + avg[4] + avg[5], avg[6] + avg[7] + avg[8]];
        let d = (rs[0] - D50_XYZ[0]).powi(2) + (rs[1] - D50_XYZ[1]).powi(2) + (rs[2] - D50_XYZ[2]).powi(2);
        if d < 0.05 { avg } else { detect_camera_to_xyz(&avg) }
    } else if let Some(a) = fm1 {
        let rs = [a[0] + a[1] + a[2], a[3] + a[4] + a[5], a[6] + a[7] + a[8]];
        let d = (rs[0] - D50_XYZ[0]).powi(2) + (rs[1] - D50_XYZ[1]).powi(2) + (rs[2] - D50_XYZ[2]).powi(2);
        if d < 0.05 { a } else { detect_camera_to_xyz(&a) }
    } else {
        identity_ccm()
    };
    let cat = build_bradford_matrix(&D50_XYZ, &D65_XYZ);
    mat_mul_3x3(&cs.get_xyz_to_rgb_matrix(), &mat_mul_3x3(&cat, &cam_to_xyz))
}

/// Full chain: demosaic → normalise → WB → CCM → [hedge] → [rolloff] → OETF.
/// `hedge` mirrors the export ON/display imbalance hedge (pipeline.rs); the
/// raw-truth `before` image always passes false.
fn render(bayer: &[u16], g: &HlGeometry, f: &Frame, fused: &[f32; 9], cs: ColorSpace,
          tf: TransferFunction, view: &str, hedge: bool) -> Vec<f32> {
    let mut rgb = vec![0f32; g.width * g.height * 3];
    let dem = BilinearDemosaic::new(g.pattern);
    dem.process_par_into(bayer, g.stride as u32, g.offset_x as u32, g.offset_y as u32,
                         g.width as u32, g.height as u32, &g.pattern, &mut rgb).expect("demosaic");
    normalize_linear_per_channel(&mut rgb, f.bl[0], f.bl[1], f.bl[2], f.wl);
    let r_gain = f.as_shot[1] / f.as_shot[0].max(1e-6);
    let b_gain = f.as_shot[1] / f.as_shot[2].max(1e-6);
    rgb.par_chunks_exact_mut(3).for_each(|c| {
        let wb = [c[0] * r_gain, c[1], c[2] * b_gain];
        let o = mat_mul_vec3(fused, &wb);
        c[0] = o[0].max(0.0); c[1] = o[1].max(0.0); c[2] = o[2].max(0.0);
    });
    // Preview PNGs need display-referred codes for viewing. A log transfer
    // leaves scene codes, so it gets a viewing encode; any other transfer
    // already display-encoded the triple, and a second OETF would wash
    // brights to white and hide real chroma (it masked the Rec709-ON
    // residual tint through analysis — the after-PNG is a pipeline witness,
    // not a pretty picture).
    let needs_viewing_encode = view == "log";
    match view {
        "log" => { tf.process(&mut rgb); }
        _ => {
            if view == "graded" {
                // A pinned, deliberately contrasty look at the top end: this is
                // what makes the highlight boundary legible in a still.
                rgb.par_chunks_exact_mut(3).for_each(|c| {
                    for v in c.iter_mut() { *v = *v * 1.35; }
                });
            }
            if hedge { apply_on_highlight_imbalance_hedge(&mut rgb); }
            apply_display_rolloff(&mut rgb);
            tf.process(&mut rgb);
        }
    }
    if needs_viewing_encode {
        // log -> sRGB for viewing
        rgb.par_chunks_exact_mut(3).for_each(|c| {
            for v in c.iter_mut() { *v = rec709_oetf((*v).clamp(0.0, 1.0)); }
        });
    }
    let _ = cs;
    rgb
}

// ---------------------------------------------------------------------------
// Minimal 8-bit PNG writer (no external image dependency; the project is
// deliberately dependency-light and this is a review tool, not a deliverable).
// ---------------------------------------------------------------------------
fn write_png(path: &Path, px: &[f32], w: usize, h: usize, max_dim: u32, crop: Option<(u32, u32, u32, u32)>) -> Result<(), anyhow::Error> {
    let (cx, cy, cw, ch) = crop.unwrap_or((0, 0, w as u32, h as u32));
    anyhow::ensure!(cw > 0 && ch > 0 && cx + cw <= w as u32 && cy + ch <= h as u32,
                    "crop {cx},{cy},{cw},{ch} out of bounds for {w}x{h}");
    let mut out = vec![0f32; (cw * ch * 3) as usize];
    for y in 0..ch {
        for x in 0..cw {
            let si = ((cy + y) as usize * w + (cx + x) as usize) * 3;
            let di = (y * cw + x) as usize * 3;
            for c in 0..3 { out[di + c] = px[si + c]; }
        }
    }
    // Area-average downscale. Done before the encoder, never after: the encoder
    // is handed exactly ow*oh*3 samples and nothing else.
    let (ow, oh) = if cw.max(ch) > max_dim {
        let s = max_dim as f32 / cw.max(ch) as f32;
        (((cw as f32 * s) as u32).max(1), ((ch as f32 * s) as u32).max(1))
    } else { (cw, ch) };
    if (ow, oh) != (cw, ch) {
        let (cw, ch, ow, oh) = (cw as usize, ch as usize, ow as usize, oh as usize);
        let mut small = vec![0f32; (ow * oh * 3) as usize];
        for y in 0..oh {
            let y0 = y * ch / oh;
            let y1 = (((y + 1) * ch) / oh).max(y0 + 1).min(ch);
            for x in 0..ow {
                let x0 = x * cw / ow;
                let x1 = (((x + 1) * cw) / ow).max(x0 + 1).min(cw);
                let (mut acc, mut n) = ([0f64; 3], 0f64);
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let si = (sy * cw + sx) * 3;
                        for c in 0..3 { acc[c] += out[si + c] as f64; }
                        n += 1.0;
                    }
                }
                let di = (y * ow + x) * 3;
                for c in 0..3 { small[di + c] = (acc[c] / n) as f32; }
            }
        }
        out = small;
    }
    write_png_raw(path, &out, ow, oh)
}

fn write_png_raw(path: &Path, px: &[f32], w: u32, h: u32) -> Result<(), anyhow::Error> {
    // 8-bit RGB, filter type 0 on every row, zlib "stored" deflate blocks.
    // 8 bits is deliberate: the visual gate is for looking at, and 16-bit RGB
    // PNGs are not universally decodable by viewers.
    anyhow::ensure!(px.len() >= (w as usize * h as usize * 3),
                    "encoder given {} samples, needs {}", px.len(), w as usize * h as usize * 3);
    let mut raw = Vec::with_capacity((w * h * 3 + h) as usize);
    for y in 0..h as usize {
        raw.push(0);
        for x in 0..w as usize {
            let i = (y * w as usize + x) * 3;
            for c in 0..3 {
                let v = (px[i + c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                raw.push(v);
            }
        }
    }
    let idat = zlib_stored(&raw);
    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, truecolour
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &idat);
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, &png)?;
    // Self-check: a malformed image that still "writes fine" is worse than no
    // image, because the visual gate would silently pass. Verify the signature
    // and that IHDR decodes back to exactly the dimensions we intended.
    let back = std::fs::read(path)?;
    anyhow::ensure!(back.len() > 33 && &back[..8] == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
                    && &back[12..16] == b"IHDR", "{} is not a PNG", path.display());
    let dw = u32::from_be_bytes([back[16], back[17], back[18], back[19]]);
    let dh = u32::from_be_bytes([back[20], back[21], back[22], back[23]]);
    anyhow::ensure!(dw == w && dh == h && back[24] == 8 && back[25] == 2,
                    "{} decodes as {dw}x{dh} depth {} colour {}, expected {w}x{h} depth 8 colour 2",
                    path.display(), back[24], back[25]);
    Ok(())
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_data = Vec::with_capacity(4 + data.len());
    crc_data.extend_from_slice(kind);
    crc_data.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_data).to_be_bytes());
}

fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01];
    let mut i = 0;
    while i < data.len() {
        let n = (data.len() - i).min(65535);
        let last = if i + n >= data.len() { 1u8 } else { 0u8 };
        z.push(last);
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&data[i..i + n]);
        i += n;
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&(((b << 16) | a) as u32).to_be_bytes());
    z
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, e) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 { c = if c & 1 != 0 { 0xedb88320 ^ (c >> 1) } else { c >> 1 }; }
        *e = c;
    }
    let mut c = 0xffffffffu32;
    for &b in data { c = table[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8); }
    c ^ 0xffffffff
}
