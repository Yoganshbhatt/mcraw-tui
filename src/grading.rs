/// Per-file grading adjustments, stored as JSON sidecars alongside source
/// MCRAW files. Each field defaults to "no-op" (0.0 or false) so that a
/// file without a sidecar renders identically to the baked-in decode.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RawAdjustments {
    pub enabled: bool,
    pub exposure_stops: f32,
    pub white_balance_kelvin: u16,
    pub white_balance_tint: f32,
    pub lift_r: f32,
    pub lift_g: f32,
    pub lift_b: f32,
    pub gamma_r: f32,
    pub gamma_g: f32,
    pub gamma_b: f32,
    pub gain_r: f32,
    pub gain_g: f32,
    pub gain_b: f32,
    pub contrast: f32,
    pub saturation: f32,
}

impl Default for RawAdjustments {
    fn default() -> Self {
        Self {
            enabled: false,
            exposure_stops: 0.0,
            white_balance_kelvin: 5500,
            white_balance_tint: 0.0,
            lift_r: 0.0, lift_g: 0.0, lift_b: 0.0,
            gamma_r: 1.0, gamma_g: 1.0, gamma_b: 1.0,
            gain_r: 1.0, gain_g: 1.0, gain_b: 1.0,
            contrast: 1.0,
            saturation: 1.0,
        }
    }
}
