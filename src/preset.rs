use std::path::PathBuf;

use crate::color::{ColorSpace, TransferFunction};
use crate::export::{
    Av1Profile, CodecFamily, DnxhrProfile, H264Profile, HevcProfile, ProResProfile,
    RateControl, Vp9Profile,
};

/// A named bundle of every export setting that the user can configure.
///
/// `ExportPreset` is the on-disk representation used by the preset save/load
/// feature. It captures *all* knob positions so applying a preset is a
/// straight field copy.
#[derive(Debug, Clone)]
pub struct ExportPreset {
    pub name: String,

    pub color_space: ColorSpace,
    pub transfer_function: TransferFunction,
    pub codec_family: CodecFamily,

    pub prores_profile: ProResProfile,
    pub dnxhr_profile: DnxhrProfile,
    pub hevc_profile: HevcProfile,
    pub h264_profile: H264Profile,
    pub av1_profile: Av1Profile,
    pub vp9_profile: Vp9Profile,

    pub rate_control: RateControl,
    /// Optional export folder override. When `None`, the runtime default
    /// (file's parent directory) is used.
    pub export_folder: Option<PathBuf>,
}

/// On-disk file format. Held as a thin wrapper so we can evolve the JSON
/// schema without breaking existing `presets.json` files.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct PresetFile {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    presets: Vec<StoredPreset>,
}

/// Flat, stringly-typed mirror of `ExportPreset` for JSON serialization.
/// We keep this separate from `ExportPreset` so that future changes to
/// the in-memory representation (e.g. a richer rate-control type) don't
/// silently break saved files.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredPreset {
    name: String,

    color_space: String,
    transfer_function: String,
    codec_family: String,

    #[serde(default)]
    prores_profile: Option<String>,
    #[serde(default)]
    dnxhr_profile: Option<String>,
    #[serde(default)]
    hevc_profile: Option<String>,
    #[serde(default)]
    h264_profile: Option<String>,
    #[serde(default)]
    av1_profile: Option<String>,
    #[serde(default)]
    vp9_profile: Option<String>,

    /// Either a `RateControl` name (e.g. "Lossless", "High Quality", "Custom"),
    /// or "Custom:<value>" for the custom string variant.
    rate_control: String,
    #[serde(default)]
    export_folder: Option<PathBuf>,
}

const PRESETS_SCHEMA_VERSION: u32 = 1;

impl ExportPreset {
    /// Snapshot the current settings into a named preset.
    pub fn snapshot(
        name: String,
        color_space: ColorSpace,
        transfer_function: TransferFunction,
        codec_family: CodecFamily,
        prores_profile: ProResProfile,
        dnxhr_profile: DnxhrProfile,
        hevc_profile: HevcProfile,
        h264_profile: H264Profile,
        av1_profile: Av1Profile,
        vp9_profile: Vp9Profile,
        rate_control: RateControl,
        export_folder: Option<PathBuf>,
    ) -> Self {
        Self {
            name,
            color_space,
            transfer_function,
            codec_family,
            prores_profile,
            dnxhr_profile,
            hevc_profile,
            h264_profile,
            av1_profile,
            vp9_profile,
            rate_control,
            export_folder,
        }
    }

    /// Path to the user's presets file. Same directory as `favourites.json`.
    pub fn presets_file() -> Option<PathBuf> {
        let mut dir = dirs::config_dir()?;
        dir.push("mcraw-tui");
        std::fs::create_dir_all(&dir).ok()?;
        dir.push("presets.json");
        Some(dir)
    }

    /// Read all stored presets. Missing/corrupt files yield an empty list.
    pub fn load_all() -> Vec<ExportPreset> {
        let path = match Self::presets_file() {
            Some(p) => p,
            None => return Vec::new(),
        };
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };
        let parsed: PresetFile = match serde_json::from_str(&data) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("presets.json parse failed, ignoring: {}", e);
                return Vec::new();
            }
        };
        parsed.presets.into_iter().filter_map(Self::from_stored).collect()
    }

    /// Write the full list of presets, replacing any previous contents.
    pub fn save_all(presets: &[ExportPreset]) {
        let path = match Self::presets_file() {
            Some(p) => p,
            None => return,
        };
        let stored: Vec<StoredPreset> = presets.iter().map(Self::to_stored).collect();
        let file = PresetFile { schema_version: PRESETS_SCHEMA_VERSION, presets: stored };
        match serde_json::to_string_pretty(&file) {
            Ok(data) => {
                if let Err(e) = std::fs::write(&path, data) {
                    tracing::warn!("failed to write presets.json: {}", e);
                }
            }
            Err(e) => tracing::warn!("failed to serialize presets: {}", e),
        }
    }

    /// Replace a preset of the same name (or append). Preserves the order of
    /// the existing list with the matching slot updated.
    pub fn upsert(list: &mut Vec<ExportPreset>, preset: ExportPreset) {
        if let Some(pos) = list.iter().position(|p| p.name == preset.name) {
            list[pos] = preset;
        } else {
            list.push(preset);
        }
    }

    /// Remove a preset by name. Returns true if anything was removed.
    pub fn remove_by_name(list: &mut Vec<ExportPreset>, name: &str) -> bool {
        let before = list.len();
        list.retain(|p| p.name != name);
        list.len() != before
    }

    fn to_stored(p: &ExportPreset) -> StoredPreset {
        StoredPreset {
            name: p.name.clone(),
            color_space: cs_to_str(p.color_space).to_string(),
            transfer_function: tf_to_str(p.transfer_function).to_string(),
            codec_family: p.codec_family.name().to_string(),
            prores_profile: Some(p.prores_profile.name().to_string()),
            dnxhr_profile: Some(p.dnxhr_profile.name().to_string()),
            hevc_profile: Some(p.hevc_profile.name().to_string()),
            h264_profile: Some(p.h264_profile.name().to_string()),
            av1_profile: Some(p.av1_profile.name().to_string()),
            vp9_profile: Some(p.vp9_profile.name().to_string()),
            rate_control: rate_to_str(&p.rate_control),
            export_folder: p.export_folder.clone(),
        }
    }

    fn from_stored(s: StoredPreset) -> Option<ExportPreset> {
        let color_space = cs_from_str(&s.color_space)?;
        let transfer_function = tf_from_str(&s.transfer_function)?;
        let codec_family = codec_from_str(&s.codec_family)?;
        let prores_profile = s.prores_profile.as_deref()
            .and_then(prores_from_str).unwrap_or_default();
        let dnxhr_profile = s.dnxhr_profile.as_deref()
            .and_then(dnxhr_from_str).unwrap_or_default();
        let hevc_profile = s.hevc_profile.as_deref()
            .and_then(hevc_from_str).unwrap_or_default();
        let h264_profile = s.h264_profile.as_deref()
            .and_then(h264_from_str).unwrap_or_default();
        let av1_profile = s.av1_profile.as_deref()
            .and_then(av1_from_str).unwrap_or_default();
        let vp9_profile = s.vp9_profile.as_deref()
            .and_then(vp9_from_str).unwrap_or_default();
        let rate_control = rate_from_str(&s.rate_control);

        Some(ExportPreset {
            name: s.name,
            color_space,
            transfer_function,
            codec_family,
            prores_profile,
            dnxhr_profile,
            hevc_profile,
            h264_profile,
            av1_profile,
            vp9_profile,
            rate_control,
            export_folder: s.export_folder,
        })
    }
}

// ---------------------------------------------------------------------------
// Enum <-> string conversion helpers
// ---------------------------------------------------------------------------
//
// We avoid the `strum` dependency and write the conversions by hand. The
// lookups are `O(n)` but `n` is tiny and these run only on save/load.

fn cs_to_str(cs: ColorSpace) -> &'static str {
    // Match the display name so users see consistent strings in JSON.
    cs.name()
}

fn cs_from_str(s: &str) -> Option<ColorSpace> {
    ColorSpace::all().iter().copied().find(|c| c.name() == s)
}

fn tf_to_str(tf: TransferFunction) -> &'static str {
    tf.name()
}

fn tf_from_str(s: &str) -> Option<TransferFunction> {
    TransferFunction::all().iter().copied().find(|t| t.name() == s)
}

fn codec_from_str(s: &str) -> Option<CodecFamily> {
    CodecFamily::all().iter().copied().find(|c| c.name() == s)
}

fn prores_from_str(s: &str) -> Option<ProResProfile> {
    ProResProfile::all().iter().copied().find(|p| p.name() == s)
}

fn dnxhr_from_str(s: &str) -> Option<DnxhrProfile> {
    DnxhrProfile::all().iter().copied().find(|p| p.name() == s)
}

fn hevc_from_str(s: &str) -> Option<HevcProfile> {
    HevcProfile::all().iter().copied().find(|p| p.name() == s)
}

fn h264_from_str(s: &str) -> Option<H264Profile> {
    H264Profile::all().iter().copied().find(|p| p.name() == s)
}

fn av1_from_str(s: &str) -> Option<Av1Profile> {
    Av1Profile::all().iter().copied().find(|p| p.name() == s)
}

fn vp9_from_str(s: &str) -> Option<Vp9Profile> {
    Vp9Profile::all().iter().copied().find(|p| p.name() == s)
}

fn rate_to_str(r: &RateControl) -> String {
    r.name()
}

fn rate_from_str(s: &str) -> RateControl {
    // Stored form is the display name produced by `RateControl::name()`.
    // For the named variants that's e.g. "Lossless", "High Quality",
    // "Master 400M", "Standard 150M". For the custom variant the name
    // is "Custom: [value]" (or "Custom: []" for an empty value).
    if let Some(inner) = s.strip_prefix("Custom: [").and_then(|x| x.strip_suffix(']')) {
        return RateControl::Custom(inner.to_string());
    }
    match s {
        "Lossless" => RateControl::Lossless,
        "High Quality" => RateControl::High,
        "Standard" => RateControl::Standard,
        "Master 400M" => RateControl::Master400M,
        "Standard 150M" => RateControl::Standard150M,
        _ => RateControl::Lossless, // Forward-compat fallback.
    }
}

// ---------------------------------------------------------------------------
// Default impls for codec-profile enums
// ---------------------------------------------------------------------------
//
// The profile enums in `export.rs` are `Copy + PartialEq` but do not derive
// `Default`. Local impls let us write `unwrap_or_default()` when a stored
// preset omits a field (forward-compatibility for old presets files).

impl Default for ProResProfile { fn default() -> Self { ProResProfile::HQ } }
impl Default for DnxhrProfile { fn default() -> Self { DnxhrProfile::HQX } }
impl Default for HevcProfile { fn default() -> Self { HevcProfile::Main10_420 } }
impl Default for H264Profile { fn default() -> Self { H264Profile::Main8bit } }
impl Default for Av1Profile { fn default() -> Self { Av1Profile::Profile0_420_10bit } }
impl Default for Vp9Profile { fn default() -> Self { Vp9Profile::Profile2_420_10bit } }

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ExportPreset {
        ExportPreset::snapshot(
            "ARRIRAW-bake".to_string(),
            ColorSpace::ARRIWideGamut4,
            TransferFunction::ARRIlog4,
            CodecFamily::ProRes,
            ProResProfile::HQ,
            DnxhrProfile::HQX,
            HevcProfile::Main10_420,
            H264Profile::Main8bit,
            Av1Profile::Profile0_420_10bit,
            Vp9Profile::Profile2_420_10bit,
            RateControl::Lossless,
            Some(PathBuf::from("/tmp/out")),
        )
    }

    #[test]
    fn roundtrip_through_stored() {
        let original = sample();
        let stored = ExportPreset::to_stored(&original);
        let recovered = ExportPreset::from_stored(stored).expect("from_stored");
        assert_eq!(recovered.name, original.name);
        assert_eq!(recovered.color_space, original.color_space);
        assert_eq!(recovered.transfer_function, original.transfer_function);
        assert_eq!(recovered.codec_family, original.codec_family);
        assert_eq!(recovered.prores_profile, original.prores_profile);
        assert_eq!(recovered.export_folder, original.export_folder);
    }

    #[test]
    fn upsert_replaces_same_name() {
        let mut list = vec![sample()];
        let mut updated = sample();
        updated.color_space = ColorSpace::Rec709;
        ExportPreset::upsert(&mut list, updated);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].color_space, ColorSpace::Rec709);
    }

    #[test]
    fn upsert_appends_new_name() {
        let mut list = vec![sample()];
        let mut new_one = sample();
        new_one.name = "ProRes-4444".to_string();
        ExportPreset::upsert(&mut list, new_one);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn remove_by_name_drops_matching() {
        let mut list = vec![sample()];
        let mut second = sample();
        second.name = "second".to_string();
        list.push(second);
        assert!(ExportPreset::remove_by_name(&mut list, "second"));
        assert_eq!(list.len(), 1);
        assert!(!ExportPreset::remove_by_name(&mut list, "missing"));
    }

    #[test]
    fn custom_rate_roundtrip() {
        let mut p = sample();
        p.rate_control = RateControl::Custom("80M".to_string());
        let stored = ExportPreset::to_stored(&p);
        let recovered = ExportPreset::from_stored(stored).expect("from_stored");
        if let RateControl::Custom(s) = recovered.rate_control {
            assert_eq!(s, "80M");
        } else {
            panic!("expected Custom, got {:?}", recovered.rate_control);
        }
    }
}
