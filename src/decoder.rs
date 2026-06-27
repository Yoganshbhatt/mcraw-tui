use anyhow::{anyhow, Result};
use serde_json::Value;
use std::io::Write;
use std::path::Path;

pub struct Decoder {
    inner: motioncam_decoder::Decoder,
}

#[derive(Debug, Clone)]
pub struct LensShadingMap {
    pub channels: Vec<Vec<f32>>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct ContainerMetadata {
    pub color_matrix1: [f32; 9],
    pub color_matrix2: [f32; 9],
    pub forward_matrix1: [f32; 9],
    pub forward_matrix2: [f32; 9],
    pub calibration_matrix1: [f32; 9],
    pub calibration_matrix2: [f32; 9],
    pub calibration_illuminant1: i32,
    pub calibration_illuminant2: i32,
    pub has_calibration_illuminants: bool,
    pub white_level: f64,
    pub black_level: [f64; 4],
    pub black_level_count: i32,
    pub audio_sample_rate_hz: i32,
    pub num_audio_channels: i32,
    pub lens_shading_map: Option<LensShadingMap>,
}

#[derive(Debug, Clone)]
pub struct FrameMetadata {
    pub width: u32,
    pub height: u32,
    pub timestamp_ns: i64,
    pub as_shot_neutral: [f32; 3],
    pub exposure_time: f64,
    pub iso: f32,
    pub focal_length: f32,
    pub aperture: f32,
    pub dynamic_black_level: Option<[f32; 4]>,
    pub dynamic_white_level: Option<f32>,
    pub lens_shading_map: Option<LensShadingMap>,
}

fn json_to_matrix9(val: &Value, key: &str) -> [f32; 9] {
    let mut result = [0.0f32; 9];
    if let Some(arr) = val.get(key).and_then(|v| v.as_array()) {
        for (i, item) in arr.iter().enumerate().take(9) {
            if let Some(n) = item.as_f64() {
                result[i] = n as f32;
            }
        }
    }
    result
}

fn json_to_black_level(val: &Value) -> ([f64; 4], i32) {
    let mut result = [0.0f64; 4];
    let mut count = 0i32;
    if let Some(arr) = val.get("blackLevel").and_then(|v| v.as_array()) {
        for (i, item) in arr.iter().enumerate().take(4) {
            if let Some(n) = item.as_f64() {
                result[i] = n;
                count = (i + 1) as i32;
            }
        }
    }
    (result, count)
}

fn json_to_as_shot_neutral(val: &Value) -> [f32; 3] {
    let mut result = [1.0f32; 3];
    if let Some(arr) = val.get("asShotNeutral").and_then(|v| v.as_array()) {
        for (i, item) in arr.iter().enumerate().take(3) {
            if let Some(n) = item.as_f64() {
                result[i] = n as f32;
            }
        }
    }
    result
}

fn json_to_lens_shading_map(val: &Value) -> Option<LensShadingMap> {
    let map_arr = val.get("lensShadingMap").and_then(|v| v.as_array())?;
    if map_arr.len() < 4 {
        return None;
    }
    let width = val.get("lensShadingMapWidth").and_then(|v| v.as_u64())? as u32;
    let height = val.get("lensShadingMapHeight").and_then(|v| v.as_u64())? as u32;
    let expected_len = (width * height) as usize;
    let channels: Vec<Vec<f32>> = map_arr.iter().take(4).filter_map(|ch| {
        let arr = ch.as_array()?;
        if arr.len() < expected_len {
            return None;
        }
        Some(arr.iter().filter_map(|v| v.as_f64().map(|x| x as f32)).collect::<Vec<f32>>())
    }).collect();
    if channels.len() < 4 {
        return None;
    }
    Some(LensShadingMap { channels, width, height })
}

fn json_to_dynamic_black_level(val: &Value) -> Option<[f32; 4]> {
    let arr = val.get("dynamicBlackLevel").and_then(|v| v.as_array())?;
    if arr.len() < 4 {
        return None;
    }
    let mut result = [0.0f32; 4];
    for (i, item) in arr.iter().enumerate().take(4) {
        result[i] = item.as_f64()? as f32;
    }
    Some(result)
}

fn json_to_dynamic_white_level(val: &Value) -> Option<f32> {
    val.get("dynamicWhiteLevel").and_then(|v| v.as_f64()).map(|v| v as f32)
}

impl Decoder {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        tracing::debug!("decoder::new: {}", path_str);
        let inner = motioncam_decoder::Decoder::from_path(path)
            .map_err(|e| {
                tracing::error!("decoder failed to open {}: {}", path_str, e);
                anyhow!("Failed to open decoder: {}", e)
            })?;
        tracing::debug!("decoder opened successfully: {}", path_str);
        Ok(Self { inner })
    }

    pub fn container_metadata(&self) -> Result<ContainerMetadata> {
        tracing::debug!("decoder::container_metadata");
        let meta = self.inner.container_metadata();

        let color_matrix1 = json_to_matrix9(meta, "colorMatrix1");
        let color_matrix2 = json_to_matrix9(meta, "colorMatrix2");
        let forward_matrix1 = json_to_matrix9(meta, "forwardMatrix1");
        let forward_matrix2 = json_to_matrix9(meta, "forwardMatrix2");
        let calibration_matrix1 = json_to_matrix9(meta, "calibrationMatrix1");
        let calibration_matrix2 = json_to_matrix9(meta, "calibrationMatrix2");

        let illuminant1 = meta.get("calibrationIlluminant1").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let illuminant2 = meta.get("calibrationIlluminant2").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

        let white_level = meta.get("whiteLevel").and_then(|v| v.as_f64()).unwrap_or(16383.0);

        let (black_level, black_level_count) = json_to_black_level(meta);

        let audio_sample_rate = meta
            .get("extraData")
            .and_then(|e| e.get("audioSampleRate"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;
        let audio_channels = meta
            .get("extraData")
            .and_then(|e| e.get("audioChannels"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;

        let lens_shading_map = json_to_lens_shading_map(meta);

        Ok(ContainerMetadata {
            color_matrix1,
            color_matrix2,
            forward_matrix1,
            forward_matrix2,
            calibration_matrix1,
            calibration_matrix2,
            calibration_illuminant1: illuminant1,
            calibration_illuminant2: illuminant2,
            has_calibration_illuminants: illuminant1 != 0 || illuminant2 != 0,
            white_level,
            black_level,
            black_level_count,
            audio_sample_rate_hz: audio_sample_rate,
            num_audio_channels: audio_channels,
            lens_shading_map,
        })
    }

    pub fn timestamps(&self) -> Result<Vec<i64>> {
        let ts = self.inner.frame_timestamps().collect::<Vec<_>>();
        tracing::debug!("decoder::timestamps: {} frames", ts.len());
        Ok(ts)
    }

    /// Hint the OS to prefetch a frame's range into the page cache (B4).
    /// See `motioncam_decoder::Decoder::prefetch`. No-op on Windows.
    pub fn prefetch(&self, timestamp_ns: i64) {
        self.inner.prefetch(timestamp_ns);
    }

    pub fn load_frame(&self, timestamp_ns: i64) -> Result<(Vec<u16>, FrameMetadata)> {
        let (pixels, meta) = self.inner.load_frame(timestamp_ns)
            .map_err(|e| {
                tracing::error!("failed to decode frame at ns {}: {}", timestamp_ns, e);
                anyhow!("Failed to decode frame at ns {}: {}", timestamp_ns, e)
            })?;

        let width = meta.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let height = meta.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let as_shot_neutral = json_to_as_shot_neutral(&meta);
        let exposure_time = meta.get("exposureTime").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let iso = meta.get("iso").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let focal_length = meta.get("focalLength").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let aperture = meta.get("aperture").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;

        Ok((pixels, FrameMetadata {
            width,
            height,
            timestamp_ns,
            as_shot_neutral,
            exposure_time,
            iso,
            focal_length,
            aperture,
            dynamic_black_level: json_to_dynamic_black_level(&meta),
            dynamic_white_level: json_to_dynamic_white_level(&meta),
            lens_shading_map: json_to_lens_shading_map(&meta),
        }))
    }

    /// Decode a frame into a caller-owned buffer (B1). Returns only the
    /// `asShotNeutral` triplet (B2) — the only metadata the export hot
    /// path uses. Use `load_frame` if you need the other fields.
    pub fn load_frame_into(&self, timestamp_ns: i64, out: &mut [u16]) -> Result<[f32; 3]> {
        self.inner.load_frame_into(timestamp_ns, out)
            .map_err(|e| {
                tracing::error!("failed to decode frame at ns {}: {}", timestamp_ns, e);
                anyhow!("Failed to decode frame at ns {}: {}", timestamp_ns, e)
            })
    }

    pub fn load_frame_metadata(&self, timestamp_ns: i64) -> Result<FrameMetadata> {
        let meta = self.inner.load_frame_metadata(timestamp_ns)
            .map_err(|e| anyhow!("Failed to get metadata for frame at ns {}: {}", timestamp_ns, e))?;

        let width = meta.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let height = meta.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let as_shot_neutral = json_to_as_shot_neutral(&meta);
        let exposure_time = meta.get("exposureTime").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let iso = meta.get("iso").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let focal_length = meta.get("focalLength").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let aperture = meta.get("aperture").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;

        Ok(FrameMetadata {
            width,
            height,
            timestamp_ns,
            as_shot_neutral,
            exposure_time,
            iso,
            focal_length,
            aperture,
            dynamic_black_level: json_to_dynamic_black_level(&meta),
            dynamic_white_level: json_to_dynamic_white_level(&meta),
            lens_shading_map: json_to_lens_shading_map(&meta),
        })
    }

    /// Write all audio chunks to `writer` as raw s16le PCM, one chunk at a
    /// time.  Never holds more than one chunk in memory — safe for long
    /// recordings.  The caller should wrap the file in a `BufWriter` for
    /// decent I/O coalescing.
    pub fn write_audio_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        let chunks = self.inner.load_audio()
            .map_err(|e| anyhow!("Failed to load audio: {}", e))?;
        for chunk in chunks {
            for &sample in &chunk.samples {
                writer.write_all(&sample.to_le_bytes())?;
            }
        }
        Ok(())
    }
}
