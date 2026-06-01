use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct MotionJsonMetadata {
    #[serde(rename = "sensorArrangment", default)]
    sensor_arrangement: Option<String>,
    #[serde(rename = "sensorOrientation", default)]
    sensor_orientation: Option<i64>,
    #[serde(rename = "forwardMatrix1", default)]
    forward_matrix1: Option<Vec<f64>>,
    #[serde(rename = "forwardMatrix2", default)]
    forward_matrix2: Option<Vec<f64>>,
    #[serde(rename = "colorMatrix1", default)]
    color_matrix1: Option<Vec<f64>>,
    #[serde(rename = "colorMatrix2", default)]
    color_matrix2: Option<Vec<f64>>,
    #[serde(rename = "calibrationMatrix1", default)]
    calibration_matrix1: Option<Vec<f64>>,
    #[serde(rename = "calibrationMatrix2", default)]
    calibration_matrix2: Option<Vec<f64>>,
    #[serde(rename = "whiteLevel", default)]
    white_level: Option<f64>,
    #[serde(rename = "blackLevel", default)]
    black_level: Option<Vec<f64>>,
    #[serde(rename = "baselineExposure", default)]
    baseline_exposure: Option<f64>,
    #[serde(rename = "apertures", default)]
    apertures: Option<Vec<f64>>,
    #[serde(rename = "focalLengths", default)]
    focal_lengths: Option<Vec<f64>>,
    #[serde(rename = "uniqueCameraModel", default)]
    unique_camera_model: Option<String>,
    #[serde(rename = "numSegments", default)]
    num_segments: Option<i64>,
    #[serde(rename = "extraData", default)]
    extra_data: Option<ExtraData>,
    #[serde(rename = "deviceSpecificProfile", default)]
    device_specific_profile: Option<DeviceProfile>,
}

#[derive(Debug, Deserialize)]
struct ExtraData {
    #[serde(rename = "recordingType", default)]
    recording_type: Option<String>,
    #[serde(rename = "audioSampleRate", default)]
    audio_sample_rate: Option<i64>,
    #[serde(rename = "audioChannels", default)]
    audio_channels: Option<i64>,
    #[serde(rename = "useAccurateTimestamp", default)]
    use_accurate_timestamp: Option<bool>,
    #[serde(rename = "metadata", default)]
    metadata: Option<BuildMetadata>,
}

#[derive(Debug, Deserialize)]
struct BuildMetadata {
    #[serde(rename = "build.model", default)]
    build_model: Option<String>,
    #[serde(rename = "build.manufacturer", default)]
    build_manufacturer: Option<String>,
    #[serde(rename = "version.major", default)]
    version_major: Option<String>,
    #[serde(rename = "version.build", default)]
    version_build: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeviceProfile {
    #[serde(rename = "cameraId", default)]
    camera_id: Option<String>,
    #[serde(rename = "deviceModel", default)]
    device_model: Option<String>,
}

const INDEX_MAGIC: u32 = 0x8A905612;
const ITEM_TYPE_BUFFER_INDEX: u32 = 0;
const ITEM_TYPE_METADATA: u32 = 3;

#[derive(Debug)]
struct BufferIndex {
    magic: u32,
    num_offsets: u32,
    data_offset: i64,
}

#[derive(Debug)]
struct BufferOffset {
    offset: i64,
    timestamp: i64,
}

/// Bayer filter pattern IDs as defined in the MCRAW spec (Appendix A)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BayerPattern {
    RGGB,
    GRBG,
    GBRG,
    BGGR,
    QuadBayerRGGB,
    QuadBayerGRBG,
    QuadBayerGBRG,
    QuadBayerBGGR,
}

impl BayerPattern {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => BayerPattern::RGGB,
            1 => BayerPattern::GRBG,
            2 => BayerPattern::GBRG,
            3 => BayerPattern::BGGR,
            4 => BayerPattern::QuadBayerRGGB,
            5 => BayerPattern::QuadBayerGRBG,
            6 => BayerPattern::QuadBayerGBRG,
            7 => BayerPattern::QuadBayerBGGR,
            _ => BayerPattern::RGGB,
        }
    }

    pub fn to_u8(&self) -> u8 {
        match self {
            BayerPattern::RGGB => 0,
            BayerPattern::GRBG => 1,
            BayerPattern::GBRG => 2,
            BayerPattern::BGGR => 3,
            BayerPattern::QuadBayerRGGB => 4,
            BayerPattern::QuadBayerGRBG => 5,
            BayerPattern::QuadBayerGBRG => 6,
            BayerPattern::QuadBayerBGGR => 7,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            BayerPattern::RGGB => "RGGB",
            BayerPattern::GRBG => "GRBG",
            BayerPattern::GBRG => "GBRG",
            BayerPattern::BGGR => "BGGR",
            BayerPattern::QuadBayerRGGB => "QuadBayer RGGB",
            BayerPattern::QuadBayerGRBG => "QuadBayer GRBG",
            BayerPattern::QuadBayerGBRG => "QuadBayer GBRG",
            BayerPattern::QuadBayerBGGR => "QuadBayer BGGR",
        }
    }

    /// Dcraw-style `filters` encoding for WGSL shaders.
    /// Maps Bayer pattern to a u32 bitfield R=0, G1=1, G2=3(!), B=2.
    pub fn to_dcraw_filters(&self) -> u32 {
        match self {
            BayerPattern::RGGB => 0x94949494,
            BayerPattern::BGGR => 0x16161616,
            BayerPattern::GRBG => 0x61616161,
            BayerPattern::GBRG => 0x49494949,
            _ => 0x94949494, // QuadBayer patterns fall back to RGGB
        }
    }
}

/// Camera metadata extracted from the MCRAW header block
#[derive(Debug, Clone)]
pub struct CameraMetadata {
    pub sensor_make: Option<String>,
    pub sensor_model: Option<String>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    pub focal_length: Option<f64>,
    pub aperture: Option<f64>,
    pub iso: Option<u32>,
    pub exposure_time: Option<f64>,
    pub white_balance: Option<f64>,
    pub capture_date: Option<String>,
    pub color_matrix: Option<[f64; 9]>,
    pub color_matrix2: Option<[f64; 9]>,
    pub forward_matrix1: Option<[f64; 9]>,
    pub forward_matrix2: Option<[f64; 9]>,
    pub calibration_matrix1: Option<[f64; 9]>,
    pub calibration_matrix2: Option<[f64; 9]>,
    pub calibration_illuminant1: Option<i32>,
    pub calibration_illuminant2: Option<i32>,
    pub calibration_illuminant: Option<String>,
    pub wb_multipliers: Option<[f32; 3]>,
}

impl Default for CameraMetadata {
    fn default() -> Self {
        CameraMetadata {
            sensor_make: None,
            sensor_model: None,
            camera_model: None,
            lens_model: None,
            focal_length: None,
            aperture: None,
            iso: None,
            exposure_time: None,
            white_balance: None,
            capture_date: None,
            color_matrix: None,
            color_matrix2: None,
            forward_matrix1: None,
            forward_matrix2: None,
            calibration_matrix1: None,
            calibration_matrix2: None,
            calibration_illuminant1: None,
            calibration_illuminant2: None,
            calibration_illuminant: None,
            wb_multipliers: None,
        }
    }
}

/// Complete parsed information from an MCRAW file header
#[derive(Debug, Clone)]
pub struct McrawFileInfo {
    pub path: String,
    pub size: u64,
    pub format_version: u32,
    pub frame_count: u32,
    pub width: u16,
    pub height: u16,
    pub fps: f64,
    pub has_audio: bool,
    pub audio_sample_rate: u32,
    pub audio_channels: u16,
    pub bit_depth: u16,
    pub bayer_pattern: BayerPattern,
    pub camera_metadata: CameraMetadata,
    pub frame_offsets: Vec<u64>,
    pub audio_offset: Option<u64>,
    pub audio_length: Option<u64>,
    pub sensor_width: u16,
    pub sensor_height: u16,
    pub active_offset_x: u16,
    pub active_offset_y: u16,
    pub active_width: u16,
    pub active_height: u16,
    pub white_level: f64,
    pub black_level: f64,
}

impl McrawFileInfo {
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        tracing::debug!("McrawFileInfo::from_path: {:?}", path);
        let metadata = fs::metadata(&path)
            .with_context(|| format!("Failed to read metadata for {:?}", path))?;

        let data = fs::read(&path)
            .with_context(|| format!("Failed to read file {:?}", path))?;

        let info = parse_header(&data, path)?;
        Ok(McrawFileInfo {
            path: path.to_string_lossy().into_owned(),
            size: metadata.len(),
            ..info
        })
    }

    pub fn enhance_with_decoder(&mut self) {
        let path = self.path.clone();
        tracing::debug!("enhance_with_decoder: {}", path);
        let decoder_result = crate::decoder::Decoder::new(&path);
        let decoder = match decoder_result {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("failed to open decoder for {}: {}", path, e);
                return;
            }
        };
        if let Ok(container_meta) = decoder.container_metadata() {
            if container_meta.white_level > 0.0 {
                self.white_level = container_meta.white_level;
                tracing::debug!("white_level from container: {}", self.white_level);
            }
            if container_meta.black_level_count > 0 {
                self.black_level = container_meta.black_level[0];
                tracing::debug!("black_level from container: {}", self.black_level);
            }

            let as_f64 = |v: &[f32; 9]| -> [f64; 9] {
                let mut r = [0.0; 9];
                for (i, &x) in v.iter().enumerate() { r[i] = x as f64; }
                r
            };

            self.camera_metadata.color_matrix = Some(as_f64(&container_meta.color_matrix1));
            let non_zero = |m: &[f32; 9]| m.iter().any(|&x| x != 0.0);

            if non_zero(&container_meta.color_matrix2) {
                self.camera_metadata.color_matrix2 = Some(as_f64(&container_meta.color_matrix2));
            }
            if non_zero(&container_meta.forward_matrix1) {
                self.camera_metadata.forward_matrix1 = Some(as_f64(&container_meta.forward_matrix1));
            }
            if non_zero(&container_meta.forward_matrix2) {
                self.camera_metadata.forward_matrix2 = Some(as_f64(&container_meta.forward_matrix2));
            }
            if non_zero(&container_meta.calibration_matrix1) {
                self.camera_metadata.calibration_matrix1 = Some(as_f64(&container_meta.calibration_matrix1));
            }
            if non_zero(&container_meta.calibration_matrix2) {
                self.camera_metadata.calibration_matrix2 = Some(as_f64(&container_meta.calibration_matrix2));
            }
            if container_meta.has_calibration_illuminants {
                self.camera_metadata.calibration_illuminant1 = Some(container_meta.calibration_illuminant1);
                self.camera_metadata.calibration_illuminant2 = Some(container_meta.calibration_illuminant2);
                tracing::debug!("calibration_illuminants: illum1={}, illum2={}",
                    container_meta.calibration_illuminant1, container_meta.calibration_illuminant2);
            }
        }
        if let Ok(timestamps) = decoder.timestamps() {
            if self.frame_count == 0 && !timestamps.is_empty() {
                self.frame_count = timestamps.len() as u32;
                if timestamps.len() >= 2 {
                    let duration_ns = timestamps[timestamps.len() - 1] - timestamps[0];
                    if duration_ns > 0 && self.fps == 0.0 {
                        let duration_in_seconds = duration_ns as f64 / 1_000_000_000.0;
                        self.fps = (self.frame_count.saturating_sub(1)) as f64 / duration_in_seconds;
                    }
                }
                tracing::debug!("enhanced from timestamps: {} frames, {:.2} fps", self.frame_count, self.fps);
            }
            if let Ok(first_frame_meta) = decoder.load_frame_metadata(timestamps[0]) {
                if self.width == 0 || self.height == 0 {
                    self.width = first_frame_meta.width as u16;
                    self.height = first_frame_meta.height as u16;
                    tracing::debug!("enhanced dimensions: {}x{}", first_frame_meta.width, first_frame_meta.height);
                }
                let n = first_frame_meta.as_shot_neutral;
                if n[0] > 1e-6 && n[1] > 1e-6 && n[2] > 1e-6 {
                    let r_gain = n[1] / n[0];
                    let b_gain = n[1] / n[2];
                    self.camera_metadata.wb_multipliers = Some([r_gain, 1.0, b_gain]);
                    tracing::debug!("wb_multipliers: R={:.3} G={:.3} B={:.3}", r_gain, 1.0, b_gain);
                }
            }
        }
    }

    pub fn format_name(&self) -> &'static str {
        match self.format_version {
            1 => "MotionCam v1 (Legacy)",
            2 => "MotionCam v2",
            3 => "MotionCam v3",
            _ => "Unknown format",
        }
    }

    pub fn duration_seconds(&self) -> f64 {
        self.frame_count as f64 / self.fps
    }

    pub fn resolution_label(&self) -> &'static str {
        match (self.width, self.height) {
            (1920, 1080) => "1080p",
            (2560, 1440) => "1440p",
            (3840, 2160) => "4K",
            (4096, 2160) => "4K DCI",
            _ => "Custom",
        }
    }
}

fn parse_motion_header(data: &[u8], path: &Path) -> Result<McrawFileInfo> {
    if data.len() < 17 {
        anyhow::bail!("File {:?} is too small for MOTION header", path);
    }

    let format_version = data[7] as u32;
    tracing::debug!("parse_motion_header: version={} json_len={}", format_version, u32::from_le_bytes([data[12], data[13], data[14], data[15]]));
    let json_len = u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as usize;
    let json_start = 16;
    let json_end = json_start + json_len;

    if json_end > data.len() {
        anyhow::bail!("JSON metadata extends beyond file data");
    }

    let json_str = std::str::from_utf8(&data[json_start..json_end])
        .with_context(|| "Invalid UTF-8 in MOTION JSON metadata")?;

    let json: MotionJsonMetadata = serde_json::from_str(json_str)
        .with_context(|| "Failed to parse MOTION JSON metadata")?;

    let bayer_pattern = match json.sensor_arrangement.as_deref() {
        Some("rggb") | Some("standard") => BayerPattern::RGGB,
        Some("grbg") => BayerPattern::GRBG,
        Some("gbrg") => BayerPattern::GBRG,
        Some("bggr") => BayerPattern::BGGR,
        _ => BayerPattern::RGGB,
    };

    let extra_data = json.extra_data;
    let device_profile = json.device_specific_profile;

    let camera_model_opt: Option<String> = json.unique_camera_model
        .or_else(|| device_profile.as_ref().and_then(|p| p.device_model.clone()));
    let camera_model = camera_model_opt.unwrap_or_default();

    let sensor_make = extra_data
        .as_ref()
        .and_then(|e| e.metadata.as_ref())
        .and_then(|m| m.build_manufacturer.clone())
        .unwrap_or_default();

    let build_model = extra_data
        .as_ref()
        .and_then(|e| e.metadata.as_ref())
        .and_then(|m| m.build_model.clone())
        .unwrap_or(camera_model.clone());

    let aperture = json.apertures.and_then(|mut a| a.pop());
    let focal_length = json.focal_lengths.and_then(|mut a| a.pop());
    let audio_sample_rate = extra_data.as_ref()
        .and_then(|e| e.audio_sample_rate)
        .unwrap_or(0) as u32;
    let audio_channels = extra_data.as_ref()
        .and_then(|e| e.audio_channels)
        .unwrap_or(0) as u16;
    let has_audio = audio_channels > 0;

    let color_matrix = json.color_matrix1.clone().or(json.forward_matrix1.clone())
        .and_then(|m| {
            if m.len() == 9 {
                Some(m.try_into().ok()?)
            } else {
                None
            }
        });

    let color_matrix2 = json.color_matrix2.clone().and_then(|m| {
        if m.len() == 9 { Some(m.try_into().ok()?) } else { None }
    });
    let forward_matrix1 = json.forward_matrix1.clone().and_then(|m| {
        if m.len() == 9 { Some(m.try_into().ok()?) } else { None }
    });
    let forward_matrix2 = json.forward_matrix2.clone().and_then(|m| {
        if m.len() == 9 { Some(m.try_into().ok()?) } else { None }
    });
    let calibration_matrix1 = json.calibration_matrix1.clone().and_then(|m| {
        if m.len() == 9 { Some(m.try_into().ok()?) } else { None }
    });
    let calibration_matrix2 = json.calibration_matrix2.clone().and_then(|m| {
        if m.len() == 9 { Some(m.try_into().ok()?) } else { None }
    });

    let bit_depth = json.white_level
        .map(detect_bit_depth_from_white_level)
        .unwrap_or(12);

    let mut frame_count: u32 = 0;
    let mut width: u16 = 0;
    let mut height: u16 = 0;
    let mut fps: f64 = 0.0;

    if let Some((num_offsets, offsets, timestamps)) = parse_buffer_index(data) {
        frame_count = num_offsets;
        if num_offsets >= 2 {
            let mut sorted_ts = timestamps.clone();
            sorted_ts.sort();
            let duration_ns = sorted_ts[num_offsets as usize - 1] - sorted_ts[0];
            if duration_ns > 0 {
                fps = (num_offsets as f64 - 1.0) / (duration_ns as f64 / 1_000_000_000.0);
            }
        }
    }

    Ok(McrawFileInfo {
        path: path.to_string_lossy().into_owned(),
        size: data.len() as u64,
        format_version,
        frame_count,
        width,
        height,
        fps,
        has_audio,
        audio_sample_rate,
        audio_channels,
        bit_depth,
        bayer_pattern,
        camera_metadata: CameraMetadata {
            sensor_make: if sensor_make.is_empty() { None } else { Some(sensor_make) },
            sensor_model: None,
            camera_model: if build_model.is_empty() { None } else { Some(build_model) },
            lens_model: None,
            focal_length,
            aperture,
            iso: None,
            exposure_time: None,
            white_balance: None,
            capture_date: None,
            color_matrix,
            color_matrix2,
            forward_matrix1,
            forward_matrix2,
            calibration_matrix1,
            calibration_matrix2,
            calibration_illuminant1: None,
            calibration_illuminant2: None,
            calibration_illuminant: None,
            wb_multipliers: None,
        },
        frame_offsets: Vec::new(),
        audio_offset: None,
        audio_length: None,
        sensor_width: 0,
        sensor_height: 0,
        active_offset_x: 0,
        active_offset_y: 0,
        active_width: 0,
        active_height: 0,
        white_level: 16383.0,
        black_level: 0.0,
    })
}

fn parse_buffer_index(data: &[u8]) -> Option<(u32, Vec<i64>, Vec<i64>)> {
    let file_len = data.len();
    if file_len < 8 {
        return None;
    }

    let item_size = u32::from_le_bytes([
        data[file_len - 8],
        data[file_len - 7],
        data[file_len - 6],
        data[file_len - 5],
    ]) as usize;

    let idx_data_start = file_len - 8 - item_size;
    if idx_data_start < 16 {
        return None;
    }

    let buf_idx_type = u32::from_le_bytes([
        data[idx_data_start],
        data[idx_data_start + 1],
        data[idx_data_start + 2],
        data[idx_data_start + 3],
    ]);
    if buf_idx_type != ITEM_TYPE_BUFFER_INDEX {
        return None;
    }

    let buf_idx_size = u32::from_le_bytes([
        data[idx_data_start + 4],
        data[idx_data_start + 5],
        data[idx_data_start + 6],
        data[idx_data_start + 7],
    ]) as usize;

    let magic = u32::from_le_bytes([
        data[idx_data_start + 8],
        data[idx_data_start + 9],
        data[idx_data_start + 10],
        data[idx_data_start + 11],
    ]);
    if magic != INDEX_MAGIC {
        return None;
    }

    let num_offsets = u32::from_le_bytes([
        data[idx_data_start + 12],
        data[idx_data_start + 13],
        data[idx_data_start + 14],
        data[idx_data_start + 15],
    ]);

    let data_offset = i64::from_le_bytes([
        data[idx_data_start + 16],
        data[idx_data_start + 17],
        data[idx_data_start + 18],
        data[idx_data_start + 19],
        data[idx_data_start + 20],
        data[idx_data_start + 21],
        data[idx_data_start + 22],
        data[idx_data_start + 23],
    ]);

    let mut offsets = Vec::new();
    let mut timestamps = Vec::new();

    for i in 0..num_offsets {
        let pos = data_offset as usize + (i as usize) * 16;
        if pos + 16 > data.len() {
            break;
        }
        let offset = i64::from_le_bytes([
            data[pos],
            data[pos + 1],
            data[pos + 2],
            data[pos + 3],
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]);
        let timestamp = i64::from_le_bytes([
            data[pos + 8],
            data[pos + 9],
            data[pos + 10],
            data[pos + 11],
            data[pos + 12],
            data[pos + 13],
            data[pos + 14],
            data[pos + 15],
        ]);
        offsets.push(offset);
        timestamps.push(timestamp);
    }

    Some((num_offsets, offsets, timestamps))
}

fn parse_header(data: &[u8], path: &Path) -> Result<McrawFileInfo> {
    if data.len() < 17 {
        anyhow::bail!("File {:?} is too small to be a valid file (need at least 17 bytes, got {})", path, data.len());
    }

    // Check for "MOTION " magic header (new format with JSON metadata)
    if data.starts_with(b"MOTION ") {
        tracing::debug!("detected MOTION format header");
        return parse_motion_header(data, path);
    }

    // Check for "MCRAW" magic header (legacy binary format)
    if data.len() < 36 {
        anyhow::bail!("File {:?} is too small to be a valid MCRAW file (need at least 36 bytes, got {})", path, data.len());
    }

    let magic = &data[0..5];
    if magic != b"MCRAW" {
        anyhow::bail!(
            "Invalid MCRAW magic header in {:?}: expected 'MCRAW', found {:?}",
            path,
            magic
        );
    }
    tracing::debug!("detected MCRAW legacy format header");

    let format_version = u32::from_be_bytes([data[5], data[6], data[7], data[8]]);
    let frame_count = u32::from_be_bytes([data[9], data[10], data[11], data[12]]);
    let width = u16::from_be_bytes([data[13], data[14]]);
    let height = u16::from_be_bytes([data[15], data[16]]);
    let fps = f64::from_be_bytes([
        data[17], data[18], data[19], data[20], data[21], data[22], data[23], data[24],
    ]);
    let has_audio = data[25] != 0;

    let audio_sample_rate = if has_audio && data.len() >= 30 {
        u32::from_be_bytes([data[26], data[27], data[28], data[29]])
    } else {
        0
    };

    let audio_channels = if data.len() >= 32 {
        u16::from_be_bytes([data[30], data[31]])
    } else {
        0
    };

    let bit_depth = if data.len() >= 34 {
        u16::from_be_bytes([data[32], data[33]])
    } else {
        0
    };

    let bayer_pattern_id = if data.len() >= 35 {
        data[34]
    } else {
        0
    };
    let bayer_pattern = BayerPattern::from_u8(bayer_pattern_id);

    let mut offset = 36;
    let mut camera_metadata = CameraMetadata::default();
    let mut frame_offsets = Vec::new();
    let mut audio_offset: Option<u64> = None;
    let mut audio_length: Option<u64> = None;
    let mut sensor_width: u16 = 0;
    let mut sensor_height: u16 = 0;
    let mut active_offset_x: u16 = 0;
    let mut active_offset_y: u16 = 0;
    let mut active_width: u16 = 0;
    let mut active_height: u16 = 0;
    let mut _color_matrix: Option<[f64; 9]> = None;
    let mut _calibration_illuminant: Option<String> = None;

    if offset < data.len() {
        let block_length = read_u32_be(&data, offset) as usize;
        offset += 4;
        let block_end = offset + block_length;

        while offset < block_end && offset < data.len() {
            let tag = data[offset];
            offset += 1;

            match tag {
                0x01 => {
                    if let Ok(s) = parse_string(&data, &mut offset) {
                        camera_metadata.sensor_make = Some(s);
                    }
                }
                0x02 => {
                    if let Ok(s) = parse_string(&data, &mut offset) {
                        camera_metadata.sensor_model = Some(s);
                    }
                }
                0x03 => {
                    if let Ok(s) = parse_string(&data, &mut offset) {
                        camera_metadata.camera_model = Some(s);
                    }
                }
                0x04 => {
                    if let Ok(s) = parse_string(&data, &mut offset) {
                        camera_metadata.lens_model = Some(s);
                    }
                }
                0x05 => {
                    if let Ok(v) = parse_f64(&data, &mut offset) {
                        camera_metadata.focal_length = Some(v);
                    }
                }
                0x06 => {
                    if let Ok(v) = parse_f64(&data, &mut offset) {
                        camera_metadata.aperture = Some(v);
                    }
                }
                0x07 => {
                    if let Ok(v) = parse_u32_be(&data, &mut offset) {
                        camera_metadata.iso = Some(v);
                    }
                }
                0x08 => {
                    if let Ok(v) = parse_f64(&data, &mut offset) {
                        camera_metadata.exposure_time = Some(v);
                    }
                }
                0x09 => {
                    if let Ok(v) = parse_f64(&data, &mut offset) {
                        camera_metadata.white_balance = Some(v);
                    }
                }
                0x0A => {
                    if let Ok(s) = parse_string(&data, &mut offset) {
                        camera_metadata.capture_date = Some(s);
                    }
                }
                0x0B => {
                    let matrix = parse_f64_array(&data, &mut offset, 9);
                    if matrix.len() == 9 {
                        let arr: [f64; 9] = matrix.try_into().ok().unwrap_or([0.0; 9]);
                          _color_matrix = Some(arr);
                    }
                }
                0x0C => {
                    if let Ok(s) = parse_string(&data, &mut offset) {
                        _calibration_illuminant = Some(s);
                    }
                }
                0x10 => {
                    let count = parse_u32_be(&data, &mut offset);
                    if let Ok(n) = count {
                        let mut offsets = Vec::with_capacity(n as usize);
                        for _ in 0..n {
                            if offset + 8 <= data.len() {
                                let val = u64::from_be_bytes([
                                    data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
                                    data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
                                ]);
                                offsets.push(val);
                                offset += 8;
                            } else {
                                break;
                            }
                        }
                        frame_offsets = offsets;
                    }
                }
                0x11 => {
                    if has_audio && offset + 8 <= data.len() {
                        audio_offset = Some(u64::from_be_bytes([
                            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
                            data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
                        ]));
                        offset += 8;
                    }
                }
                0x12 => {
                    if has_audio && offset + 8 <= data.len() {
                        audio_length = Some(u64::from_be_bytes([
                            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
                            data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
                        ]));
                        offset += 8;
                    }
                }
                0x13 => {
                    if offset + 2 <= data.len() {
                        sensor_width = u16::from_be_bytes([data[offset], data[offset + 1]]);
                        offset += 2;
                    }
                }
                0x14 => {
                    if offset + 2 <= data.len() {
                        sensor_height = u16::from_be_bytes([data[offset], data[offset + 1]]);
                        offset += 2;
                    }
                }
                0x15 => {
                    if offset + 2 <= data.len() {
                        active_offset_x = u16::from_be_bytes([data[offset], data[offset + 1]]);
                        offset += 2;
                    }
                }
                0x16 => {
                    if offset + 2 <= data.len() {
                        active_offset_y = u16::from_be_bytes([data[offset], data[offset + 1]]);
                        offset += 2;
                    }
                }
                0x17 => {
                    if offset + 2 <= data.len() {
                        active_width = u16::from_be_bytes([data[offset], data[offset + 1]]);
                        offset += 2;
                    }
                }
                0x18 => {
                    if offset + 2 <= data.len() {
                        active_height = u16::from_be_bytes([data[offset], data[offset + 1]]);
                        offset += 2;
                    }
                }
                _ => {
                    offset += 1;
                }
            }
        }
    }

    Ok(McrawFileInfo {
        path: path.to_string_lossy().into_owned(),
        size: data.len() as u64,
        format_version,
        frame_count,
        width,
        height,
        fps,
        has_audio,
        audio_sample_rate,
        audio_channels,
        bit_depth,
        bayer_pattern,
        camera_metadata,
        frame_offsets,
        audio_offset,
        audio_length,
        sensor_width,
        sensor_height,
        active_offset_x,
        active_offset_y,
        active_width,
        active_height,
        white_level: 16383.0,
        black_level: 0.0,
    })
}

fn read_u32_be(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
    ])
}

fn parse_u32_be(data: &[u8], offset: &mut usize) -> Result<u32> {
    if *offset + 4 > data.len() {
        return Err(anyhow::anyhow!("Unexpected end of data"));
    }
    let val = u32::from_be_bytes([
        data[*offset], data[*offset + 1], data[*offset + 2], data[*offset + 3],
    ]);
    *offset += 4;
    Ok(val)
}

fn parse_f64(data: &[u8], offset: &mut usize) -> Result<f64> {
    if *offset + 8 > data.len() {
        return Err(anyhow::anyhow!("Unexpected end of data"));
    }
    let val = f64::from_be_bytes([
        data[*offset], data[*offset + 1], data[*offset + 2], data[*offset + 3],
        data[*offset + 4], data[*offset + 5], data[*offset + 6], data[*offset + 7],
    ]);
    *offset += 8;
    Ok(val)
}

fn parse_f64_array(data: &[u8], offset: &mut usize, len: usize) -> Vec<f64> {
    let mut result = Vec::with_capacity(len);
    for _ in 0..len {
        if let Ok(v) = parse_f64(data, offset) {
            result.push(v);
        } else {
            break;
        }
    }
    result
}

fn parse_string(data: &[u8], offset: &mut usize) -> Result<String> {
    if *offset + 4 > data.len() {
        return Err(anyhow::anyhow!("Unexpected end of data"));
    }
    let str_len = u32::from_be_bytes([
        data[*offset], data[*offset + 1], data[*offset + 2], data[*offset + 3],
    ]) as usize;
    *offset += 4;
    if *offset + str_len > data.len() {
        return Err(anyhow::anyhow!("String extends beyond data"));
    }
    let s = std::str::from_utf8(&data[*offset..*offset + str_len])
        .map_err(|e| anyhow::anyhow!("Invalid UTF-8 string: {}", e))?;
    *offset += str_len;
    Ok(s.to_string())
}

pub fn detect_bit_depth_from_white_level(white_level: f64) -> u16 {
    if white_level <= 1024.0 {
        10
    } else if white_level <= 4096.0 {
        12
    } else if white_level <= 16384.0 {
        14
    } else if white_level <= 65536.0 {
        16
    } else {
        12
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bayer_pattern_from_u8() {
        assert_eq!(BayerPattern::from_u8(0), BayerPattern::RGGB);
        assert_eq!(BayerPattern::from_u8(1), BayerPattern::GRBG);
        assert_eq!(BayerPattern::from_u8(2), BayerPattern::GBRG);
        assert_eq!(BayerPattern::from_u8(3), BayerPattern::BGGR);
        assert_eq!(BayerPattern::from_u8(4), BayerPattern::QuadBayerRGGB);
        assert_eq!(BayerPattern::from_u8(5), BayerPattern::QuadBayerGRBG);
        assert_eq!(BayerPattern::from_u8(6), BayerPattern::QuadBayerGBRG);
        assert_eq!(BayerPattern::from_u8(7), BayerPattern::QuadBayerBGGR);
        assert_eq!(BayerPattern::from_u8(99), BayerPattern::RGGB);
    }

    #[test]
    fn test_bayer_pattern_to_u8() {
        assert_eq!(BayerPattern::RGGB.to_u8(), 0);
        assert_eq!(BayerPattern::GRBG.to_u8(), 1);
        assert_eq!(BayerPattern::GBRG.to_u8(), 2);
        assert_eq!(BayerPattern::BGGR.to_u8(), 3);
        assert_eq!(BayerPattern::QuadBayerRGGB.to_u8(), 4);
        assert_eq!(BayerPattern::QuadBayerGRBG.to_u8(), 5);
        assert_eq!(BayerPattern::QuadBayerGBRG.to_u8(), 6);
        assert_eq!(BayerPattern::QuadBayerBGGR.to_u8(), 7);
    }

    #[test]
    fn test_detect_bit_depth_from_white_level() {
        assert_eq!(detect_bit_depth_from_white_level(1023.0), 10);
        assert_eq!(detect_bit_depth_from_white_level(1024.0), 10);
        assert_eq!(detect_bit_depth_from_white_level(1025.0), 12);
        assert_eq!(detect_bit_depth_from_white_level(4095.0), 12);
        assert_eq!(detect_bit_depth_from_white_level(4096.0), 12);
        assert_eq!(detect_bit_depth_from_white_level(4097.0), 14);
        assert_eq!(detect_bit_depth_from_white_level(16383.0), 14);
        assert_eq!(detect_bit_depth_from_white_level(16384.0), 14);
        assert_eq!(detect_bit_depth_from_white_level(16385.0), 16);
        assert_eq!(detect_bit_depth_from_white_level(65535.0), 16);
        assert_eq!(detect_bit_depth_from_white_level(65536.0), 16);
        assert_eq!(detect_bit_depth_from_white_level(65537.0), 12);
        assert_eq!(detect_bit_depth_from_white_level(0.0), 10);
    }

    #[test]
    fn test_parse_header_minimal() {
        let mut data = vec![0u8; 36];
        data[0..5].copy_from_slice(b"MCRAW");
        data[5..9].copy_from_slice(&2u32.to_be_bytes());
        data[9..13].copy_from_slice(&10u32.to_be_bytes());
        data[13..15].copy_from_slice(&(1920u16).to_be_bytes());
        data[15..17].copy_from_slice(&(1080u16).to_be_bytes());
        data[17..25].copy_from_slice(&(30.0f64).to_be_bytes());
        data[25] = 0;

        let info = parse_header(&data, std::path::Path::new("test.mcraw")).unwrap();
        assert_eq!(info.format_version, 2);
        assert_eq!(info.frame_count, 10);
        assert_eq!(info.width, 1920);
        assert_eq!(info.height, 1080);
        assert!((info.fps - 30.0).abs() < 0.001);
        assert!(!info.has_audio);
    }

    #[test]
    fn test_duration_seconds() {
        let mut data = vec![0u8; 36];
        data[0..5].copy_from_slice(b"MCRAW");
        data[9..13].copy_from_slice(&600u32.to_be_bytes());
        data[17..25].copy_from_slice(&(30.0f64).to_be_bytes());
        let info = parse_header(&data, std::path::Path::new("test.mcraw")).unwrap();
        assert!((info.duration_seconds() - 20.0).abs() < 0.001);
    }

    #[test]
    fn test_resolution_label() {
        let make_info = |w: u16, h: u16| McrawFileInfo {
            path: String::new(),
            size: 0,
            format_version: 2,
            frame_count: 0,
            width: w,
            height: h,
            fps: 30.0,
            has_audio: false,
            audio_sample_rate: 0,
            audio_channels: 0,
            bit_depth: 0,
            bayer_pattern: BayerPattern::RGGB,
            camera_metadata: CameraMetadata::default(),
            frame_offsets: Vec::new(),
            audio_offset: None,
            audio_length: None,
            sensor_width: 0,
            sensor_height: 0,
            active_offset_x: 0,
            active_offset_y: 0,
            active_width: 0,
            active_height: 0,
            white_level: 16383.0,
            black_level: 0.0,
        };

        assert_eq!(make_info(1920, 1080).resolution_label(), "1080p");
        assert_eq!(make_info(2560, 1440).resolution_label(), "1440p");
        assert_eq!(make_info(3840, 2160).resolution_label(), "4K");
        assert_eq!(make_info(4096, 2160).resolution_label(), "4K DCI");
        assert_eq!(make_info(1280, 720).resolution_label(), "Custom");
    }

    #[test]
    fn test_parse_header_with_string_metadata() {
        let mut data = vec![0u8; 64];
        data[0..5].copy_from_slice(b"MCRAW");
        data[5] = 2;
        data[9..13].copy_from_slice(&1u32.to_be_bytes());
        data[13..15].copy_from_slice(&(1920u16).to_be_bytes());
        data[15..17].copy_from_slice(&(1080u16).to_be_bytes());
        data[17..25].copy_from_slice(&(30.0f64).to_be_bytes());
        data[25] = 0;

        let camera_model = "TestCamera";
        let block_offset = 36;
        let str_len = camera_model.len() as u32;
        let block_len = 1 + 4 + str_len as u32; // 1 tag + 4 str_len + str data
        data[block_offset..block_offset + 4].copy_from_slice(&(block_len as u32).to_be_bytes());
        data[block_offset + 4] = 0x03;
        data[block_offset + 5..block_offset + 9].copy_from_slice(&str_len.to_be_bytes());
        data[block_offset + 9..block_offset + 9 + camera_model.len()]
            .copy_from_slice(camera_model.as_bytes());

        let info = parse_header(&data, std::path::Path::new("test.mcraw")).unwrap();
        assert_eq!(info.camera_metadata.camera_model, Some("TestCamera".to_string()));
    }

    #[test]
    fn test_parse_header_invalid_magic() {
        let data = vec![b'X'; 36];
        let result = parse_header(&data, std::path::Path::new("test.mcraw"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_header_too_small() {
        let data = vec![0u8; 10];
        let result = parse_header(&data, std::path::Path::new("test.mcraw"));
        assert!(result.is_err());
    }
}
