use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
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
    #[serde(rename = "colorIlluminant1", default)]
    color_illuminant1: Option<String>,
    #[serde(rename = "colorIlluminant2", default)]
    color_illuminant2: Option<String>,
    #[serde(rename = "lensShadingMap", default)]
    lens_shading_map: Option<Vec<Vec<f64>>>,
    #[serde(rename = "lensShadingMapWidth", default)]
    lens_shading_map_width: Option<i64>,
    #[serde(rename = "lensShadingMapHeight", default)]
    lens_shading_map_height: Option<i64>,
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
    pub black_level_per_channel: [f64; 4],
    pub black_level_count: i32,
    pub lens_shading_map: Option<crate::decoder::LensShadingMap>,
    pub dynamic_black_level: Option<[f32; 4]>,
    pub dynamic_white_level: Option<f32>,
    /// First frame timestamp from BufferIndex (None if not available without decoder).
    pub first_timestamp: Option<i64>,
}

/// Data extracted from frame 0's JSON metadata header via bounded file read.
struct FirstFrameMeta {
    width: u16,
    height: u16,
    /// White balance gains [R, G, B] computed from asShotNeutral: G/R, 1.0, G/B.
    wb_gains: Option<[f32; 3]>,
}

/// Read width, height and white balance gains from frame 0's JSON metadata.
fn read_first_frame_meta(file: &mut fs::File, frame0_offset: u64) -> Option<FirstFrameMeta> {
    file.seek(SeekFrom::Start(frame0_offset)).ok()?;
    let mut hdr = [0u8; 8];
    file.read_exact(&mut hdr).ok()?;
    let buf_type = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    let buf_size = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
    if buf_type != 2 {
        return None;
    }
    file.seek(SeekFrom::Current(buf_size as i64)).ok()?;
    file.read_exact(&mut hdr).ok()?;
    let meta_type = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    let meta_size = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
    if meta_type != 3 {
        return None;
    }
    let mut json_buf = vec![0u8; meta_size as usize];
    file.read_exact(&mut json_buf).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&json_buf).ok()?;
    let w = json.get("width")?.as_u64()? as u16;
    let h = json.get("height")?.as_u64()? as u16;
    if w == 0 || h == 0 {
        return None;
    }
    let wb_gains = json.get("asShotNeutral").and_then(|v| v.as_array()).and_then(|arr| {
        if arr.len() >= 3 {
            let r = arr[0].as_f64()?;
            let g = arr[1].as_f64()?;
            let b = arr[2].as_f64()?;
            if r > 1e-6 && g > 1e-6 && b > 1e-6 {
                Some([(g / r) as f32, 1.0, (g / b) as f32])
            } else {
                None
            }
        } else {
            None
        }
    });
    tracing::debug!("read_first_frame_meta: w={} h={} wb_gains={:?}", w, h, wb_gains);
    Some(FirstFrameMeta { width: w, height: h, wb_gains })
}

impl McrawFileInfo {
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        tracing::debug!("McrawFileInfo::from_path: {:?}", path);
        let file_meta = fs::metadata(&path)
            .with_context(|| format!("Failed to read metadata for {:?}", path))?;
        let file_size = file_meta.len();

        let mut file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open {:?}", path))?;

        let mut magic_buf = [0u8; 16];
        file.read_exact(&mut magic_buf)
            .with_context(|| format!("Failed to read header from {:?}", path))?;

        let info = if magic_buf.starts_with(b"MOTION ") {
            let json_len = u32::from_le_bytes([
                magic_buf[12], magic_buf[13], magic_buf[14], magic_buf[15],
            ]) as usize;

            let mut json_buf = vec![0u8; json_len];
            file.read_exact(&mut json_buf)
                .with_context(|| format!("Failed to read MOTION JSON block from {:?}", path))?;

            let mut data = Vec::with_capacity(16 + json_len);
            data.extend_from_slice(&magic_buf);
            data.extend_from_slice(&json_buf);

            let mut info = parse_motion_header(&data, path)?;

            // Read BufferIndex from end-24: Item(8) + BufferIndex(16: magic, num_offsets, index_data_offset)
            if file_size >= 24 {
                let mut end_buf = [0u8; 24];
                file.seek(SeekFrom::End(-24))
                    .with_context(|| format!("Failed to seek to BufferIndex in {:?}", path))?;
                file.read_exact(&mut end_buf)
                    .with_context(|| format!("Failed to read BufferIndex from {:?}", path))?;

                let idx_magic = u32::from_le_bytes([end_buf[8], end_buf[9], end_buf[10], end_buf[11]]);
                if idx_magic == INDEX_MAGIC {
                    let num_offsets = u32::from_le_bytes([end_buf[12], end_buf[13], end_buf[14], end_buf[15]]);
                    let idx_data_offset = i64::from_le_bytes([
                        end_buf[16], end_buf[17], end_buf[18], end_buf[19],
                        end_buf[20], end_buf[21], end_buf[22], end_buf[23],
                    ]) as u64;

                    info.frame_count = num_offsets;

                    if num_offsets > 0 && idx_data_offset + (num_offsets as u64 * 16) <= file_size {
                        let mut offset_buf = vec![0u8; num_offsets as usize * 16];
                        file.seek(SeekFrom::Start(idx_data_offset))
                            .with_context(|| format!("Failed to seek to offset data in {:?}", path))?;
                        file.read_exact(&mut offset_buf)
                            .with_context(|| format!("Failed to read offset data from {:?}", path))?;

                        let mut first_frame_offset: u64 = 0;
                        let mut timestamps = Vec::with_capacity(num_offsets as usize);
                        for i in 0..num_offsets as usize {
                            let off = i64::from_le_bytes([
                                offset_buf[i*16], offset_buf[i*16+1], offset_buf[i*16+2], offset_buf[i*16+3],
                                offset_buf[i*16+4], offset_buf[i*16+5], offset_buf[i*16+6], offset_buf[i*16+7],
                            ]);
                            let ts = i64::from_le_bytes([
                                offset_buf[i*16+8], offset_buf[i*16+9], offset_buf[i*16+10], offset_buf[i*16+11],
                                offset_buf[i*16+12], offset_buf[i*16+13], offset_buf[i*16+14], offset_buf[i*16+15],
                            ]);
                            if i == 0 { first_frame_offset = off as u64; }
                            timestamps.push(ts);
                        }

                        // Sort timestamps to compute fps and first_timestamp
                        timestamps.sort();
                        info.first_timestamp = Some(timestamps[0]);
                        if num_offsets >= 2 {
                            let duration_ns = timestamps[num_offsets as usize - 1] - timestamps[0];
                            if duration_ns > 0 {
                                info.fps = (num_offsets as f64 - 1.0) / (duration_ns as f64 / 1_000_000_000.0);
                            }
                        }

                        // Read width/height + wb gains from frame 0's JSON metadata (bounded read, 1-2KB)
                        if first_frame_offset > 0 {
                            if let Some(meta) = read_first_frame_meta(&mut file, first_frame_offset) {
                                info.width = meta.width;
                                info.height = meta.height;
                                if let Some(wb) = meta.wb_gains {
                                    info.camera_metadata.wb_multipliers = Some(wb);
                                }
                            } else {
                                tracing::debug!("from_path: read_first_frame_meta returned None for offset={}", first_frame_offset);
                            }
                        }
                    }
                }
            }

            info
        } else if &magic_buf[..5] == b"MCRAW" {
            let mut rest_header = [0u8; 20];
            file.read_exact(&mut rest_header)
                .with_context(|| format!("Failed to read legacy header from {:?}", path))?;

            let mut data = Vec::with_capacity(36);
            data.extend_from_slice(&magic_buf);
            data.extend_from_slice(&rest_header);

            if file_size > 36 {
                let mut block_len_buf = [0u8; 4];
                file.read_exact(&mut block_len_buf)
                    .with_context(|| format!("Failed to read TLV block length from {:?}", path))?;
                let block_length = u32::from_be_bytes(block_len_buf) as usize;

                let mut tlv_buf = vec![0u8; block_length];
                file.read_exact(&mut tlv_buf)
                    .with_context(|| format!("Failed to read TLV block from {:?}", path))?;

                data.extend_from_slice(&block_len_buf);
                data.extend_from_slice(&tlv_buf);
            }

            parse_header(&data, path)?
        } else {
            anyhow::bail!(
                "Invalid MCRAW magic header in {:?}: expected 'MCRAW' or 'MOTION ', got {:?}",
                path,
                &magic_buf[..7]
            );
        };

        Ok(McrawFileInfo {
            path: path.to_string_lossy().into_owned(),
            size: file_size,
            ..info
        })
    }

    /// Skip Decoder creation when all essential metadata is already populated.
    pub fn is_metadata_complete(&self) -> bool {
        self.width > 0 && self.height > 0 && self.frame_count > 0
            && self.first_timestamp.is_some()
            && self.camera_metadata.wb_multipliers.is_some()
    }

    pub fn enhance_from_decoder(&mut self, decoder: &crate::decoder::Decoder) {
        // Container-level metadata (skip if already populated from JSON parse)
        if self.camera_metadata.color_matrix.is_none() {
            if let Ok(container_meta) = decoder.container_metadata() {
                if container_meta.white_level > 0.0 {
                    self.white_level = container_meta.white_level;
                    tracing::debug!("white_level from container: {}", self.white_level);
                }
                if container_meta.black_level_count > 0 {
                    self.black_level = container_meta.black_level[0];
                    self.black_level_per_channel = container_meta.black_level;
                    self.black_level_count = container_meta.black_level_count;
                    tracing::debug!("black_level from container: {} ({} ch)", self.black_level, self.black_level_count);
                }
                self.lens_shading_map = container_meta.lens_shading_map.clone();

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
        }
        // Timestamps and first-frame data (always run, in-memory from mmap)
        if let Ok(timestamps) = decoder.timestamps() {
            if !timestamps.is_empty() {
                self.frame_count = timestamps.len() as u32;
                if timestamps.len() >= 2 {
                    let duration_ns = timestamps[timestamps.len() - 1] - timestamps[0];
                    if duration_ns > 0 {
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
                if self.camera_metadata.wb_multipliers.is_none()
                    && n[0] > 1e-6 && n[1] > 1e-6 && n[2] > 1e-6
                {
                    let r_gain = n[1] / n[0];
                    let b_gain = n[1] / n[2];
                    self.camera_metadata.wb_multipliers = Some([r_gain, 1.0, b_gain]);
                    tracing::debug!("wb_multipliers: R={:.3} G={:.3} B={:.3}", r_gain, 1.0, b_gain);
                }
                if first_frame_meta.dynamic_black_level.is_some() {
                    self.dynamic_black_level = first_frame_meta.dynamic_black_level;
                    tracing::debug!("dynamic_black_level from first frame: {:?}", self.dynamic_black_level);
                }
                if first_frame_meta.dynamic_white_level.is_some() {
                    self.dynamic_white_level = first_frame_meta.dynamic_white_level;
                    tracing::debug!("dynamic_white_level from first frame: {:?}", self.dynamic_white_level);
                }
                // Fallback: if we still don't have a lens shading map (e.g. it was
                // not in the JSON header or the container-metadata block was
                // skipped because color_matrix was already populated), read it
                // from the first frame's per-frame metadata.
                if self.lens_shading_map.is_none() {
                    if let Some(ref lsm) = first_frame_meta.lens_shading_map {
                        self.lens_shading_map = Some(crate::decoder::LensShadingMap {
                            channels: lsm.channels.clone(),
                            width: lsm.width,
                            height: lsm.height,
                        });
                        tracing::info!("lens_shading_map from first frame: {}x{}", lsm.width, lsm.height);
                    }
                }
            }
        }
    }

    pub fn enhance_with_decoder(&mut self) {
        if self.camera_metadata.color_matrix.is_some() {
            tracing::debug!("enhance_with_decoder: metadata already populated, skipping decoder");
            return;
        }
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
        self.enhance_from_decoder(&decoder);
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

    let build_model = extra_data
        .as_ref()
        .and_then(|e| e.metadata.as_ref())
        .and_then(|m| m.build_model.clone());

    let camera_model: Option<String> = device_profile.as_ref()
        .and_then(|p| p.device_model.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| json.unique_camera_model.filter(|s| !s.is_empty()))
        .or_else(|| build_model.filter(|s| !s.is_empty()));

    let sensor_make = extra_data
        .as_ref()
        .and_then(|e| e.metadata.as_ref())
        .and_then(|m| m.build_manufacturer.clone())
        .unwrap_or_default();

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

    let frame_count: u32 = 0;
    let width: u16 = 0;
    let height: u16 = 0;
    let fps: f64 = 0.0;

    let (black_level, black_level_per_channel, black_level_count) = json.black_level
        .as_ref()
        .map(|levels| {
            let count = levels.len() as i32;
            let avg = if levels.is_empty() { 0.0 } else { levels.iter().sum::<f64>() / levels.len() as f64 };
            let mut per_ch = [avg; 4];
            for (i, &v) in levels.iter().enumerate().take(4) {
                per_ch[i] = v;
            }
            (avg, per_ch, count)
        })
        .unwrap_or((0.0, [0.0; 4], 0));

    let white_level = json.white_level.unwrap_or(16383.0);

    // asShotNeutral is per-frame metadata only, not in container JSON.
    // It's extracted in from_path via read_first_frame_meta from frame 0's
    // per-frame JSON header, so this stays None here.
    let wb_multipliers: Option<[f32; 3]> = None;


    // Map JSON string illuminant names to DNG illuminant constants
    let json_illuminant_to_const = |s: &str| -> Option<i32> {
        match s.trim().to_lowercase().as_str() {
            "d50" | "horizon" | "cool_white" => Some(23),
            "d55" => Some(22),
            "d65" | "daylight" | "fine_weather" | "cloudy" => Some(21),
            "d75" | "shade" => Some(24),
            "standardlighta" | "standard_a" | "tungsten" | "incandescent" | "warm_white" | "iso_studio_tungsten" => Some(17),
            "fluorescent" | "tl84" => Some(12),
            "flash" | "standardlightb" => Some(4),
            _ => None,
        }
    };

    let calibration_illuminant1 = json.color_illuminant1.as_deref().and_then(json_illuminant_to_const);
    let calibration_illuminant2 = json.color_illuminant2.as_deref().and_then(json_illuminant_to_const);

    let lens_shading_map = json.lens_shading_map.as_ref().and_then(|channels| {
        let width = json.lens_shading_map_width? as u32;
        let height = json.lens_shading_map_height? as u32;
        if channels.len() < 4 { return None; }
        let f32_channels: Vec<Vec<f32>> = channels.iter().take(4).map(|ch| ch.iter().map(|&v| v as f32).collect()).collect();
        if f32_channels.len() < 4 { return None; }
        Some(crate::decoder::LensShadingMap { channels: f32_channels, width, height })
    });

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
            camera_model,
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
            calibration_illuminant1,
            calibration_illuminant2,
            calibration_illuminant: None,
            wb_multipliers,
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
        white_level,
        black_level,
        black_level_per_channel,
        black_level_count,
        lens_shading_map,
        dynamic_black_level: None,
        dynamic_white_level: None,
        first_timestamp: None,
    })
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
        black_level_per_channel: [0.0; 4],
        black_level_count: 0,
        lens_shading_map: None,
        dynamic_black_level: None,
        dynamic_white_level: None,
        first_timestamp: None,
    })
}

/// Read a big-endian u32 from a byte slice at the given offset.
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

    fn make_test_info(w: u16, h: u16) -> McrawFileInfo {
        McrawFileInfo {
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
            black_level_per_channel: [0.0; 4],
            black_level_count: 0,
            lens_shading_map: None,
            dynamic_black_level: None,
            dynamic_white_level: None,
            first_timestamp: None,
        }
    }

    #[test]
    fn test_resolution_label() {
        assert_eq!(make_test_info(1920, 1080).resolution_label(), "1080p");
        assert_eq!(make_test_info(2560, 1440).resolution_label(), "1440p");
        assert_eq!(make_test_info(3840, 2160).resolution_label(), "4K");
        assert_eq!(make_test_info(4096, 2160).resolution_label(), "4K DCI");
        assert_eq!(make_test_info(1280, 720).resolution_label(), "Custom");
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
