use anyhow::{anyhow, Result};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::ptr;

#[repr(C)]
pub struct McDecoder {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct McContainerMetadata {
    pub width: u32,
    pub height: u32,
    pub white_level: f64,
    pub black_level: [f64; 4],
    pub black_level_count: c_int,
    pub sensor_arrangement: [c_char; 8],
    pub color_matrix1: [f32; 9],
    pub color_matrix2: [f32; 9],
    pub forward_matrix1: [f32; 9],
    pub forward_matrix2: [f32; 9],
    pub calibration_illuminant1: i32,
    pub calibration_illuminant2: i32,
    pub calibration_matrix1: [f32; 9],
    pub calibration_matrix2: [f32; 9],
    pub has_calibration_illuminants: bool,
    pub audio_sample_rate_hz: c_int,
    pub num_audio_channels: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct McFrameMetadata {
    pub width: u32,
    pub height: u32,
    pub timestamp_ns: i64,
    pub as_shot_neutral: [f32; 3],
    pub exposure_time: f64,
    pub iso: f32,
    pub focal_length: f32,
    pub aperture: f32,
}

extern "C" {
    fn decoder_create(path: *const c_char) -> *mut McDecoder;
    fn decoder_destroy(decoder: *mut McDecoder);
    fn decoder_get_container_metadata(decoder: *mut McDecoder, out: *mut McContainerMetadata) -> c_int;
    fn decoder_get_frame_count(decoder: *mut McDecoder) -> i64;
    fn decoder_get_frame_timestamps(decoder: *mut McDecoder, out_timestamps: *mut i64, capacity: i64) -> i64;
    fn decoder_load_frame(
        decoder: *mut McDecoder,
        timestamp_ns: i64,
        out_size: *mut u32,
        out_meta: *mut McFrameMetadata,
    ) -> *mut u8;
    fn decoder_load_frame_metadata(
        decoder: *mut McDecoder,
        timestamp_ns: i64,
        out_meta: *mut McFrameMetadata,
    ) -> c_int;
    fn decoder_load_audio(decoder: *mut McDecoder, out_sample_count: *mut u32) -> *mut i16;
    fn decoder_free_buffer(ptr: *mut c_void);
    fn decoder_last_error() -> *const c_char;
}

fn get_last_error() -> String {
    unsafe {
        let err_ptr = decoder_last_error();
        if err_ptr.is_null() {
            "Unknown C++ error".to_string()
        } else {
            CStr::from_ptr(err_ptr).to_string_lossy().into_owned()
        }
    }
}

pub struct Decoder {
    handle: *mut McDecoder,
}

// Safe: Decoder is used from only one thread at a time (loader stage).
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_str = path.as_ref().to_string_lossy();
        let c_path = CString::new(path_str.as_bytes())?;
        let handle = unsafe { decoder_create(c_path.as_ptr()) };
        if handle.is_null() {
            return Err(anyhow!("Failed to open decoder: {}", get_last_error()));
        }
        Ok(Self { handle })
    }

    pub fn container_metadata(&self) -> Result<McContainerMetadata> {
        let mut meta = McContainerMetadata::default();
        let res = unsafe { decoder_get_container_metadata(self.handle, &mut meta) };
        if res != 0 {
            return Err(anyhow!("Failed to read container metadata: {}", get_last_error()));
        }
        Ok(meta)
    }

    pub fn timestamps(&self) -> Result<Vec<i64>> {
        let count = unsafe { decoder_get_frame_count(self.handle) };
        if count < 0 {
            return Err(anyhow!("Failed to get frame count: {}", get_last_error()));
        }
        let mut timestamps = vec![0i64; count as usize];
        let written = unsafe {
            decoder_get_frame_timestamps(self.handle, timestamps.as_mut_ptr(), count)
        };
        if written < 0 {
            return Err(anyhow!("Failed to get timestamps: {}", get_last_error()));
        }
        timestamps.truncate(written as usize);
        Ok(timestamps)
    }

    pub fn load_frame(&self, timestamp_ns: i64) -> Result<(Vec<u8>, McFrameMetadata)> {
        let mut size: u32 = 0;
        let mut meta = McFrameMetadata::default();
        let ptr = unsafe { decoder_load_frame(self.handle, timestamp_ns, &mut size, &mut meta) };
        if ptr.is_null() {
            return Err(anyhow!("Failed to decode frame at ns {}: {}", timestamp_ns, get_last_error()));
        }
        let data = unsafe { std::slice::from_raw_parts(ptr, size as usize).to_vec() };
        unsafe { decoder_free_buffer(ptr as *mut c_void) };
        Ok((data, meta))
    }

    pub fn load_frame_metadata(&self, timestamp_ns: i64) -> Result<McFrameMetadata> {
        let mut meta = McFrameMetadata::default();
        let res = unsafe { decoder_load_frame_metadata(self.handle, timestamp_ns, &mut meta) };
        if res != 0 {
            return Err(anyhow!("Failed to get metadata for frame at ns {}: {}", timestamp_ns, get_last_error()));
        }
        Ok(meta)
    }

    pub fn load_audio(&self) -> Result<Vec<i16>> {
        let mut sample_count: u32 = 0;
        let ptr = unsafe { decoder_load_audio(self.handle, &mut sample_count) };
        if ptr.is_null() {
            return Err(anyhow!("Failed to load audio: {}", get_last_error()));
        }
        let audio = unsafe { std::slice::from_raw_parts(ptr, sample_count as usize).to_vec() };
        unsafe { decoder_free_buffer(ptr as *mut c_void) };
        Ok(audio)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { decoder_destroy(self.handle) };
        }
    }
}