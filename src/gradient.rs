use ratatui::style::Color;

/// Multi-stop gradient for warm tones (Exposure slider, progress bars, borders).
/// #1A0505 (deep soil) → #3A1A15 (dark bark) → #6B3A2A (amber shadow)
///   → #B57A35 (warm amber) → #E8A035 (golden amber).
pub const GRADIENT_WARM: &[(Color, f32); 5] = &[
    (Color::Rgb(0x1A, 0x05, 0x05), 0.0),
    (Color::Rgb(0x3A, 0x1A, 0x15), 0.25),
    (Color::Rgb(0x6B, 0x3A, 0x2A), 0.50),
    (Color::Rgb(0xB5, 0x7A, 0x35), 0.75),
    (Color::Rgb(0xE8, 0xA0, 0x35), 1.0),
];

/// Multi-stop gradient for cool tones (White Balance slider, scope backgrounds).
/// #0A1A25 (deep river) → #1A3A45 (deep water) → #4D8A8A (teal)
///   → #6DAEAE (mist) → #E8E4D9 (parchment).
pub const GRADIENT_COOL: &[(Color, f32); 5] = &[
    (Color::Rgb(0x0A, 0x1A, 0x25), 0.0),
    (Color::Rgb(0x1A, 0x3A, 0x45), 0.25),
    (Color::Rgb(0x4D, 0x8A, 0x8A), 0.50),
    (Color::Rgb(0x6D, 0xAE, 0xAE), 0.75),
    (Color::Rgb(0xE8, 0xE4, 0xD9), 1.0),
];

/// Linear interpolation between two RGB colors.
/// `t=0` returns `a`, `t=1` returns `b`. `t` is clamped to [0, 1].
/// If either color is not `Color::Rgb`, returns `a` unchanged.
pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => Color::Rgb(
            (ar as f32 + (br as f32 - ar as f32) * t) as u8,
            (ag as f32 + (bg as f32 - ag as f32) * t) as u8,
            (ab as f32 + (bb as f32 - ab as f32) * t) as u8,
        ),
        _ => a,
    }
}

/// Interpolate across a multi-stop gradient at position `t` (0.0 — 1.0).
///
/// `stops` must be sorted by position. Returns the nearest stop color if `t`
/// falls before the first or after the last stop.
pub fn multi_stop_color(stops: &[(Color, f32)], t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    if stops.is_empty() {
        return Color::Rgb(0, 0, 0);
    }
    if stops.len() == 1 {
        return stops[0].0;
    }
    for i in 0..stops.len() - 1 {
        if t >= stops[i].1 && t <= stops[i + 1].1 {
            let seg_len = stops[i + 1].1 - stops[i].1;
            let seg_t = if seg_len == 0.0 { 0.0 } else { (t - stops[i].1) / seg_len };
            return lerp_color(stops[i].0, stops[i + 1].0, seg_t);
        }
    }
    stops.last().unwrap().0
}

/// Pre-compute a horizontal row of gradient colors.
///
/// Returns one `Color` per cell for the given `width`. Useful for caching
/// per-cell border or progress-bar colors across consecutive renders.
pub fn gradient_horizontal(width: u16, stops: &[(Color, f32)]) -> Vec<Color> {
    if width == 0 {
        return Vec::new();
    }
    (0..width)
        .map(|i| {
            let t = i as f32 / (width - 1) as f32;
            multi_stop_color(stops, t)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_identity() {
        let a = Color::Rgb(10, 20, 30);
        let b = Color::Rgb(100, 200, 250);
        assert_eq!(lerp_color(a, b, 0.0), a);
        assert_eq!(lerp_color(a, b, 1.0), b);
    }

    #[test]
    fn lerp_midpoint() {
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(100, 200, 0);
        assert_eq!(lerp_color(a, b, 0.5), Color::Rgb(50, 100, 0));
    }

    #[test]
    fn lerp_clamps_t() {
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(100, 0, 0);
        assert_eq!(lerp_color(a, b, -0.5), a);
        assert_eq!(lerp_color(a, b, 1.5), b);
    }

    #[test]
    fn lerp_non_rgb_fallback() {
        let a = Color::Red;
        let b = Color::Rgb(100, 0, 0);
        assert_eq!(lerp_color(a, b, 0.5), a);
    }

    #[test]
    fn multi_stop_single() {
        let stops = &[(Color::Rgb(50, 50, 50), 0.0)];
        assert_eq!(multi_stop_color(stops, 0.0), Color::Rgb(50, 50, 50));
        assert_eq!(multi_stop_color(stops, 0.5), Color::Rgb(50, 50, 50));
    }

    #[test]
    fn multi_stop_warm_midpoint() {
        let c = multi_stop_color(GRADIENT_WARM, 0.5);
        // At t=0.5: lands exactly on stop 2 (#6B3A2A)
        assert_eq!(c, Color::Rgb(0x6B, 0x3A, 0x2A));
    }

    #[test]
    fn gradient_horizontal_empty() {
        assert!(gradient_horizontal(0, GRADIENT_WARM).is_empty());
    }

    #[test]
    fn gradient_horizontal_single() {
        let colors = gradient_horizontal(1, GRADIENT_WARM);
        assert_eq!(colors.len(), 1);
        // When width=1, t=NaN → falls to last stop
        assert_eq!(colors[0], GRADIENT_WARM.last().unwrap().0);
    }

    #[test]
    fn gradient_horizontal_width_matches() {
        let colors = gradient_horizontal(10, GRADIENT_WARM);
        assert_eq!(colors.len(), 10);
        assert_eq!(colors[0], GRADIENT_WARM[0].0);
        assert_eq!(colors[9], GRADIENT_WARM.last().unwrap().0);
    }
}
