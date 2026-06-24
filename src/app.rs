use anyhow::Result;
use crossterm::{
    cursor::MoveTo,
    terminal::{disable_raw_mode, enable_raw_mode, window_size, EnterAlternateScreen, LeaveAlternateScreen},
    event::{Event, KeyEventKind, EnableBracketedPaste, DisableBracketedPaste, EnableMouseCapture, DisableMouseCapture},
    QueueableCommand,
};
use std::io::Write;
use percent_encoding::percent_decode_str;
use ratatui::backend::CrosstermBackend;
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tokio::time;

use crate::cli::{Cli, CliCommands, ResolvedCli};
use crate::color::{build_preview_ccm, ColorSpace, TransferFunction};
use crate::export::{
    Av1Profile, CodecFamily, DnxhrProfile, H264Profile, HevcProfile,
    ProResProfile, RateControl, Vp9Profile,
};
use crate::hardware::probe_hardware;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::decoder::Decoder;
use crate::encoder::{EncodeJob, EncodeStatus, Encoder, OutputFormat};
use crate::file::McrawFileInfo;
use crate::file_browser::FileBrowser;
use crate::preset::ExportPreset;
use crate::preview::pipeline::{GpuPreviewPipeline, PreviewParams, PreviewGpuContext, Ready};
use crate::preview::PreviewState;
use crate::preview::pipeline::params::{transfer_to_u32, color_space_to_u32, bayer_phase_to_u32};
use crate::stats::PipelineStats;
use crate::thumbnail::ThumbnailCache;
use crate::thumbnail_worker::{ThumbnailWorkerPool, ThumbnailRequest};

/// Braille spinner frames for the rendering indicator (500ms cycle at 50ms/tick).
pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
use crate::ui::{self, ClickAction};

// ---------------------------------------------------------------------------
// Data types for the media pool / queue workflow
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ImportedFile {
    pub path: String,
    pub info: McrawFileInfo,
    pub selected: bool,
    pub first_timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueStatus {
    Waiting,
    Rendering,
    Completed,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct QueuedFile {
    pub path: String,
    pub info: McrawFileInfo,
    pub selected: bool,
    pub status: QueueStatus,
    pub progress: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    MediaPool,
    Queue,
    ExportSettings,
    Grade,
}

#[derive(Debug, Clone, Copy)]
pub struct GradeSliders {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub shadows: f32,
    pub highlights: f32,
    pub temperature: f32,
    pub tint: f32,
    pub sharpen: f32,
}

impl GradeSliders {
    pub fn name(index: usize) -> &'static str {
        match index {
            0 => "Exposure",
            1 => "Contrast",
            2 => "Saturation",
            3 => "Shadows",
            4 => "Highlights",
            5 => "Temp",
            6 => "Tint",
            7 => "Sharpen",
            _ => "",
        }
    }

    pub fn default_val(index: usize) -> f32 {
        match index {
            0 => 0.0,
            1 => 1.0,
            2 => 1.0,
            3 => 0.0,
            4 => 0.0,
            5 => 5200.0,
            6 => 0.0,
            7 => 0.0,
            _ => 0.0,
        }
    }

    pub fn min(index: usize) -> f32 {
        match index {
            0 => -5.0,
            1 => 0.0,
            2 => 0.0,
            3 => -1.0,
            4 => -1.0,
            5 => 2000.0,
            6 => -100.0,
            7 => 0.0,
            _ => 0.0,
        }
    }

    pub fn max(index: usize) -> f32 {
        match index {
            0 => 5.0,
            1 => 2.0,
            2 => 2.0,
            3 => 1.0,
            4 => 1.0,
            5 => 10000.0,
            6 => 100.0,
            7 => 1.0,
            _ => 1.0,
        }
    }

    pub fn step_small(index: usize) -> f32 {
        match index {
            0 => 0.1,
            5 => 50.0,
            6 => 1.0,
            _ => 0.05,
        }
    }

    pub fn step_large(index: usize) -> f32 {
        match index {
            0 => 1.0,
            5 => 500.0,
            6 => 10.0,
            _ => 0.25,
        }
    }

    pub fn value(&self, index: usize) -> f32 {
        match index {
            0 => self.exposure,
            1 => self.contrast,
            2 => self.saturation,
            3 => self.shadows,
            4 => self.highlights,
            5 => self.temperature,
            6 => self.tint,
            7 => self.sharpen,
            _ => 0.0,
        }
    }

    pub fn normalized(&self, index: usize) -> f32 {
        let v = self.value(index);
        let lo = Self::min(index);
        let hi = Self::max(index);
        if hi <= lo { return 0.5; }
        ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
    }

    pub fn display_value(&self, index: usize) -> String {
        let sign = |x: f32| if x >= 0.0 { "+" } else { "" };
        match index {
            0 => format!("{}{:.1} stops", sign(self.exposure), self.exposure),
            1 => format!("{:.2}x", self.contrast),
            2 => format!("{:.2}x", self.saturation),
            3 => format!("{}{:.2}", sign(self.shadows), self.shadows),
            4 => format!("{}{:.2}", sign(self.highlights), self.highlights),
            5 => format!("{:.0}K", self.temperature),
            6 => format!("{}{:.0}", sign(self.tint), self.tint),
            _ => format!("{:.2}", self.sharpen),
        }
    }

    pub fn set(&mut self, index: usize, v: f32) {
        let lo = Self::min(index);
        let hi = Self::max(index);
        let v = v.clamp(lo, hi);
        match index {
            0 => self.exposure = v,
            1 => self.contrast = v,
            2 => self.saturation = v,
            3 => self.shadows = v,
            4 => self.highlights = v,
            5 => self.temperature = v,
            6 => self.tint = v,
            7 => self.sharpen = v,
            _ => {}
        }
    }

    pub fn apply_delta(&mut self, index: usize, step: f32) {
        let cur = self.value(index);
        self.set(index, cur + step);
    }

    pub fn count() -> usize { 8 }
}

impl Default for GradeSliders {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            shadows: 0.0,
            highlights: 0.0,
            temperature: 5200.0,
            tint: 0.0,
            sharpen: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportPopupState {
    Hidden,
    DroppedFiles {
        files: Vec<String>,
        folder: String,
        all_in_folder: Vec<String>,
    },
}

#[derive(Debug)]
pub enum ExportEvent {
    Progress(f64),
    Stats(Arc<PipelineStats>),
    Done(Result<()>),
}

/// Snapshot of the most recently finished export. Kept so the UI can show a
/// post-render summary (codec, settings, elapsed time, output path, etc.)
/// instead of immediately reverting to the preview panel.
#[derive(Debug, Clone)]
pub struct ExportSummary {
    pub output_path: String,
    pub codec_label: String,
    pub profile_label: String,
    pub color_space: String,
    pub transfer: String,
    pub rate_control: String,
    pub frame_count: usize,
    pub elapsed: Duration,
    pub result: Result<(), String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Browse,
    Info,
    Export,
}

/// Tracks real render-loop frame rate using a simple per-second counter.
///
/// Updates once per second with EMA smoothing (`0.9 * prev + 0.1 * current`)
/// to dampen visual jitter. The value is exposed via `fps()` and rendered
/// right-aligned in the header bar.
#[derive(Debug, Clone)]
pub struct FPSCounter {
    last_draw: Instant,
    frames_this_second: u32,
    second_start: Instant,
    smooth_fps: f64,
}

impl FPSCounter {
    pub fn new() -> Self {
        Self {
            last_draw: Instant::now(),
            frames_this_second: 0,
            second_start: Instant::now(),
            smooth_fps: 0.0,
        }
    }

    /// Call once per frame before measuring elapsed time.
    pub fn tick(&mut self) {
        let now = Instant::now();
        self.frames_this_second += 1;
        if now.duration_since(self.second_start).as_secs_f64() >= 1.0 {
            let fps = self.frames_this_second as f64;
            if self.smooth_fps == 0.0 {
                self.smooth_fps = fps;
            } else {
                self.smooth_fps = self.smooth_fps * 0.9 + fps * 0.1;
            }
            self.frames_this_second = 0;
            self.second_start = now;
        }
        self.last_draw = now;
    }

    pub fn fps(&self) -> f64 {
        self.smooth_fps
    }
}

pub struct App {
    pub running: bool,
    pub screen: Screen,
    pub file_path: Option<String>,
    pub file_info: Option<McrawFileInfo>,
    pub frame_index: usize,
    pub frame_count: usize,
    pub encode_jobs: Vec<EncodeJob>,
    pub status_message: String,
    pub show_help: bool,
    pub error: Option<String>,
    pub browser: FileBrowser,

    pub is_exporting: bool,
    pub export_cancelled: bool,
    pub export_progress: f64,
    pub export_rx: Option<mpsc::Receiver<ExportEvent>>,
    pub cancel_token: Option<Arc<AtomicBool>>,

    /// Snapshot of the most-recent finished export — drives the post-render
    /// summary panel. Cleared when the user starts a new export.
    pub last_export_summary: Option<ExportSummary>,

    /// Settings captured at `start_export` time so `poll_export` can build
    /// an accurate `ExportSummary` even if the user has since cycled the
    /// export-settings panel to different values.
    pub pending_export_summary: Option<ExportSummary>,

    // Which queue item is currently being rendered (for sequential batch)
    pub current_rendering_index: Option<usize>,

    // Export folder for the current session
    pub export_folder: Option<std::path::PathBuf>,

    // Favourite folders for quick browser navigation
    pub favourite_folders: Vec<std::path::PathBuf>,

    // Help overlay scroll position
    pub help_scroll: u16,

    // Culling mode flag
    pub show_culling: bool,

    // Full-screen grade mode (Shift+G)
    pub show_grade_screen: bool,

    // Persistent export settings
    pub export_color_space: ColorSpace,
    pub export_transfer_function: TransferFunction,
    pub export_codec_family: CodecFamily,
    pub export_focus: ExportFocus,
    pub export_fps: Option<f64>,
    pub export_start_time: Option<Instant>,

    // Sticky per-codec profiles
    pub prores_profile: ProResProfile,
    pub dnxhr_profile: DnxhrProfile,
    pub hevc_profile: HevcProfile,
    pub h264_profile: H264Profile,
    pub av1_profile: Av1Profile,
    pub vp9_profile: Vp9Profile,

    // Runtime hardware probe result
    pub hardware_caps: crate::hardware::HardwareCaps,

    // Rate control
    pub active_rate_control: RateControl,
    pub is_editing_custom_rate: bool,

    // Grading sliders (Phase 2)
    pub grade_sliders: GradeSliders,
    pub grade_focus: usize,
    /// Active mouse drag on a grade slider: (slider_index, track_x, track_width)
    pub grade_dragging: Option<(usize, u16, u16)>,

    // Media pool / queue workflow
    pub imported_files: Vec<ImportedFile>,
    pub media_pool_index: usize,

    pub queue: Vec<QueuedFile>,
    pub queue_index: usize,

    pub show_browser: bool,
    pub import_popup: ImportPopupState,

    pub focus_target: FocusTarget,

    pub show_full_info: bool,

    // Browser double-click detection
    pub last_browser_click: Option<(Instant, usize)>,

    // Grade slider double-click detection
    pub last_grade_click: Option<(Instant, usize)>,

    // Drag-drop visual feedback
    pub drop_highlight: Option<Instant>,

    // Async drag-drop import state
    pub drop_import_rx: Option<mpsc::Receiver<DropImportEvent>>,
    pub drop_import_cancel: Option<Arc<AtomicBool>>,

    // Drop preview overlay for visual feedback
    pub drop_preview: Option<DropPreview>,

    // Persistent ListState offset for browser (prevents viewport jumping on click)
    pub browser_scroll_offset: Cell<usize>,

    // Pinned favourites bar toggle
    pub show_favourites_bar: bool,

    // When true, the browser list is replaced by a flat view of the
    // user's favourite folders (f-key toggle). `..` is hidden in this
    // view because the favourites list isn't a filesystem hierarchy.
    pub browsing_favourites: bool,

    // Persistent ListState offset for the favourites list view
    pub favourites_scroll_offset: Cell<usize>,

    // Timestamp + index of last clicked favourite (for d-key removal)
    pub last_clicked_favourite: Option<(Instant, usize)>,

    // -------------------------------------------------------------------
    // Export presets
    // -------------------------------------------------------------------
    /// User-saved export setting bundles. Loaded from
    /// `presets.json` at startup, written back on every change.
    pub presets: Vec<crate::preset::ExportPreset>,

    /// Name of the preset that was last applied, if any. Shown in the
    /// Export Settings panel header so the user can see *why* the current
    /// settings look the way they do.
    pub active_preset: Option<String>,

    /// State of the preset-picker overlay.
    pub preset_picker: PresetPickerState,

    /// True while the user is typing a name for a new preset. Captures
    /// the live text and the cursor position. Esc cancels, Enter saves.
    pub preset_naming: Option<PresetNamingState>,

    // Preview pipeline state (Phase 1)
    pub decoder: Option<Decoder>,
    pub timestamps: Vec<i64>,
    pub preview_state: PreviewState,
    pub preview_pipeline: Option<GpuPreviewPipeline<Ready>>,
    pub preview_gpu_context: Option<Arc<PreviewGpuContext>>,
    pub thumbnail_cache: ThumbnailCache,
    pub pending_preview_ts: Option<i64>,
    /// Background worker pool for async thumbnail generation.
    pub thumbnail_worker: Option<ThumbnailWorkerPool>,
    /// (Path, timestamp) of the last thumbnail submitted to the worker — used
    /// for deduplication so rapid navigation doesn't flood the workers.
    pub thumbnail_requested: Option<(PathBuf, i64)>,

    // Sixel write-back state for Ghost Widget pattern
    pub sixel_pending: Cell<bool>,
    pub sixel_write_pos: Cell<Option<(u16, u16)>>,
    /// Character-cell footprint of the last written sixel (x, y, chars_w, chars_h).
    pub sixel_occupy_size: Cell<Option<(u16, u16, u16, u16)>>,
    /// Index of the media pool item whose sixel was last written to the terminal.
    pub last_written_media_index: Cell<Option<usize>>,
    /// Terminal character cell size in pixels — updated at each loop iteration.
    pub term_cell_size: Cell<(f32, f32)>,
    /// Character dimensions of the thumbnail panel area (cols, rows).
    pub preview_panel_chars: Cell<Option<(u16, u16)>>,
    /// Set to true by render_preview_panel when panel chars change from the
    /// pre-computed estimate to the real layout value, triggering a re-generation.
    pub needs_rethumbnail: Cell<bool>,

    // Animation state
    pub spinner_frame: u8,
    pub progress_anim_offset: u8,

    // Real-time render-loop performance meter
    pub fps_counter: FPSCounter,

    // Heatwave shockwave countdown (0 = inactive)
    pub shockwave_ticks_remaining: u8,

    // Focus strip state — whether the single-line HUD is in expanded slider view
    pub grade_strip_active: bool,
    // Parameter morph animation: (old_index, ticks_remaining)
    pub grade_morph: Option<(usize, u8)>,
    // Phosphor trail: (track_position 0..1, ticks_remaining)
    pub phosphor_trail: Vec<(f32, u8)>,
    // Snapshot for before/after comparison (B key)
    pub grade_before_snapshot: Option<GradeSliders>,
    // Focus strip idle counter: decrements each tick
    pub grade_strip_idle_ticks: u8,

}

/// Overlay state for the preset-picker. `Shown` holds the list, cursor
/// index, and a transient error/info string rendered at the bottom.
#[derive(Debug, Clone, Default)]
pub struct PresetPickerState {
    pub open: bool,
    pub index: usize,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PresetNamingState {
    pub name: String,
    pub message: Option<String>,
}

/// Event from async drag-drop import worker
pub enum DropImportEvent {
    FileReady { path: String, info: McrawFileInfo, first_timestamp: i64 },
    Failed { path: String, error: String },
    Complete { imported: usize, failed: usize },
}

/// Visual preview of dropped files
pub struct DropPreview {
    pub files: Vec<String>,
    pub start_time: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFocus {
    ColorSpace,
    TransferFunction,
    CodecFamily,
    Profile,
    RateControl,
    Fps,
}

impl App {
    fn favourites_file() -> Option<PathBuf> {
        let mut dir = dirs::config_dir()?;
        dir.push("mcraw-tui");
        std::fs::create_dir_all(&dir).ok()?;
        dir.push("favourites.json");
        Some(dir)
    }

    fn load_favourites() -> Vec<PathBuf> {
        let path = match Self::favourites_file() {
            Some(p) => p,
            None => return Vec::new(),
        };
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };
        serde_json::from_str(&data).unwrap_or_default()
    }

    fn save_favourites(&self) {
        let path = match Self::favourites_file() {
            Some(p) => p,
            None => return,
        };
        if let Ok(data) = serde_json::to_string(&self.favourite_folders) {
            let _ = std::fs::write(path, data);
        }
    }

    pub fn new_with_placeholder(placeholder_path: Option<PathBuf>) -> Self {
        let caps = probe_hardware();
        App {
            running: true,
            screen: Screen::Browse,
            file_path: None,
            file_info: None,
            frame_index: 0,
            frame_count: 0,
            encode_jobs: Vec::new(),
            status_message: String::from("Ready | Drag-drop .mcraw files or press b to browse"),
            show_help: false,
            error: None,
            browser: FileBrowser::new(),

            is_exporting: false,
            export_cancelled: false,
            export_progress: 0.0,
            export_rx: None,
            cancel_token: None,
            last_export_summary: None,
            pending_export_summary: None,

            export_color_space: ColorSpace::Rec709,
            export_transfer_function: TransferFunction::Gamma24,
            export_codec_family: CodecFamily::HEVC,
            export_focus: ExportFocus::CodecFamily,
            export_fps: None,
            export_start_time: None,

            prores_profile: ProResProfile::HQ,
            dnxhr_profile: DnxhrProfile::HQX,
            hevc_profile: HevcProfile::Main10_420,
            h264_profile: H264Profile::Main8bit,
            av1_profile: Av1Profile::Profile0_420_10bit,
            vp9_profile: Vp9Profile::Profile2_420_10bit,

            hardware_caps: caps,
            active_rate_control: RateControl::Lossless,
            is_editing_custom_rate: false,

            imported_files: Vec::new(),
            media_pool_index: 0,
            queue: Vec::new(),
            queue_index: 0,
            show_browser: true,
            current_rendering_index: None,
            export_folder: None,
            favourite_folders: Self::load_favourites(),
            help_scroll: 0,
            show_culling: false,
            show_grade_screen: false,
            import_popup: ImportPopupState::Hidden,
            focus_target: FocusTarget::MediaPool,
            show_full_info: false,
            last_browser_click: None,
            last_grade_click: None,
            drop_highlight: None,
            drop_import_rx: None,
            drop_import_cancel: None,
            drop_preview: None,
            browser_scroll_offset: Cell::new(0),
            show_favourites_bar: true,
            last_clicked_favourite: None,
            browsing_favourites: false,
            favourites_scroll_offset: Cell::new(0),
            presets: ExportPreset::load_all(),
            active_preset: None,
            preset_picker: PresetPickerState::default(),
            preset_naming: None,

            spinner_frame: 0,
            progress_anim_offset: 0,
            decoder: None,
            timestamps: Vec::new(),
            preview_state: PreviewState::Empty,
            preview_pipeline: None,
            preview_gpu_context: None,
            thumbnail_cache: ThumbnailCache::new_with_placeholder(placeholder_path.as_deref()),
            pending_preview_ts: None,
            thumbnail_worker: Some(ThumbnailWorkerPool::new(2)),
            thumbnail_requested: None,
            sixel_pending: Cell::new(false),
            sixel_write_pos: Cell::new(None),
            sixel_occupy_size: Cell::new(None),
            last_written_media_index: Cell::new(None),
            term_cell_size: Cell::new((10.0, 20.0)),
            preview_panel_chars: Cell::new(None),
            needs_rethumbnail: Cell::new(false),
            fps_counter: FPSCounter::new(),
            shockwave_ticks_remaining: 0,
            grade_sliders: GradeSliders::default(),
            grade_focus: 0,
            grade_dragging: None,
            grade_strip_active: true,
            grade_morph: None,
            phosphor_trail: Vec::new(),
            grade_before_snapshot: None,
            grade_strip_idle_ticks: 0,
        }
    }

    /// Create App with bundled placeholder (no custom sixel file).
    pub fn new() -> Self {
        Self::new_with_placeholder(None)
    }

    // -----------------------------------------------------------------------
    // File loading
    // -----------------------------------------------------------------------

    pub fn load_file(&mut self, path: String) {
        tracing::info!("load_file: path={}", path);
        self.error = None;
        self.status_message = String::new();
        match McrawFileInfo::from_path(&path) {
            Ok(mut info) => {
                tracing::debug!("file parsed: frames={} {}x{} fps={}", info.frame_count, info.width, info.height, info.fps);
                let (decoder, timestamps) = match Decoder::new(&path) {
                    Ok(decoder) => {
                        let ts = decoder.timestamps().unwrap_or_default();
                        (Some(decoder), ts)
                    }
                    Err(e) => {
                        tracing::warn!("decoder init failed (OK for non-RAW): {}", e);
                        (None, Vec::new())
                    }
                };

                    if let Some(ref decoder) = decoder {
                        if let Ok(container_meta) = decoder.container_metadata() {
                            let as_f64 = |v: &[f32; 9]| -> [f64; 9] {
                                let mut r = [0.0; 9];
                                for (i, &x) in v.iter().enumerate() { r[i] = x as f64; }
                                r
                            };
                            let non_zero = |m: &[f32; 9]| m.iter().any(|&x| x != 0.0);

                            info.camera_metadata.color_matrix = Some(as_f64(&container_meta.color_matrix1));
                            if non_zero(&container_meta.color_matrix2) {
                                info.camera_metadata.color_matrix2 = Some(as_f64(&container_meta.color_matrix2));
                            }
                            if non_zero(&container_meta.forward_matrix1) {
                                info.camera_metadata.forward_matrix1 = Some(as_f64(&container_meta.forward_matrix1));
                            }
                            if non_zero(&container_meta.forward_matrix2) {
                                info.camera_metadata.forward_matrix2 = Some(as_f64(&container_meta.forward_matrix2));
                            }
                            if container_meta.has_calibration_illuminants {
                                info.camera_metadata.calibration_illuminant1 = Some(container_meta.calibration_illuminant1);
                                info.camera_metadata.calibration_illuminant2 = Some(container_meta.calibration_illuminant2);
                            }

                            if container_meta.white_level > 0.0 {
                                info.white_level = container_meta.white_level;
                            }
                            if container_meta.black_level_count > 0 {
                                info.black_level = container_meta.black_level[0];
                            }
                        }
                        info.frame_count = timestamps.len() as u32;
                        if timestamps.len() >= 2 {
                            let duration_ns = timestamps[timestamps.len() - 1] - timestamps[0];
                            if duration_ns > 0 {
                                let duration_in_seconds = duration_ns as f64 / 1_000_000_000.0;
                                info.fps = (info.frame_count.saturating_sub(1)) as f64 / duration_in_seconds;
                            }
                        }
                        if !timestamps.is_empty() {
                            if let Ok(first_frame_meta) = decoder.load_frame_metadata(timestamps[0]) {
                                info.width = first_frame_meta.width as u16;
                                info.height = first_frame_meta.height as u16;
                            }
                            // Initialize grade temperature from file white balance
                            if let Some(wb) = info.camera_metadata.wb_multipliers {
                                let r_gain = wb[0];
                                let b_gain = wb[2];
                                let ratio = (r_gain / b_gain.max(1e-6)).clamp(0.1, 10.0);
                                let temp = if ratio >= 1.0 {
                                    5200.0 + (ratio - 1.0) * 3000.0
                                } else {
                                    5200.0 - (1.0 - ratio) * 3000.0
                                };
                                self.grade_sliders.set(5, temp.clamp(2000.0, 10000.0));
                            } else {
                                self.grade_sliders.set(5, 5200.0);
                            }
                        }
                    }

                // Store decoder + timestamps for on-demand preview frame decode
                self.decoder = decoder;
                self.timestamps = timestamps;

                // Reset preview state for new file
                self.preview_state = PreviewState::Empty;
                self.pending_preview_ts = None;

                // Init GPU preview pipeline (lazy — reuses on next file)
                if self.preview_pipeline.is_none() {
                    if let Ok(context) = PreviewGpuContext::new() {
                        let ctx_arc = Arc::new(context);
                        match GpuPreviewPipeline::new().init(ctx_arc.clone()) {
                            Ok(pipeline) => {
                                self.preview_pipeline = Some(pipeline);
                                self.preview_gpu_context = Some(ctx_arc);
                            }
                            Err(e) => {
                                tracing::warn!("GPU preview pipeline init failed: {}", e);
                                self.preview_state = PreviewState::Error(format!("GPU: {}", e));
                            }
                        }
                    } else {
                        tracing::warn!("No GPU adapter found — preview disabled");
                        self.preview_state = PreviewState::Error("No GPU available".into());
                    }
                }

                self.file_info = Some(info.clone());
                self.frame_count = info.frame_count as usize;
                self.file_path = Some(path.clone());

                let already_pos = self.imported_files.iter().position(|f| f.path == path);
                if let Some(pos) = already_pos {
                    self.media_pool_index = pos;
                    tracing::debug!("file already in media pool at index={}, switching to it", pos);
                } else {
                        self.imported_files.push(ImportedFile {
                            path: path.clone(),
                            info: info.clone(),
                            selected: true,
                            first_timestamp: self.timestamps.first().copied().unwrap_or(0),
                        });
                    self.media_pool_index = self.imported_files.len() - 1;
                    tracing::info!("file added to media pool: index={}", self.media_pool_index);
                }

                self.status_message = format!("Imported: {}", path);
                tracing::info!("file loaded successfully: {}", path);

                // Reset Ghost Widget index so it writes the new thumbnail
                self.last_written_media_index.set(None);

                // Auto-request preview for the first frame
                if self.decoder.is_some() && !self.timestamps.is_empty() {
                    self.frame_index = 0;
                    self.request_frame_decode(0);
                }
            }
            Err(e) => {
                tracing::error!("failed to load file {}: {}", path, e);
                self.error = Some(format!("Failed to load file: {}", e));
                self.status_message = format!("Error: {}", e);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Preview pipeline (Phase 1)
    // -----------------------------------------------------------------------

    /// Request a decode + GPU process for the given frame_index.
    /// The actual work happens in `poll_preview()` on the next tick.
    pub fn request_frame_decode(&mut self, new_index: usize) {
        if new_index >= self.timestamps.len() {
            self.preview_state = PreviewState::Empty;
            self.pending_preview_ts = None;
            return;
        }
        let ts = self.timestamps[new_index];
        self.preview_state = PreviewState::Loading { started: Instant::now() };
        self.pending_preview_ts = Some(ts);
    }

    /// Poll the thumbnail worker for results and submit pending requests.
    /// Called every tick in the main loop. Never blocks.
    pub fn poll_thumbnail(&mut self) {
        // 1. Drain completed results from worker threads — zero blocking
        if let Some(ref worker) = self.thumbnail_worker {
            while let Ok(result) = worker.result_rx.try_recv() {
                // Cache every completed thumbnail
                if let Some(cached) = result.to_cached() {
                    self.thumbnail_cache.insert(result.path.clone(), cached);
                }
                // Update preview state only if this result is for the current file
                let is_current = self.file_path.as_ref().map_or(false, |fp| *fp == *result.path.to_string_lossy());
                if is_current {
                    if let Some(sixel) = result.sixel {
                        self.sixel_pending.set(true);
                        self.preview_state = PreviewState::Ready {
                            sixel,
                            width: result.width,
                            height: result.height,
                        };
                    } else {
                        let msg = result.error.unwrap_or_else(|| "Unknown error".into());
                        self.preview_state = PreviewState::Error(msg);
                    }
                }
            }
        }

        // 2. Check if there is a pending frame to generate
        let ts = match self.pending_preview_ts.take() {
            Some(ts) => ts,
            None => return,
        };

        // 3. Check cache — skip worker if already cached (unless panel size
        //    changed, requiring re-generation at the new dimensions).
        let path_buf = match self.file_path.as_ref() {
            Some(p) => PathBuf::from(p),
            None => {
                self.preview_state = PreviewState::Empty;
                return;
            }
        };
        let needs_regen = self.needs_rethumbnail.get();
        if !needs_regen {
            if let Some(cached) = self.thumbnail_cache.get(&path_buf) {
                self.sixel_pending.set(true);
                self.preview_state = PreviewState::Ready {
                    sixel: cached.sixel,
                    width: cached.width,
                    height: cached.height,
                };
                return;
            }
        }

        // 4. Deduplicate: don't re-submit if we already sent the same (path, ts)
        if !needs_regen && self.thumbnail_requested.as_ref() == Some(&(path_buf.clone(), ts)) {
            return;
        }

        // 4a. Defer until panel dimensions are known from an actual render.
        //     Without this, the first thumbnail would use 320x180 fallback.
        if self.preview_panel_chars.get().is_none() {
            self.pending_preview_ts = Some(ts);  // restore so next tick retries
            return;
        }

        // 4b. Timeout check: if preview has been Loading for >5s, show error
        if let PreviewState::Loading { started } = &self.preview_state {
            if started.elapsed() > Duration::from_secs(5) {
                self.preview_state = PreviewState::Error("Timed out".into());
                return;
            }
        }

        // 5. Build params on the main thread (fast — no I/O)
        let frame_meta_width;
        let frame_meta_height;
        let (cm_f32, bayer_phase, bl, wl) = match self.file_info.as_ref() {
            Some(info) => {
                let cm = build_preview_ccm(
                    info.camera_metadata.color_matrix.as_ref(),
                    info.camera_metadata.forward_matrix1.as_ref(),
                    info.camera_metadata.forward_matrix2.as_ref(),
                    info.camera_metadata.color_matrix2.as_ref(),
                    info.camera_metadata.calibration_matrix1.as_ref(),
                );
                frame_meta_width = info.width as u32;
                frame_meta_height = info.height as u32;
                let bp = bayer_phase_to_u32(&info.bayer_pattern);
                let bl = info.black_level as f32;
                let wl = if info.white_level > 0.0 { info.white_level as f32 } else { 4095.0 };
                (cm, bp, bl, wl)
            }
            None => {
                // Without file info, we can't build proper params
                self.preview_state = PreviewState::Empty;
                return;
            }
        };

        let (target_w, target_h) = match self.preview_panel_chars.get() {
            Some((panel_cols, panel_rows)) => {
                let (cell_w, cell_h) = self.term_cell_size.get();
                let avail_px_w = (panel_cols as f32 * cell_w).ceil() as u32;
                let avail_px_h = (panel_rows as f32 * cell_h).ceil() as u32;
                (avail_px_w.max(16), avail_px_h.max(16))
            }
            None => (crate::thumbnail::THUMBNAIL_WIDTH, crate::thumbnail::THUMBNAIL_HEIGHT),
        };

        let params = self.build_preview_params(&cm_f32, bayer_phase, bl, wl,
            frame_meta_width, frame_meta_height, target_w, target_h);

        // 6. Submit to worker — non-blocking
        if let Some(ref worker) = self.thumbnail_worker {
            worker.submit(ThumbnailRequest {
                path: path_buf.clone(),
                timestamp_ns: ts,
                params,
            });
            self.thumbnail_requested = Some((path_buf, ts));
            self.preview_state = PreviewState::Loading { started: Instant::now() };
        }
        self.needs_rethumbnail.set(false);
    }

    /// Build PreviewParams from current grade sliders + file metadata + target dimensions.
    fn build_preview_params(
        &self,
        ccm: &[f32; 9],
        bayer_phase: u32,
        black_level: f32,
        white_level: f32,
        raw_width: u32,
        raw_height: u32,
        target_w: u32,
        target_h: u32,
    ) -> PreviewParams {
        // Aspect-ratio-fit target dimensions
        let bayer_aspect = raw_width as f64 / raw_height as f64;
        let target_aspect = target_w as f64 / target_h as f64;

        let (width, height) = if bayer_aspect > target_aspect {
            let h = (target_w as f64 / bayer_aspect) as u32;
            (target_w, h.max(1))
        } else {
            let w = (target_h as f64 * bayer_aspect) as u32;
            (w.max(1), target_h)
        };

        // White balance gains from grade sliders combined with as-shot neutral
        let as_shot = self.file_info.as_ref()
            .and_then(|info| info.camera_metadata.wb_multipliers)
            .unwrap_or([1.0, 1.0, 1.0]);

        // Temperature/tint adjustment: modulate the as-shot neutral gains
        let temp_offset = self.grade_sliders.temperature - 5200.0;
        let tint_offset = self.grade_sliders.tint;
        let wb_gain_r = as_shot[0] * (1.0 + temp_offset / 10000.0);
        let wb_gain_g = as_shot[1];
        let wb_gain_b = as_shot[2] * (1.0 - temp_offset / 10000.0 + tint_offset / 100.0);

        // Exposure: send stops directly — shader applies exp2(stops)
        let exposure_stops = self.grade_sliders.exposure;

        // Determine if any non-default grade is active
        let adjust_enabled = (self.grade_sliders.exposure.abs() > 0.01
            || (self.grade_sliders.contrast - 1.0).abs() > 0.01
            || (self.grade_sliders.saturation - 1.0).abs() > 0.01
            || self.grade_sliders.shadows.abs() > 0.01
            || self.grade_sliders.highlights.abs() > 0.01
            || (self.grade_sliders.temperature - 5200.0).abs() > 50.0
            || self.grade_sliders.tint.abs() > 0.5) as u32;

        PreviewParams {
            width,
            height,
            bayer_width: raw_width,
            bayer_height: raw_height,
            black_level,
            white_level,
            exposure: exposure_stops,
            wb_r: wb_gain_r,
            wb_g: wb_gain_g,
            wb_b: wb_gain_b,
            contrast: self.grade_sliders.contrast,
            saturation: self.grade_sliders.saturation,
            shadows: self.grade_sliders.shadows,
            highlights: self.grade_sliders.highlights,
            _align0: 0.0,
            _align1: 0.0,
            ccm_row0: [ccm[0], ccm[1], ccm[2], 0.0],
            ccm_row1: [ccm[3], ccm[4], ccm[5], 0.0],
            ccm_row2: [ccm[6], ccm[7], ccm[8], 0.0],
            color_space: color_space_to_u32(&ColorSpace::Rec709),
            transfer: transfer_to_u32(&TransferFunction::Gamma24),
            adjust_enabled,
            bayer_phase,
            compute_histogram: 0,
            _pad0: 0, _pad1: 0, _pad2: 0, _pad3: 0, _pad4: 0, _pad5: 0, _pad6: 0,
        }
    }

    /// Submit a thumbnail generation request to the background worker for
    /// any file in the media pool, identified by path + timestamp.
    fn request_thumbnail_for(&self, path: &str, timestamp_ns: i64) {
        let worker = match self.thumbnail_worker.as_ref() {
            Some(w) => w,
            None => return,
        };
        let imported = match self.imported_files.iter().find(|f| f.path == path) {
            Some(f) => f,
            None => return,
        };
        let cm = build_preview_ccm(
            imported.info.camera_metadata.color_matrix.as_ref(),
            imported.info.camera_metadata.forward_matrix1.as_ref(),
            imported.info.camera_metadata.forward_matrix2.as_ref(),
            imported.info.camera_metadata.color_matrix2.as_ref(),
            imported.info.camera_metadata.calibration_matrix1.as_ref(),
        );
        let bp = bayer_phase_to_u32(&imported.info.bayer_pattern);
        let bl = imported.info.black_level as f32;
        let wl = if imported.info.white_level > 0.0 { imported.info.white_level as f32 } else { 4095.0 };
        let (target_w, target_h) = match self.preview_panel_chars.get() {
            Some((pc, pr)) => {
                let (cw, ch) = self.term_cell_size.get();
                ((pc as f32 * cw).ceil() as u32, (pr as f32 * ch).ceil() as u32)
            }
            None => (crate::thumbnail::THUMBNAIL_WIDTH, crate::thumbnail::THUMBNAIL_HEIGHT),
        };
        let params = self.build_preview_params(&cm, bp, bl, wl,
            imported.info.width as u32, imported.info.height as u32, target_w, target_h);
        worker.submit(ThumbnailRequest {
            path: PathBuf::from(path),
            timestamp_ns,
            params,
        });
    }

    /// Bilinear RGBA (u8) resize — scales src to dst dimensions. Returns a new Vec.
    fn resize_rgba(&self, src: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u8> {
        if src_w == dst_w && src_h == dst_h {
            return src.to_vec();
        }
        let mut dst = vec![0u8; (dst_w * dst_h * 4) as usize];
        for y in 0..dst_h {
            let src_y = y as f32 * src_h as f32 / dst_h as f32;
            let y0 = (src_y.floor() as u32).min(src_h.saturating_sub(1));
            let y1 = (y0 + 1).min(src_h.saturating_sub(1));
            let fy = src_y - y0 as f32;
            for x in 0..dst_w {
                let src_x = x as f32 * src_w as f32 / dst_w as f32;
                let x0 = (src_x.floor() as u32).min(src_w.saturating_sub(1));
                let x1 = (x0 + 1).min(src_w.saturating_sub(1));
                let fx = src_x - x0 as f32;
                let idx00 = ((y0 * src_w + x0) * 4) as usize;
                let idx01 = ((y0 * src_w + x1) * 4) as usize;
                let idx10 = ((y1 * src_w + x0) * 4) as usize;
                let idx11 = ((y1 * src_w + x1) * 4) as usize;
                let didx = ((y * dst_w + x) * 4) as usize;
                for c in 0..4 {
                    let v00 = src[idx00 + c] as f32;
                    let v01 = src[idx01 + c] as f32;
                    let v10 = src[idx10 + c] as f32;
                    let v11 = src[idx11 + c] as f32;
                    let v0 = v00 + (v01 - v00) * fx;
                    let v1 = v10 + (v11 - v10) * fx;
                    dst[didx + c] = (v0 + (v1 - v0) * fy).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        dst
    }

    /// Add multiple files to the media pool (used by drag-drop).
    /// Returns (imported_count, failed_count).
    pub fn load_files_batch(&mut self, paths: &[String]) -> (usize, usize) {
        tracing::info!("load_files_batch: count={}", paths.len());
        let mut imported = 0;
        let mut failed = 0;
        for path in paths {
            self.error = None;
            match McrawFileInfo::from_path(path) {
                Ok(mut info) => {
                    let mut first_ts = 0i64;
                    if let Ok(decoder) = Decoder::new(path) {
                        if let Ok(container_meta) = decoder.container_metadata() {
                            let as_f64 = |v: &[f32; 9]| -> [f64; 9] {
                                let mut r = [0.0; 9];
                                for (i, &x) in v.iter().enumerate() { r[i] = x as f64; }
                                r
                            };
                            let non_zero = |m: &[f32; 9]| m.iter().any(|&x| x != 0.0);
                            info.camera_metadata.color_matrix = Some(as_f64(&container_meta.color_matrix1));
                            if non_zero(&container_meta.color_matrix2) {
                                info.camera_metadata.color_matrix2 = Some(as_f64(&container_meta.color_matrix2));
                            }
                            if non_zero(&container_meta.forward_matrix1) {
                                info.camera_metadata.forward_matrix1 = Some(as_f64(&container_meta.forward_matrix1));
                            }
                            if non_zero(&container_meta.forward_matrix2) {
                                info.camera_metadata.forward_matrix2 = Some(as_f64(&container_meta.forward_matrix2));
                            }
                            if container_meta.has_calibration_illuminants {
                                info.camera_metadata.calibration_illuminant1 = Some(container_meta.calibration_illuminant1);
                                info.camera_metadata.calibration_illuminant2 = Some(container_meta.calibration_illuminant2);
                            }
                            if container_meta.white_level > 0.0 {
                                info.white_level = container_meta.white_level;
                            }
                            if container_meta.black_level_count > 0 {
                                info.black_level = container_meta.black_level[0];
                            }
                        }
                        if let Ok(timestamps) = decoder.timestamps() {
                            first_ts = timestamps.first().copied().unwrap_or(0);
                            info.frame_count = timestamps.len() as u32;
                            if timestamps.len() >= 2 {
                                let duration_ns = timestamps[timestamps.len() - 1] - timestamps[0];
                                if duration_ns > 0 {
                                    let duration_in_seconds = duration_ns as f64 / 1_000_000_000.0;
                                    info.fps = (info.frame_count.saturating_sub(1)) as f64 / duration_in_seconds;
                                }
                            }
                            if let Ok(first_frame_meta) = decoder.load_frame_metadata(timestamps[0]) {
                                info.width = first_frame_meta.width as u16;
                                info.height = first_frame_meta.height as u16;
                            }
                        }
                    }

                    let already = self.imported_files.iter().any(|f| f.path == *path);
                    if !already {
                        self.imported_files.push(ImportedFile {
                            path: path.clone(),
                            info: info.clone(),
                            selected: true,
                            first_timestamp: first_ts,
                        });
                        imported += 1;
                        tracing::debug!("batch imported: {} ({} total)", path, self.imported_files.len());
                    }
                }
                Err(e) => {
                    failed += 1;
                    tracing::warn!("batch import failed for {}: {}", path, e);
                }
            }
        }
        // Select the first newly imported file
        if imported > 0 && self.imported_files.len() > 0 {
            self.media_pool_index = self.imported_files.len() - imported;
            self.file_info = Some(self.imported_files[self.media_pool_index].info.clone());
            self.file_path = Some(self.imported_files[self.media_pool_index].path.clone());
            self.frame_count = self.imported_files[self.media_pool_index].info.frame_count as usize;
        }
        (imported, failed)
    }

    /// Start async import of dropped files on a background thread.
    /// Returns immediately; results arrive via DropImportEvent channel.
    pub fn start_async_import(&mut self, paths: Vec<String>) {
        // Cancel any in-progress import
        if let Some(cancel) = self.drop_import_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }

        let (tx, rx) = mpsc::channel::<DropImportEvent>();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.drop_import_cancel = Some(cancel_flag.clone());
        self.drop_import_rx = Some(rx);

        // Show preview overlay
        self.drop_preview = Some(DropPreview {
            files: paths.iter()
                .filter(|p| p.to_lowercase().ends_with(".mcraw"))
                .map(|p| p.clone())
                .collect(),
            start_time: Instant::now(),
        });

        let total = paths.len();
        self.status_message = format!("Importing {} file(s)...", total);

        std::thread::spawn(move || {
            let mut imported = 0;
            let mut failed = 0;

            for path in paths {
                if cancel_flag.load(Ordering::Relaxed) {
                    tracing::info!("async drag-drop import cancelled");
                    break;
                }

                let path_clone = path.clone();
                match McrawFileInfo::from_path(&path) {
                    Ok(mut info) => {
                        let mut first_ts: i64 = 0;
                        // Enhance with decoder metadata (same as load_file)
                        if let Ok(decoder) = Decoder::new(&path) {
                            first_ts = decoder.timestamps().ok()
                                .and_then(|ts| ts.first().copied())
                                .unwrap_or(0);
                            if let Ok(container_meta) = decoder.container_metadata() {
                                let as_f64 = |v: &[f32; 9]| -> [f64; 9] {
                                    let mut r = [0.0; 9];
                                    for (i, &x) in v.iter().enumerate() { r[i] = x as f64; }
                                    r
                                };
                                let non_zero = |m: &[f32; 9]| m.iter().any(|&x| x != 0.0);
                                info.camera_metadata.color_matrix = Some(as_f64(&container_meta.color_matrix1));
                                if non_zero(&container_meta.color_matrix2) {
                                    info.camera_metadata.color_matrix2 = Some(as_f64(&container_meta.color_matrix2));
                                }
                                if non_zero(&container_meta.forward_matrix1) {
                                    info.camera_metadata.forward_matrix1 = Some(as_f64(&container_meta.forward_matrix1));
                                }
                                if non_zero(&container_meta.forward_matrix2) {
                                    info.camera_metadata.forward_matrix2 = Some(as_f64(&container_meta.forward_matrix2));
                                }
                                if container_meta.has_calibration_illuminants {
                                    info.camera_metadata.calibration_illuminant1 = Some(container_meta.calibration_illuminant1);
                                    info.camera_metadata.calibration_illuminant2 = Some(container_meta.calibration_illuminant2);
                                }
                                if container_meta.white_level > 0.0 {
                                    info.white_level = container_meta.white_level;
                                }
                                if container_meta.black_level_count > 0 {
                                    info.black_level = container_meta.black_level[0];
                                }
                            }
                            if let Ok(timestamps) = decoder.timestamps() {
                                info.frame_count = timestamps.len() as u32;
                                if timestamps.len() >= 2 {
                                    let duration_ns = timestamps[timestamps.len() - 1] - timestamps[0];
                                    if duration_ns > 0 {
                                        let duration_in_seconds = duration_ns as f64 / 1_000_000_000.0;
                                        info.fps = (info.frame_count.saturating_sub(1)) as f64 / duration_in_seconds;
                                    }
                                }
                                if let Ok(first_frame_meta) = decoder.load_frame_metadata(timestamps[0]) {
                                    info.width = first_frame_meta.width as u16;
                                    info.height = first_frame_meta.height as u16;
                                }
                            }
                        }

                        let _ = tx.send(DropImportEvent::FileReady { path: path_clone, info, first_timestamp: first_ts });
                        imported += 1;
                    }
                    Err(e) => {
                        let _ = tx.send(DropImportEvent::Failed {
                            path: path_clone,
                            error: e.to_string(),
                        });
                        failed += 1;
                        tracing::warn!("async drag-drop import failed: {}: {}", path, e);
                    }
                }
            }

            let _ = tx.send(DropImportEvent::Complete { imported, failed });
        });
    }

    /// Poll for async drag-drop import results. Call every frame.
    pub fn poll_drop_import(&mut self) {
        let rx = match self.drop_import_rx.take() {
            Some(rx) => rx,
            None => return,
        };

        let mut keep_rx = true;
        while let Ok(event) = rx.try_recv() {
            match event {
                DropImportEvent::FileReady { path, info, first_timestamp } => {
                    let already = self.imported_files.iter().any(|f| f.path == path);
                    if !already {
                        self.imported_files.push(ImportedFile {
                            path: path.clone(),
                            info: info.clone(),
                            selected: true,
                            first_timestamp,
                        });
                        // Select the first imported file
                        if self.imported_files.len() == 1 {
                            self.media_pool_index = 0;
                            self.file_info = Some(info.clone());
                            self.file_path = Some(path.clone());
                            self.frame_count = info.frame_count as usize;
                        }
                        tracing::debug!("async imported: {} ({} total)", path, self.imported_files.len());
                    }
                }
                DropImportEvent::Failed { path, error } => {
                    tracing::warn!("async import failed: {}: {}", path, error);
                }
                DropImportEvent::Complete { imported, failed } => {
                    keep_rx = false;
                    self.drop_import_cancel = None;
                    if imported > 0 {
                        self.media_pool_index = self.imported_files.len().saturating_sub(imported);
                        if let Some(f) = self.imported_files.get(self.media_pool_index) {
                            self.file_info = Some(f.info.clone());
                            self.file_path = Some(f.path.clone());
                            self.frame_count = f.info.frame_count as usize;
                        }
                    }
                    if failed > 0 {
                        self.status_message = format!("Imported {} file(s), {} failed", imported, failed);
                    } else {
                        self.status_message = format!("Imported {} file(s)", imported);
                    }
                    tracing::info!("async drag-drop import complete: {} imported, {} failed", imported, failed);
                }
            }
        }

        if keep_rx {
            self.drop_import_rx = Some(rx);
        }
    }

    pub fn load_all_in_folder(&mut self, dir: &std::path::Path) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut mcraw_paths: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map_or(false, |ext| ext == "mcraw"))
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            mcraw_paths.sort();
            let count = mcraw_paths.len();
            for path in mcraw_paths {
                self.load_file(path);
            }
            if count > 0 {
                self.status_message = format!("Imported {} .mcraw files from {}", count, dir.display());
            } else {
                self.status_message = format!("No .mcraw files found in {}", dir.display());
            }
        }
    }

    // -----------------------------------------------------------------------
    // Media pool helpers
    // -----------------------------------------------------------------------

    pub fn focused_file_info(&self) -> Option<&McrawFileInfo> {
        self.imported_files.get(self.media_pool_index).map(|f| &f.info)
    }

    pub fn toggle_media_pool_selection(&mut self) {
        if let Some(f) = self.imported_files.get_mut(self.media_pool_index) {
            f.selected = !f.selected;
        }
    }

    /// Toggle all media pool items between selected and unselected.
    /// If any file is unselected, selects all; if all are selected, deselects all.
    pub fn toggle_select_all(&mut self) {
        if self.imported_files.is_empty() {
            return;
        }
        let all_selected = self.imported_files.iter().all(|f| f.selected);
        for f in &mut self.imported_files {
            f.selected = !all_selected;
        }
        let msg = if all_selected { "Deselected all" } else { "Selected all" };
        self.status_message = format!("{} ({} files)", msg, self.imported_files.len());
    }

    /// Switch the active decoder and preview to the file at `new_index`.
    pub fn switch_media_pool_item(&mut self, new_index: usize) {
        if new_index >= self.imported_files.len() {
            return;
        }
        if new_index == self.media_pool_index {
            return;
        }
        let path = self.imported_files[new_index].path.clone();
        self.media_pool_index = new_index;
        self.last_export_summary = None;
        self.sixel_pending.set(false);
        self.sixel_write_pos.set(None);
        self.last_written_media_index.set(None);
        if self.file_path.as_deref() != Some(&path) {
            self.load_file(path);
        } else {
            // Same file, just update preview for current frame
            self.preview_state = PreviewState::Empty;
            if self.decoder.is_some() && !self.timestamps.is_empty() {
                self.request_frame_decode(self.frame_index.min(self.timestamps.len() - 1));
            }
        }

        // Prefetch ±3 neighbor thumbnails for smooth scrolling
        let start = new_index.saturating_sub(3);
        let end = self.imported_files.len().min(new_index + 4);
        for i in start..end {
            if i == new_index { continue; }
            let n = &self.imported_files[i];
            if n.first_timestamp > 0 {
                self.request_thumbnail_for(&n.path, n.first_timestamp);
            }
        }
    }

    pub fn add_selected_to_queue(&mut self) {
        let selected: Vec<ImportedFile> = self.imported_files.iter()
            .filter(|f| f.selected)
            .cloned()
            .collect();
        if selected.is_empty() {
            self.status_message = "No files selected - use Space to select, then a to add".to_string();
            return;
        }
        let count = selected.len();
        for imp in &selected {
            let already = self.queue.iter().any(|q| q.path == imp.path);
            if !already {
                self.queue.push(QueuedFile {
                    path: imp.path.clone(),
                    info: imp.info.clone(),
                    selected: true,
                    status: QueueStatus::Waiting,
                    progress: 0.0,
                });
            }
        }
        self.status_message = format!("Added {} file(s) to render queue", count);
    }

    pub fn add_all_to_queue(&mut self) {
        if self.imported_files.is_empty() {
            self.status_message = "No files in media pool".to_string();
            return;
        }
        let count = self.imported_files.len();
        for imp in &self.imported_files {
            let already = self.queue.iter().any(|q| q.path == imp.path);
            if !already {
                self.queue.push(QueuedFile {
                    path: imp.path.clone(),
                    info: imp.info.clone(),
                    selected: true,
                    status: QueueStatus::Waiting,
                    progress: 0.0,
                });
            }
        }
        self.status_message = format!("Added all {} file(s) to render queue", count);
    }

    pub fn remove_from_media_pool(&mut self) {
        if self.imported_files.is_empty() {
            return;
        }
        let name = self.imported_files[self.media_pool_index]
            .path
            .split(std::path::MAIN_SEPARATOR)
            .last()
            .unwrap_or("unknown")
            .to_string();
        self.imported_files.remove(self.media_pool_index);
        if self.media_pool_index >= self.imported_files.len() && self.imported_files.len() > 0 {
            self.media_pool_index = self.imported_files.len() - 1;
        }
        self.status_message = format!("Removed {} from media pool", name);
    }

    // -----------------------------------------------------------------------
    // Queue helpers
    // -----------------------------------------------------------------------

    pub fn toggle_queue_selection(&mut self) {
        if let Some(q) = self.queue.get_mut(self.queue_index) {
            q.selected = !q.selected;
        }
    }

    pub fn remove_from_queue(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let has_selected = self.queue.iter().any(|q| q.selected);
        if has_selected {
            self.queue.retain(|q| !q.selected);
            self.status_message = "Removed selected items from queue".to_string();
        } else {
            let name = self.queue[self.queue_index]
                .path
                .split(std::path::MAIN_SEPARATOR)
                .last()
                .unwrap_or("unknown")
                .to_string();
            self.queue.remove(self.queue_index);
            if self.queue_index >= self.queue.len() && self.queue.len() > 0 {
                self.queue_index = self.queue.len() - 1;
            }
            self.status_message = format!("Removed {} from queue", name);
        }
        if self.queue_index >= self.queue.len() && !self.queue.is_empty() {
            self.queue_index = self.queue.len() - 1;
        }
    }

    pub fn clear_completed_queue(&mut self) {
        let before = self.queue.len();
        self.queue.retain(|q| !matches!(q.status, QueueStatus::Completed | QueueStatus::Failed(_)));
        let removed = before - self.queue.len();
        if removed > 0 {
            self.status_message = format!("Cleared {} completed/failed item(s)", removed);
        } else {
            self.status_message = "No completed/failed items to clear".to_string();
        }
        if self.queue_index >= self.queue.len() && !self.queue.is_empty() {
            self.queue_index = self.queue.len() - 1;
        }
    }

    pub fn render_selected(&mut self) {
        let selected_indices: Vec<usize> = self.queue.iter()
            .enumerate()
            .filter(|(_, q)| q.selected)
            .map(|(i, _)| i)
            .collect();
        if selected_indices.is_empty() {
            self.status_message = "No items selected in queue - use Space to select".to_string();
            return;
        }
        self.status_message = format!("Starting render of {} selected file(s)...", selected_indices.len());
        // Start the first one
        if let Some(&first_idx) = selected_indices.first() {
            self.current_rendering_index = Some(first_idx);
            let q = &self.queue[first_idx];
            self.file_info = Some(q.info.clone());
            self.file_path = Some(q.path.clone());
            self.frame_count = q.info.frame_count as usize;
            self.start_export();
        }
    }

    pub fn render_all(&mut self) {
        if self.queue.is_empty() {
            self.status_message = "Queue is empty".to_string();
            return;
        }
        self.status_message = format!("Starting render of all {} file(s)...", self.queue.len());
        for q in &mut self.queue {
            q.selected = true;
        }
        // Start from the first item
        self.current_rendering_index = Some(0);
        if let Some(q) = self.queue.first() {
            self.file_info = Some(q.info.clone());
            self.file_path = Some(q.path.clone());
            self.frame_count = q.info.frame_count as usize;
            self.start_export();
        }
    }

    fn start_next_queued_render(&mut self) {
        // Find the next selected queue item that's Waiting
        if let Some(current) = self.current_rendering_index {
            let next_idx = (current + 1..self.queue.len())
                .find(|&i| self.queue[i].selected && self.queue[i].status == QueueStatus::Waiting);
            if let Some(idx) = next_idx {
                self.current_rendering_index = Some(idx);
                self.queue[idx].status = QueueStatus::Rendering;
                let q = &self.queue[idx];
                self.file_info = Some(q.info.clone());
                self.file_path = Some(q.path.clone());
                self.frame_count = q.info.frame_count as usize;
                self.start_export();
            } else {
                // No more items to render
                self.current_rendering_index = None;
                let done = self.queue.iter().filter(|q| q.selected && q.status == QueueStatus::Completed).count();
                let total = self.queue.iter().filter(|q| q.selected).count();
                self.status_message = format!("Batch render complete: {}/{} done", done, total);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Export profile helpers
    // -----------------------------------------------------------------------

    pub fn active_profile_is_8bit(&self) -> bool {
        match self.export_codec_family {
            CodecFamily::ProRes => false,
            CodecFamily::DNxHR => false,
            CodecFamily::HEVC => self.hevc_profile.is_8bit(),
            CodecFamily::H264 => self.h264_profile.is_8bit(),
            CodecFamily::AV1 => self.av1_profile.is_8bit(),
            CodecFamily::VP9 => self.vp9_profile.is_8bit(),
        }
    }

    pub fn active_profile_name(&self) -> &'static str {
        match self.export_codec_family {
            CodecFamily::ProRes => self.prores_profile.name(),
            CodecFamily::DNxHR => self.dnxhr_profile.name(),
            CodecFamily::HEVC => self.hevc_profile.name(),
            CodecFamily::H264 => self.h264_profile.name(),
            CodecFamily::AV1 => self.av1_profile.name(),
            CodecFamily::VP9 => self.vp9_profile.name(),
        }
    }

    pub fn cycle_rate_control(&mut self) {
        self.active_rate_control = self.active_rate_control.next();
        self.is_editing_custom_rate = false;
        self.status_message = format!("Rate: {}", self.active_rate_control.name());
    }

    pub fn fps_label(fps: Option<f64>) -> String {
        match fps {
            None => "Original".to_string(),
            Some(v) if (v - 23.976).abs() < 0.001 => "23.976".to_string(),
            Some(v) if (v - 24.0).abs() < 0.001 => "24".to_string(),
            Some(v) if (v - 25.0).abs() < 0.001 => "25".to_string(),
            Some(v) if (v - 30.0).abs() < 0.001 => "30".to_string(),
            Some(v) if (v - 50.0).abs() < 0.001 => "50".to_string(),
            Some(v) if (v - 60.0).abs() < 0.001 => "60".to_string(),
            Some(v) if (v - 120.0).abs() < 0.001 => "120".to_string(),
            Some(v) => format!("{:.3}", v),
        }
    }

    /// Cycle through FPS presets: Original → 23.976 → 24 → 25 → 30 → 50 → 60 → 120
    pub fn cycle_export_fps(&mut self) {
        self.export_fps = match self.export_fps {
            None => Some(23.976),
            Some(v) if (v - 23.976).abs() < 0.001 => Some(24.0),
            Some(v) if (v - 24.0).abs() < 0.001 => Some(25.0),
            Some(v) if (v - 25.0).abs() < 0.001 => Some(30.0),
            Some(v) if (v - 30.0).abs() < 0.001 => Some(50.0),
            Some(v) if (v - 50.0).abs() < 0.001 => Some(60.0),
            Some(v) if (v - 60.0).abs() < 0.001 => Some(120.0),
            _ => None,
        };
        self.export_focus = ExportFocus::Fps;
        self.status_message = format!("FPS: {}", Self::fps_label(self.export_fps));
    }

    pub fn cycle_codec(&mut self, forward: bool) {
        self.export_codec_family = if forward {
            self.export_codec_family.next()
        } else {
            self.export_codec_family.prev()
        };
        self.export_focus = ExportFocus::CodecFamily;
        self.status_message = format!("Codec: {}", self.export_codec_family.name());
    }

    pub fn cycle_profile(&mut self, forward: bool) {
        match self.export_codec_family {
            CodecFamily::ProRes => {
                self.prores_profile = if forward { self.prores_profile.next() } else { self.prores_profile.prev() };
                self.status_message = format!("Profile: {}", self.prores_profile.name());
            }
            CodecFamily::DNxHR => {
                self.dnxhr_profile = if forward { self.dnxhr_profile.next() } else { self.dnxhr_profile.prev() };
                self.status_message = format!("Profile: {}", self.dnxhr_profile.name());
            }
            CodecFamily::HEVC => {
                self.hevc_profile = if forward { self.hevc_profile.next() } else { self.hevc_profile.prev() };
                self.status_message = format!("Profile: {}", self.hevc_profile.name());
            }
            CodecFamily::H264 => {
                self.h264_profile = if forward { self.h264_profile.next() } else { self.h264_profile.prev() };
                self.status_message = format!("Profile: {}", self.h264_profile.name());
            }
            CodecFamily::AV1 => {
                self.av1_profile = if forward { self.av1_profile.next() } else { self.av1_profile.prev() };
                self.status_message = format!("Profile: {}", self.av1_profile.name());
            }
            CodecFamily::VP9 => {
                self.vp9_profile = if forward { self.vp9_profile.next() } else { self.vp9_profile.prev() };
                self.status_message = format!("Profile: {}", self.vp9_profile.name());
            }
        }
        self.export_focus = ExportFocus::Profile;
    }

    pub fn start_export(&mut self) {
        if self.is_exporting {
            tracing::info!("export cancelled by user (was already exporting)");
            self.cancel_export();
            self.status_message = "Export cancelled. Press V again to restart.".to_string();
            return;
        }
        let info = match self.file_info.clone() {
            Some(i) => i,
            None => {
                tracing::warn!("start_export called with no file loaded");
                self.status_message = "No file loaded".to_string();
                return;
            }
        };

        if self.export_transfer_function.requires_10bit() && self.active_profile_is_8bit() {
            tracing::warn!("export blocked: log/HDR to 8-bit codec not supported");
            self.status_message = "Cannot export Log/HDR to 8-bit codec".to_string();
            return;
        }

        let input_path = std::path::Path::new(&info.path);
        let parent = self.export_folder.clone().unwrap_or_else(|| {
            input_path.parent().unwrap_or_else(|| std::path::Path::new(".")).to_path_buf()
        });
        let stem = input_path.file_stem().and_then(|s| s.to_str()).unwrap_or("output");

        let ext = match self.export_codec_family {
            CodecFamily::ProRes | CodecFamily::DNxHR => "mov",
            CodecFamily::VP9 => "webm",
            _ => "mp4",
        };
        let tf_label = self.export_transfer_function.name().replace([' ', '(', ')', '.'], "");
        let cs_label = self.export_color_space.name().replace([' ', '(', ')', '.'], "");
        let filename = format!("{}_{}_{}.{}", stem, tf_label, cs_label, ext);
        let mut file = parent.join(&filename);
        let mut suffix = 1;
        while file.exists() {
            let base = format!("{}_{}_{}_{}", stem, tf_label, cs_label, suffix);
            file = parent.join(&base).with_extension(ext);
            suffix += 1;
        }
        let output_path = file.to_string_lossy().to_string();
        tracing::info!("export starting: output={} codec={} profile={} rate={}",
            output_path, self.export_codec_family.name(),
            self.active_profile_name(), self.active_rate_control.name());
        let cs = self.export_color_space;
        let tf = self.export_transfer_function;
        let cf = self.export_codec_family;
        let pp = self.prores_profile;
        let dp = self.dnxhr_profile;
        let hp = self.hevc_profile;
        let h4p = self.h264_profile;
        let ap = self.av1_profile;
        let vp = self.vp9_profile;
        let hevc_enc = self.hardware_caps.best_hevc_encoder.clone();
        let h264_enc = self.hardware_caps.best_h264_encoder.clone();
        let av1_enc = self.hardware_caps.best_av1_encoder.clone();
        let prores_enc = self.hardware_caps.best_prores_encoder.clone();

        self.is_exporting = true;
        self.export_cancelled = false;
        self.export_progress = 0.0;
        self.export_start_time = Some(Instant::now());
        // Starting a fresh export — drop any previous summary so the UI
        // switches from the post-render panel back to the live progress
        // panel.
        self.last_export_summary = None;
        // Capture the settings that this export was launched with so the
        // summary stays accurate even if the user cycles the export-settings
        // panel mid-render.
        self.pending_export_summary = Some(ExportSummary {
            output_path: output_path.clone(),
            codec_label: cf.name().to_string(),
            profile_label: self.active_profile_name().to_string(),
            color_space: cs.name().to_string(),
            transfer: tf.name().to_string(),
            rate_control: self.active_rate_control.name(),
            frame_count: info.frame_count as usize,
            elapsed: Duration::default(),
            result: Ok(()),
        });
        // Mark queue item as Rendering
        if let Some(idx) = self.current_rendering_index {
            if idx < self.queue.len() {
                self.queue[idx].status = QueueStatus::Rendering;
            }
        }
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.cancel_token = Some(cancel_flag.clone());
        let (tx, rx) = mpsc::channel::<ExportEvent>();
        self.export_rx = Some(rx);
        self.status_message = format!(
            "Starting export: {} / {} via {} {} ...",
            cs.name(),
            tf.name(),
            cf.name(),
            self.active_profile_name(),
        );

        let progress_cb = {
            let prog_tx = tx.clone();
            Arc::new(move |pct: f64| { let _ = prog_tx.send(ExportEvent::Progress(pct)); })
        };

        let rate_control = self.active_rate_control.clone();
        let custom_fps = self.export_fps;
        let stats = Arc::new(PipelineStats::new());
        let stats_for_event = Arc::clone(&stats);

        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::pipeline::run_export(
                    info, output_path, progress_cb, cancel_flag, stats,
                    cs, tf, cf, pp, dp, hp, h4p, ap, vp,
                    hevc_enc, h264_enc, av1_enc, prores_enc,
                    rate_control, custom_fps,
                )
            }));
            // Always emit stats before Done so the UI can persist them,
            // even on panic/cancel.
            let _ = tx.send(ExportEvent::Stats(stats_for_event));
            match result {
                Ok(export_result) => {
                    let _ = tx.send(ExportEvent::Done(export_result));
                }
                Err(panic) => {
                    tracing::error!("export thread panicked: {:?}", panic);
                    let _ = tx.send(ExportEvent::Done(Err(anyhow::anyhow!("Export thread panicked"))));
                }
            }
        });
    }

    pub fn remove_selected_from_media_pool(&mut self) {
        let has_selected = self.imported_files.iter().any(|f| f.selected);
        if has_selected {
            let count = self.imported_files.iter().filter(|f| f.selected).count();
            self.imported_files.retain(|f| !f.selected);
            if self.media_pool_index >= self.imported_files.len() && !self.imported_files.is_empty() {
                self.media_pool_index = self.imported_files.len() - 1;
            }
            self.status_message = format!("Removed {} selected file(s) from media pool", count);
        } else {
            self.status_message = "No files selected - use Space to select".to_string();
        }
    }

    pub fn set_export_folder(&mut self, folder: std::path::PathBuf) {
        self.export_folder = Some(folder);
        self.status_message = format!("Export folder set");
    }

    pub fn toggle_favourite_folder(&mut self, folder: PathBuf) {
        if let Some(pos) = self.favourite_folders.iter().position(|f| f == &folder) {
            self.favourite_folders.remove(pos);
            self.status_message = "Removed from favourites".to_string();
        } else {
            self.favourite_folders.push(folder);
            self.status_message = "Added to favourites".to_string();
        }
        self.save_favourites();
    }

    // -----------------------------------------------------------------------
    // Export presets
    // -----------------------------------------------------------------------

    /// Snapshot the current export settings as a named preset and persist
    /// the full preset list to disk. If a preset with the same name already
    /// exists it is replaced in place.
    pub fn save_current_as_preset(&mut self, name: String) {
        let name = name.trim().to_string();
        if name.is_empty() {
            self.status_message = "Preset name cannot be empty".to_string();
            return;
        }
        let preset = ExportPreset::snapshot(
            name.clone(),
            self.export_color_space,
            self.export_transfer_function,
            self.export_codec_family,
            self.prores_profile,
            self.dnxhr_profile,
            self.hevc_profile,
            self.h264_profile,
            self.av1_profile,
            self.vp9_profile,
            self.active_rate_control.clone(),
            self.export_folder.clone(),
        );
        ExportPreset::upsert(&mut self.presets, preset);
        ExportPreset::save_all(&self.presets);
        self.active_preset = Some(name.clone());
        self.status_message = format!("Saved preset: {}", name);
    }

    /// Apply the preset at the given index, copying every field onto the
    /// app's live state.
    pub fn apply_preset(&mut self, index: usize) {
        if index >= self.presets.len() {
            return;
        }
        let p = self.presets[index].clone();
        self.export_color_space = p.color_space;
        self.export_transfer_function = p.transfer_function;
        self.export_codec_family = p.codec_family;
        self.prores_profile = p.prores_profile;
        self.dnxhr_profile = p.dnxhr_profile;
        self.hevc_profile = p.hevc_profile;
        self.h264_profile = p.h264_profile;
        self.av1_profile = p.av1_profile;
        self.vp9_profile = p.vp9_profile;
        self.active_rate_control = p.rate_control;
        self.export_folder = p.export_folder;
        // Exit custom-rate edit mode if the preset isn't a custom rate.
        if !matches!(self.active_rate_control, RateControl::Custom(_)) {
            self.is_editing_custom_rate = false;
        }
        self.active_preset = Some(p.name.clone());
        self.status_message = format!("Applied preset: {}", p.name);
    }

    /// Delete the preset at the given index. If that preset was the active
    /// one, clear the active marker.
    pub fn delete_preset(&mut self, index: usize) {
        if index >= self.presets.len() {
            return;
        }
        let removed_name = self.presets[index].name.clone();
        self.presets.remove(index);
        ExportPreset::save_all(&self.presets);
        if self.active_preset.as_deref() == Some(removed_name.as_str()) {
            self.active_preset = None;
        }
        // Keep the cursor in bounds.
        if !self.presets.is_empty() && self.preset_picker.index >= self.presets.len() {
            self.preset_picker.index = self.presets.len() - 1;
        }
        self.preset_picker.message = Some(format!("Deleted preset: {}", removed_name));
        self.status_message = format!("Deleted preset: {}", removed_name);
    }

    /// Open the preset picker overlay. If there are no presets, surface a
    /// hint in the status bar instead of opening an empty list.
    pub fn open_preset_picker(&mut self) {
        if self.presets.is_empty() {
            self.status_message = "No presets yet — press [p] to save the current settings".to_string();
            return;
        }
        self.preset_picker.open = true;
        self.preset_picker.index = self.presets.len().saturating_sub(1).min(self.preset_picker.index);
        self.preset_picker.message = None;
    }

    pub fn close_preset_picker(&mut self) {
        self.preset_picker.open = false;
        self.preset_picker.message = None;
    }

    /// Enter the in-line naming mode for a new preset. The user types the
    /// name and presses Enter to save.
    pub fn begin_naming_preset(&mut self) {
        let default_name = match &self.active_preset {
            Some(n) => format!("{} (copy)", n),
            None => "My Preset".to_string(),
        };
        self.preset_naming = Some(PresetNamingState { name: default_name, message: None });
        self.preset_picker.open = false;
    }

    pub fn cancel_naming_preset(&mut self) {
        self.preset_naming = None;
    }

    /// Finalize naming: save the preset and exit the naming state.
    pub fn commit_naming_preset(&mut self) {
        let name = match self.preset_naming.as_ref() {
            Some(s) => s.name.clone(),
            None => return,
        };
        self.preset_naming = None;
        self.save_current_as_preset(name);
    }

    /// True if the current settings exactly match the named preset (best
    /// effort: only checked for the fields we know about).
    pub fn current_matches_preset(&self, name: &str) -> bool {
        if let Some(p) = self.presets.iter().find(|p| p.name == name) {
            p.color_space == self.export_color_space
                && p.transfer_function == self.export_transfer_function
                && p.codec_family == self.export_codec_family
                && p.prores_profile == self.prores_profile
                && p.dnxhr_profile == self.dnxhr_profile
                && p.hevc_profile == self.hevc_profile
                && p.h264_profile == self.h264_profile
                && p.av1_profile == self.av1_profile
                && p.vp9_profile == self.vp9_profile
                && p.rate_control.name() == self.active_rate_control.name()
                && p.export_folder == self.export_folder
        } else {
            false
        }
    }

    pub fn import_selected_from_browser(&mut self) {
        let paths = self.browser.selected_mcraw_paths();
        if paths.is_empty() {
            self.status_message = "No .mcraw files selected in browser".to_string();
            return;
        }
        let count = paths.len();
        let (imported, failed) = self.load_files_batch(&paths);
        let msg = if failed > 0 {
            format!("Imported {} file(s), {} failed", imported, failed)
        } else {
            format!("Imported {} file(s)", imported)
        };
        self.status_message = msg;
        // Clear selection checkboxes on imported files
        for entry in self.browser.entries.iter_mut() {
            if entry.selected && entry.name.to_lowercase().ends_with(".mcraw") {
                entry.selected = false;
            }
        }
        if count > 0 {
            self.show_browser = false;
        }
    }

    pub fn cancel_export(&mut self) {
        if let Some(ref token) = self.cancel_token {
            tracing::info!("export cancellation requested");
            token.store(true, Ordering::Relaxed);
            self.export_cancelled = true;
            self.status_message = "Cancelling export...".to_string();
        }
    }

    pub fn poll_export(&mut self) {
        let rx = match self.export_rx.take() {
            Some(rx) => rx,
            None => return,
        };
        let mut keep_rx = true;
        while let Ok(event) = rx.try_recv() {
            match event {
                ExportEvent::Progress(pct) => {
                    self.export_progress = pct;
                    if let Some(q) = self.queue.iter_mut().find(|q| matches!(q.status, QueueStatus::Rendering)) {
                        q.progress = pct;
                    }
                }
                ExportEvent::Stats(_stats) => {
                    // Stats are collected internally for future TUI display
                    // (FPS meter, phase timing chart). No terminal output.
                }
                ExportEvent::Done(result) => {
                    self.is_exporting = false;
                    keep_rx = false;
                    self.cancel_token = None;
                    let elapsed = self.export_start_time
                        .take()
                        .map(|t| t.elapsed())
                        .unwrap_or_default();
                    // Mark the currently rendering item
                    if let Some(idx) = self.current_rendering_index {
                        if idx < self.queue.len() {
                            self.queue[idx].progress = 100.0;
                            if self.export_cancelled {
                                self.queue[idx].status = QueueStatus::Waiting;
                            } else {
                                match &result {
                                    Ok(()) => {
                                        self.queue[idx].status = QueueStatus::Completed;
                                    }
                                    Err(e) => {
                                        self.queue[idx].status = QueueStatus::Failed(e.to_string());
                                    }
                                }
                            }
                        }
                    }
                    // Build the post-render summary. Always shown (success,
                    // failure, or cancellation) so the user can see what
                    // ran and for how long.
                    if let Some(mut summary) = self.pending_export_summary.take() {
                        summary.elapsed = elapsed;
                        summary.result = if self.export_cancelled {
                            Err("Cancelled by user".to_string())
                        } else {
                            match &result {
                                Ok(()) => Ok(()),
                                Err(e) => Err(e.to_string()),
                            }
                        };
                        self.last_export_summary = Some(summary);
                    }
                    if self.export_cancelled {
                        self.status_message = "Export cancelled".to_string();
                        self.export_cancelled = false;
                        self.current_rendering_index = None;
                    } else {
                        let mins = elapsed.as_secs() / 60;
                        let secs = elapsed.as_secs() % 60;
                        match result {
                            Ok(()) => {
                                tracing::info!("export completed in {:02}m {:02}s", mins, secs);
                                self.status_message = format!(
                                    "Video export completed ({:02}m {:02}s)", mins, secs
                                );
                                self.shockwave_ticks_remaining = 30;
                            }
                            Err(e) => {
                                tracing::error!("export failed: {}", e);
                                self.status_message = format!("Export failed: {}", e);
                            }
                        }
                        // Auto-start next queued item
                        self.start_next_queued_render();
                    }
                    self.export_start_time = None;
                }
            }
        }
        if keep_rx {
            self.export_rx = Some(rx);
        }
    }

    pub fn add_encode_job(&mut self, format: OutputFormat) {
        let job = EncodeJob::new(uuid::Uuid::new_v4().to_string()[..8].to_string(), format);
        self.encode_jobs.push(job);
        self.status_message = "Export job added".to_string();
    }

    // -----------------------------------------------------------------------
    // Browser navigation
    // -----------------------------------------------------------------------

    pub fn select_file(&mut self) {
        let entry_data = self.browser.selected_entry().map(|e| (e.is_dir, e.name.clone(), e.path.clone()));
        if let Some((is_dir, name, path)) = entry_data {
            if is_dir {
                self.browser.enter();
                self.status_message = format!("Entered: {}", name);
                self.show_favourites_bar = false;
            } else if name.ends_with(".mcraw") {
                let path_str = path.to_string_lossy().to_string();
                self.load_file(path_str.clone());
                self.show_browser = false;

                // Add to media pool if not already present
                if let Some(ref info) = self.file_info {
                    if !self.imported_files.iter().any(|f| f.path == path_str) {
                        self.imported_files.push(ImportedFile {
                            path: path_str.clone(),
                            info: info.clone(),
                            selected: true,
                            first_timestamp: self.timestamps.first().copied().unwrap_or(0),
                        });
                    }
                }

                // Set media pool index and trigger thumbnail decode
                if let Some(idx) = self.imported_files.iter().position(|f| f.path == path_str) {
                    self.media_pool_index = idx;
                }
                self.last_written_media_index.set(None);
                self.sixel_pending.set(false);
                self.sixel_write_pos.set(None);
                self.sixel_occupy_size.set(None);
                if self.decoder.is_some() && !self.timestamps.is_empty() {
                    self.request_frame_decode(self.frame_index.min(self.timestamps.len() - 1));
                }
            } else {
                self.status_message = format!("Cannot open: {} (not a .mcraw file)", name);
            }
        }
    }

    /// Scan a folder for all .mcraw files and return sorted paths
    pub fn scan_mcraw_files_in_folder(&self, folder: &str) -> Vec<String> {
        if let Ok(entries) = std::fs::read_dir(folder) {
            let mut files: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map_or(false, |ext| ext.to_ascii_lowercase() == "mcraw"))
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            files.sort();
            files
        } else {
            Vec::new()
        }
    }

    pub fn navigate_browser(&mut self, direction: BrowserDirection) {
        match direction {
            BrowserDirection::Up => {
                self.browser.navigate_up();
            }
            BrowserDirection::Down => {
                self.browser.navigate_down();
            }
            BrowserDirection::Enter => self.select_file(),
            BrowserDirection::GoUp => {
                self.browser.go_up();
                self.show_favourites_bar = false;
            }
            BrowserDirection::ToggleHidden => self.browser.toggle_hidden(),
        }
    }

    /// Move the favourites-list cursor by `delta`. Clamps to bounds.
    pub fn navigate_favourites(&mut self, delta: i64) {
        if self.favourite_folders.is_empty() {
            return;
        }
        let cur = self.favourites_scroll_offset.get() as i64;
        let max = (self.favourite_folders.len() as i64) - 1;
        let next = (cur + delta).clamp(0, max);
        self.favourites_scroll_offset.set(next as usize);
    }

    /// Navigate into the favourite at the current cursor position.
    pub fn open_selected_favourite(&mut self) {
        let idx = self.favourites_scroll_offset.get();
        if let Some(path) = self.favourite_folders.get(idx).cloned() {
            self.status_message = format!("Navigated to favourite: {}", path.display());
            self.browser = FileBrowser::from_path(path);
            self.browser_scroll_offset = Cell::new(0);
            self.browsing_favourites = false;
            self.show_favourites_bar = false;
        }
    }

    /// Delete the favourite at the current cursor position.
    pub fn delete_selected_favourite(&mut self) {
        let idx = self.favourites_scroll_offset.get();
        if idx < self.favourite_folders.len() {
            let name = self.favourite_folders[idx].display().to_string();
            self.favourite_folders.remove(idx);
            self.save_favourites();
            if self.favourite_folders.is_empty() {
                self.browsing_favourites = false;
            } else if self.favourites_scroll_offset.get() >= self.favourite_folders.len() {
                self.favourites_scroll_offset.set(self.favourite_folders.len() - 1);
            }
            self.status_message = format!("Removed favourite: {}", name);
        }
    }

    // -----------------------------------------------------------------------
    // Focus cycling
    // -----------------------------------------------------------------------

    pub fn cycle_focus(&mut self) {
        self.focus_target = match self.focus_target {
            FocusTarget::MediaPool => FocusTarget::Grade,
            FocusTarget::Grade => FocusTarget::ExportSettings,
            FocusTarget::ExportSettings => FocusTarget::Queue,
            FocusTarget::Queue => FocusTarget::MediaPool,
        };
        let label = match self.focus_target {
            FocusTarget::MediaPool => "Media Pool",
            FocusTarget::Grade => "Grade",
            FocusTarget::ExportSettings => "Export Settings",
            FocusTarget::Queue => "Render Queue",
        };
        self.status_message = format!("Focus: {}", label);
    }

    pub fn set_focus(&mut self, target: FocusTarget) {
        self.focus_target = target;
        let label = match target {
            FocusTarget::MediaPool => "Media Pool",
            FocusTarget::Grade => "Grade",
            FocusTarget::ExportSettings => "Export Settings",
            FocusTarget::Queue => "Render Queue",
        };
        self.status_message = format!("Focus: {}", label);
    }

}

fn execute_click_action(app: &mut App, action: ClickAction) {
    match action {
        ClickAction::ToggleBrowser => {
            app.show_browser = !app.show_browser;
            app.status_message = if app.show_browser { "Browser shown" } else { "Browser hidden" }.to_string();
        }
        ClickAction::ToggleFileSelection(i) => {
            if let Some(f) = app.imported_files.get_mut(i) {
                f.selected = !f.selected;
            }
        }
        ClickAction::ToggleQueueSelection(i) => {
            if let Some(q) = app.queue.get_mut(i) {
                q.selected = !q.selected;
            }
        }
        ClickAction::SelectMediaPoolItem(i) => {
            if i < app.imported_files.len() {
                app.switch_media_pool_item(i);
            }
        }
        ClickAction::SelectQueueItem(i) => {
            if i < app.queue.len() {
                app.queue_index = i;
                app.set_focus(FocusTarget::Queue);
            }
        }
        ClickAction::FocusMediaPool => {
            app.set_focus(FocusTarget::MediaPool);
        }
        ClickAction::FocusQueue => {
            app.set_focus(FocusTarget::Queue);
        }
        ClickAction::FocusExport => {
            app.set_focus(FocusTarget::ExportSettings);
        }
        ClickAction::FocusGrade => {
            app.show_grade_screen = !app.show_grade_screen;
            if app.show_grade_screen {
                app.set_focus(FocusTarget::Grade);
                app.status_message = "Grade screen — Esc to exit".to_string();
            } else {
                app.grade_dragging = None;
                app.set_focus(FocusTarget::MediaPool);
                app.status_message = "Normal view".to_string();
            }
        }
        ClickAction::AddSelectedToQueue => app.add_selected_to_queue(),
        ClickAction::AddAllToQueue => app.add_all_to_queue(),
        ClickAction::RemoveSelectedFromMediaPool => app.remove_selected_from_media_pool(),
        ClickAction::ToggleSelectAll => app.toggle_select_all(),
        ClickAction::ToggleBrowserSelection(i) => {
            if let Some(entry) = app.browser.entries.get_mut(i) {
                if entry.name.to_lowercase().ends_with(".mcraw") {
                    entry.selected = !entry.selected;
                }
            }
        }
        ClickAction::RenderSelected => app.render_selected(),
        ClickAction::RenderAll => app.render_all(),
        ClickAction::ClearQueue => app.clear_completed_queue(),
        ClickAction::CycleCodec => {
            app.set_focus(FocusTarget::ExportSettings);
            app.cycle_codec(true);
        }
        ClickAction::CycleGamut => {
            app.set_focus(FocusTarget::ExportSettings);
            app.export_focus = ExportFocus::ColorSpace;
            app.export_color_space = app.export_color_space.next();
            app.status_message = format!("Gamut: {}", app.export_color_space.name());
        }
        ClickAction::CycleTransfer => {
            app.set_focus(FocusTarget::ExportSettings);
            app.export_focus = ExportFocus::TransferFunction;
            app.export_transfer_function = app.export_transfer_function.next();
            app.status_message = format!("Transfer: {}", app.export_transfer_function.name());
        }
        ClickAction::CycleProfile => {
            app.set_focus(FocusTarget::ExportSettings);
            app.cycle_profile(true);
        }
        ClickAction::CycleRate => {
            app.set_focus(FocusTarget::ExportSettings);
            app.export_focus = ExportFocus::RateControl;
            app.cycle_rate_control();
        }
        ClickAction::CycleFps => {
            app.set_focus(FocusTarget::ExportSettings);
            app.cycle_export_fps();
        }
        ClickAction::ImportOption1 => {
            if app.import_popup != ImportPopupState::Hidden {
                if let ImportPopupState::DroppedFiles { files, .. } = &app.import_popup {
                    let files = files.clone();
                    if !files.is_empty() {
                        let count = files.len();
                        app.status_message = format!("Importing {} file(s)...", count);
                        let (imported, failed) = app.load_files_batch(&files);
                        if failed > 0 {
                            app.status_message = format!("Imported {} file(s), {} failed", imported, failed);
                        } else {
                            app.status_message = format!("Imported {} file(s)", imported);
                        }
                    }
                    app.import_popup = ImportPopupState::Hidden;
                    app.show_browser = false;
                }
            } else if app.show_browser {
                app.import_selected_from_browser();
            }
        }
        ClickAction::ImportOption2 => {
            if app.import_popup != ImportPopupState::Hidden {
                if let ImportPopupState::DroppedFiles { all_in_folder, .. } = &app.import_popup {
                    let all_in_folder = all_in_folder.clone();
                    if !all_in_folder.is_empty() {
                        let count = all_in_folder.len();
                        app.status_message = format!("Importing all {} file(s) from folder...", count);
                        let (imported, failed) = app.load_files_batch(&all_in_folder);
                        if failed > 0 {
                            app.status_message = format!("Imported {} file(s), {} failed", imported, failed);
                        } else {
                            app.status_message = format!("Imported all {} file(s)", imported);
                        }
                    }
                    app.import_popup = ImportPopupState::Hidden;
                    app.show_browser = false;
                }
            } else if app.show_browser {
                let folder = app.browser.current_path.clone();
                app.load_all_in_folder(&folder);
                app.show_browser = false;
            }
        }
        ClickAction::ClosePopup => { app.import_popup = ImportPopupState::Hidden; }
        ClickAction::ToggleHelp => { app.show_help = !app.show_help; }
        ClickAction::BrowserNavigate(i) => {
            let now = Instant::now();
            let was_same = app.last_browser_click.as_ref().map(|&(_, idx)| idx == i).unwrap_or(false);
            let is_double = app.last_browser_click.as_ref().map(|&(t, _)| now.duration_since(t).as_millis() < 400).unwrap_or(false);

            app.browser.selected_index = i;

            if was_same && is_double {
                app.select_file();
                app.last_browser_click = None;
            } else {
                app.last_browser_click = Some((now, i));
            }
        }
        ClickAction::BrowserSelectAndEnter(i) => {
            let now = Instant::now();
            let was_same = app.last_browser_click.as_ref().map(|&(_, idx)| idx == i).unwrap_or(false);
            let is_double = app.last_browser_click.as_ref().map(|&(t, _)| now.duration_since(t).as_millis() < 400).unwrap_or(false);

            app.browser.selected_index = i;

            if was_same && is_double {
                app.select_file();
                app.last_browser_click = None;
            } else {
                app.last_browser_click = Some((now, i));
            }
        }
        ClickAction::BrowserEnter => {
            app.navigate_browser(BrowserDirection::Enter);
        }
        ClickAction::BrowserGoUp => {
            app.navigate_browser(BrowserDirection::GoUp);
        }
        ClickAction::FavouriteNavigate(i) => {
            if i < app.favourite_folders.len() {
                let path = app.favourite_folders[i].clone();
                app.browser = FileBrowser::from_path(path);
                app.browser_scroll_offset = Cell::new(0);
                app.show_favourites_bar = false;
                app.last_clicked_favourite = Some((Instant::now(), i));
                app.status_message = "Navigated to favourite folder".to_string();
            }
        }
        ClickAction::OpenPresetPicker => {
            app.open_preset_picker();
        }
        ClickAction::GradeSlider(i) => {
            app.grade_focus = i;
            app.set_focus(FocusTarget::Grade);
        }
    }
}

pub enum BrowserDirection {
    Up,
    Down,
    Enter,
    GoUp,
    ToggleHidden,
}

pub async fn run(args: Cli) -> Result<()> {
    // Resolve placeholder path: CLI --placeholder-path > env MCRAW_TUI_PLACEHOLDER
    let placeholder_path = args.placeholder_path.clone()
        .or_else(|| std::env::var("MCRAW_TUI_PLACEHOLDER").ok())
        .map(std::path::PathBuf::from);
    if let Some(ref p) = placeholder_path {
        tracing::info!("custom placeholder: {}", p.display());
    }

    let mut app = App::new_with_placeholder(placeholder_path);
    tracing::info!("app initialized: hardware_caps={:?}", app.hardware_caps);

    match args.resolve() {
        ResolvedCli::Command(CliCommands::Open { file }) => {
            if let Some(path) = file {
                app.load_file(path);
            }
        }
        ResolvedCli::Command(CliCommands::Info { file }) => {
            let path = match file {
                Some(p) => p,
                None => return Err(anyhow::anyhow!("No file specified")),
            };
            match McrawFileInfo::from_path(&path) {
                Ok(mut info) => {
                    info.enhance_with_decoder();
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
        ResolvedCli::Command(CliCommands::Export { file, format, output }) => {
            if file.is_none() {
                return Err(anyhow::anyhow!("No file specified"));
            }
            if let Err(e) = Cli::validate_export_format(&format) {
                anyhow::bail!("{}", e);
            }
            let format = match format.to_lowercase().as_str() {
                "dng" => OutputFormat::DNG { output_path: std::path::PathBuf::from(&output) },
                "prores" => OutputFormat::ProRes { output_path: std::path::PathBuf::from(&output) },
                "h264" => OutputFormat::H264 { output_path: std::path::PathBuf::from(&output) },
                "hevc" => OutputFormat::HEVC { output_path: std::path::PathBuf::from(&output) },
                _ => anyhow::bail!("Invalid format: {}", format),
            };

            let encoder = Encoder::new();
            let mut job = EncodeJob::new("cli-export".to_string(), format.clone());
            job.status = EncodeStatus::Running;

            match encoder.start_job(job.clone()).await {
                Ok(()) => { job.status = EncodeStatus::Completed; }
                Err(e) => { job.status = EncodeStatus::Failed(e.to_string()); }
            }
            return Ok(());
        }
        ResolvedCli::NoFile => {
            app.status_message = "No file specified. Use: mcraw-tui -f <path>".to_string();
        }
    }

    let stdout = std::io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;
    terminal.clear()?;
    crossterm::execute!(
        std::io::stdout(),
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture,
    )?;
    terminal.hide_cursor()?;

    enable_raw_mode()?;
    tracing::info!("terminal initialized: alternate_screen, bracketed_paste, mouse_capture enabled");

    let event_loop_running = Arc::new(AtomicBool::new(true));
    let elr = event_loop_running.clone();

    let (tx, rx) = mpsc::channel();
    tokio::spawn(async move {
        event_loop(tx, elr).await;
    });

    let encoder = Encoder::new();
    tracing::info!("entering main event loop");

    while app.running {
        // Update terminal cell size for sixel positioning
        if let Ok(ws) = window_size() {
            if ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0 {
                app.term_cell_size.set((
                    ws.width as f32 / ws.columns as f32,
                    ws.height as f32 / ws.rows as f32,
                ));
            }
        }

        app.poll_export();
        app.poll_drop_import();
        // Only generate thumbnails when on the normal (main) screen.
        // On Grade / Culling / Welcome, poll_thumbnail would poll GPU and set
        // PreviewState::Ready, causing the Ghost Widget to draw sixel over
        // those screens via the unconditional Ready check below.
        let on_normal_main = !app.show_grade_screen
            && !app.show_culling
            && !app.imported_files.is_empty();
        if on_normal_main {
            app.poll_thumbnail();
        }
        app.browser.try_refresh();

        // Record render timestamp BEFORE drawing so the FPS meter includes
        // the draw and sleep overhead, giving a realistic "frames the user
        // actually sees" reading.
        app.fps_counter.tick();

        let mut click_regions = Vec::new();
        terminal.draw(|frame| ui::render(frame, &app, &mut click_regions))?;

        // Ghost Widget: clear stale sixel when navigating to a different file,
        // then write the new sixel bytes after ratatui has flushed its buffer.
        let current_idx = app.media_pool_index;
        let file_changed = app.last_written_media_index.get() != Some(current_idx);

        // Clear old sixel area when file changes
        if file_changed {
            if let Some((lx, ly, lw, lh)) = app.sixel_occupy_size.get() {
                let clear_line: Vec<u8> = vec![b' '; lw as usize];
                for row in ly..(ly + lh).min(9999) {
                    let _ = std::io::stdout()
                        .queue(MoveTo(lx, row))
                        .and_then(|out| out.write_all(&clear_line));
                }
                app.sixel_occupy_size.set(None);
            }
        }

        if app.sixel_pending.get()
            && !app.is_exporting
            && (app.last_export_summary.is_none() || app.focused_file_info().or(app.file_info.as_ref()).is_some())
            && !app.show_grade_screen
            && !app.show_culling {
            if let Some((x, y)) = app.sixel_write_pos.get() {
                if let PreviewState::Ready { ref sixel, .. } = app.preview_state {
                    let _ = std::io::stdout()
                        .queue(MoveTo(x, y))
                        .and_then(|out| out.write_all(sixel));
                }
            }
            app.sixel_pending.set(false);
            app.last_written_media_index.set(Some(current_idx));
        }

        // Advance animation state

        // Advance animation state
        app.spinner_frame = app.spinner_frame.wrapping_add(1);
        // Slow the dither animation to ~800ms cycle (every 4th tick)
        if app.spinner_frame % 4 == 0 {
            app.progress_anim_offset = app.progress_anim_offset.wrapping_add(1);
        }
        if app.shockwave_ticks_remaining > 0 {
            app.shockwave_ticks_remaining -= 1;
        }
        // Decay grade morph animation
        if let Some((_, ref mut t)) = app.grade_morph {
            *t = t.saturating_sub(1);
            if *t == 0 { app.grade_morph = None; }
        }
        // Decay phosphor trail
        app.phosphor_trail.iter_mut().for_each(|(_, t)| *t = t.saturating_sub(1));
        app.phosphor_trail.retain(|(_, t)| *t > 0);
        // Decay focus strip idle counter
        if app.grade_strip_idle_ticks > 0 {
            app.grade_strip_idle_ticks = app.grade_strip_idle_ticks.saturating_sub(1);
        } else if app.show_grade_screen {
            app.grade_strip_active = false;
        }

        // Drain ALL pending events each frame — critical for drag-drop where
        // the terminal sends a burst of events that must be consumed together.
        // Processing only one per frame causes input lag and wrong key events
        // leaking through between paste characters.
        while let Ok(event) = rx.try_recv() {
            handle_event(&mut app, event, &encoder, &click_regions).await;
        }

        time::sleep(Duration::from_millis(16)).await;
    }

    event_loop_running.store(false, Ordering::Relaxed);
    drop(rx);
    tokio::task::yield_now().await;

    disable_raw_mode()?;
    terminal.show_cursor()?;
    crossterm::execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen,
    )?;
    tracing::info!("terminal shutdown: raw_mode disabled, screen restored");

    Ok(())
}

async fn event_loop(tx: mpsc::Sender<Event>, running: Arc<AtomicBool>) {
    tracing::debug!("event_loop started");
    while running.load(Ordering::Relaxed) {
        if crossterm::event::poll(Duration::from_millis(8)).unwrap() {
            if let Ok(event) = crossterm::event::read() {
                if tx.send(event).is_err() {
                    break;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Drag-drop path parsing helpers
// ---------------------------------------------------------------------------

/// Strip surrounding quotes from a path string (handles nested quotes).
fn strip_surrounding_quotes(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 {
        let first = s.chars().next().unwrap();
        let last = s.chars().last().unwrap();
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// Expand ~ to home directory.
fn expand_tilde(s: &str) -> String {
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    s.to_string()
}

/// Decode file:// URIs to native paths.
/// Handles file:///C:/... (Windows) and file:///home/... (Unix).
fn decode_file_uri(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("file:///") {
        // file:///C:/path → C:/path (Windows) or file:///home → /home (Unix)
        if cfg!(windows) && rest.len() >= 2 {
            let chars: Vec<char> = rest.chars().collect();
            if chars.len() >= 2 && chars[0].is_ascii_alphabetic() && chars[1] == ':' {
                return rest.to_string();
            }
        }
        // Unix: file:///home/user → /home/user
        return format!("/{}", rest);
    }
    if let Some(rest) = s.strip_prefix("file://") {
        // file://hostname/path (network paths) — strip hostname
        if let Some(slash_pos) = rest.find('/') {
            return rest[slash_pos..].to_string();
        }
        return rest.to_string();
    }
    s.to_string()
}

/// Percent-decode URI-encoded characters (e.g. %20 → space, %C3%A9 → é).
fn percent_decode_path(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    match percent_decode_str(s).decode_utf8() {
        Ok(decoded) => decoded.into_owned(),
        Err(_) => s.to_string(), // Fall back to original if decoding fails
    }
}

/// Normalize path separators for the current platform.
fn normalize_path(s: &str) -> String {
    if cfg!(windows) {
        // Preserve UNC paths (\\server\share)
        if s.starts_with("\\\\") {
            return s.to_string();
        }
        // Convert forward slashes to backslashes
        s.replace('/', "\\")
    } else {
        s.to_string()
    }
}

/// Validate and canonicalize a path. Returns None if path doesn't exist.
fn validate_path(s: &str) -> Option<String> {
    let path = std::path::Path::new(s);

    // Check if path exists
    if !path.exists() {
        tracing::debug!("path validation: does not exist: {}", s);
        return None;
    }

    // Try to canonicalize (resolves symlinks and normalizes)
    // Fall back to original if canonicalization fails
    match path.canonicalize() {
        Ok(canonical) => Some(canonical.to_string_lossy().to_string()),
        Err(_) => {
            tracing::debug!("path validation: canonicalize failed, using original: {}", s);
            Some(s.to_string())
        }
    }
}

async fn handle_event(app: &mut App, event: Event, _encoder: &Encoder, click_regions: &[ui::ClickRegion]) {
    match event {
        // -------------------------------------------------------------------
        // Drag & Drop: pasted file paths
        // -------------------------------------------------------------------
        Event::Paste(pasted) => {
            tracing::trace!("drag-drop: raw pasted bytes={:?} len={}", pasted.as_bytes(), pasted.len());

            let paths: Vec<String> = pasted
                .lines()
                .filter_map(|line| {
                    let line = line.trim();
                    if line.is_empty() {
                        return None;
                    }

                    // Strip surrounding quotes (handles "path with spaces")
                    let stripped = strip_surrounding_quotes(line);

                    // Expand ~ to home directory
                    let expanded = expand_tilde(&stripped);

                    // Decode file:// URI if present
                    let decoded = decode_file_uri(&expanded);

                    // Percent-decode URI-encoded characters (e.g. %20 → space, %C3%A9 → é)
                    let percent_decoded = percent_decode_path(&decoded);

                    // Platform-specific path normalization
                    let normalized = normalize_path(&percent_decoded);

                    // Validate path exists and canonicalize
                    validate_path(&normalized)
                })
                .collect();

            tracing::trace!("drag-drop: parsed {} paths: {:?}", paths.len(), paths);

            if paths.is_empty() {
                app.status_message = "Drag-drop: no valid paths received".to_string();
                return;
            }

            // Separate .mcraw files and directories
            let mut mcraw_files: Vec<String> = Vec::new();
            let mut folders: Vec<String> = Vec::new();

            for p in &paths {
                let path = std::path::Path::new(p);
                if path.is_dir() {
                    folders.push(p.clone());
                } else if p.to_lowercase().ends_with(".mcraw") {
                    mcraw_files.push(p.clone());
                }
            }

            // If folders were dropped, scan them for .mcraw files
            for folder in &folders {
                if let Ok(entries) = std::fs::read_dir(folder) {
                    let mut files: Vec<String> = entries
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| p.extension().map_or(false, |ext| ext.to_ascii_lowercase() == "mcraw"))
                        .map(|p| p.to_string_lossy().to_string())
                        .collect();
                    files.sort();
                    mcraw_files.extend(files);
                }
            }

            // Deduplicate while preserving order
            let mut seen = std::collections::HashSet::new();
            mcraw_files.retain(|f| seen.insert(f.clone()));

            tracing::info!("drag-drop: {} .mcraw files, {} folders", mcraw_files.len(), folders.len());

            if mcraw_files.is_empty() {
                app.status_message = "Drag-drop: no .mcraw files found in dropped items".to_string();
                return;
            }

            // Trigger visual feedback
            app.drop_highlight = Some(Instant::now());

            // Smart import: instant for small batches, async for larger ones
            // Threshold: <= 3 files = async (smooth UI), > 3 = popup for confirmation
            const ASYNC_THRESHOLD: usize = 3;

            if mcraw_files.len() <= ASYNC_THRESHOLD && folders.is_empty() {
                // Small batch: use async import for smooth UI
                app.start_async_import(mcraw_files);
            } else {
                // Large batch or folders: show import popup
                // Check if single file is alone in its folder
                if mcraw_files.len() == 1 {
                    let file = &mcraw_files[0];
                    let folder = std::path::Path::new(file)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| ".".to_string());

                    let all_in_folder: Vec<String> = if let Ok(entries) = std::fs::read_dir(&folder) {
                        let mut files: Vec<String> = entries
                            .filter_map(|e| e.ok())
                            .map(|e| e.path())
                            .filter(|p| p.extension().map_or(false, |ext| ext.to_ascii_lowercase() == "mcraw"))
                            .map(|p| p.to_string_lossy().to_string())
                            .collect();
                        files.sort();
                        files
                    } else {
                        Vec::new()
                    };

                    // Only skip popup if this is truly the only .mcraw in the folder
                    if all_in_folder.len() == 1 {
                        app.start_async_import(mcraw_files);
                        return;
                    }
                }

                // Determine the primary folder for the import popup
                let folder = if !folders.is_empty() {
                    folders[0].clone()
                } else {
                    std::path::Path::new(&mcraw_files[0])
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| ".".to_string())
                };

                // Scan ALL .mcraw files in the primary folder
                let all_in_folder: Vec<String> = if let Ok(entries) = std::fs::read_dir(&folder) {
                    let mut files: Vec<String> = entries
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| p.extension().map_or(false, |ext| ext.to_ascii_lowercase() == "mcraw"))
                        .map(|p| p.to_string_lossy().to_string())
                        .collect();
                    files.sort();
                    files
                } else {
                    Vec::new()
                };

                // Show import popup
                app.import_popup = ImportPopupState::DroppedFiles {
                    files: mcraw_files,
                    folder,
                    all_in_folder,
                };
            }
        }

        // -------------------------------------------------------------------
        // Terminal resize — clear stale sixel output and re-request preview
        // -------------------------------------------------------------------
        crossterm::event::Event::Resize(_, _) => {
            app.preview_state = PreviewState::Empty;
            if app.decoder.is_some() && !app.timestamps.is_empty() {
                app.request_frame_decode(app.frame_index.min(app.timestamps.len() - 1));
            }
        }

        // -------------------------------------------------------------------
        // Mouse events
        // -------------------------------------------------------------------
        Event::Mouse(mouse_event) => {
            use crossterm::event::{MouseEventKind, MouseButton};

            // Allow mouse on import popup (has its own click regions)
            if app.import_popup != ImportPopupState::Hidden {
                let col = mouse_event.column;
                let row = mouse_event.row;
                match mouse_event.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        for region in click_regions.iter().rev() {
                            if col >= region.area.x && col < region.area.x + region.area.width
                                && row >= region.area.y && row < region.area.y + region.area.height {
                                match &region.action {
                                    ClickAction::ImportOption1 | ClickAction::ImportOption2 => {
                                        execute_click_action(app, region.action.clone());
                                    }
                                    _ => {}
                                }
                                break;
                            }
                        }
                    }
                    _ => {}
                }
                return;
            }

            // Block mouse events when full info overlay is active
            if app.show_full_info {
                return;
            }

            match mouse_event.kind {
                MouseEventKind::ScrollUp => {
                    if app.show_help {
                        app.help_scroll = app.help_scroll.saturating_sub(1);
                    } else if app.show_browser {
                        if app.browsing_favourites {
                            app.navigate_favourites(-1);
                        } else if app.browser.selected_index > 0 {
                            app.browser.selected_index -= 1;
                        }
                    } else {
                        match app.focus_target {
                            FocusTarget::MediaPool => { if app.media_pool_index > 0 { let ni = app.media_pool_index - 1; app.switch_media_pool_item(ni); } }
                            FocusTarget::Queue => { if app.queue_index > 0 { app.queue_index -= 1; } }
                            FocusTarget::ExportSettings => {
                                // Cycle VALUES of the currently focused setting
                                match app.export_focus {
                                    ExportFocus::CodecFamily => app.cycle_codec(false),
                                    ExportFocus::ColorSpace => {
                                        app.export_color_space = app.export_color_space.prev();
                                        app.status_message = format!("Gamut: {}", app.export_color_space.name());
                                    }
                                    ExportFocus::TransferFunction => {
                                        app.export_transfer_function = app.export_transfer_function.prev();
                                        app.status_message = format!("Transfer: {}", app.export_transfer_function.name());
                                    }
                                    ExportFocus::Profile => app.cycle_profile(false),
                                    ExportFocus::RateControl => {
                                        app.active_rate_control = app.active_rate_control.prev();
                                        app.status_message = format!("Rate: {}", app.active_rate_control.name());
                                    }
                                    ExportFocus::Fps => app.cycle_export_fps(),
                                }
                            }
                            FocusTarget::Grade => {
                                let step = if mouse_event.modifiers.contains(crossterm::event::KeyModifiers::SHIFT) {
                                    GradeSliders::step_large(app.grade_focus)
                                } else {
                                    GradeSliders::step_small(app.grade_focus)
                                };
                                app.grade_sliders.apply_delta(app.grade_focus, step);
                            }
                        }
                    }
                }
                MouseEventKind::ScrollDown => {
                    if app.show_help {
                        app.help_scroll = app.help_scroll.saturating_add(1);
                    } else if app.show_browser {
                        if app.browsing_favourites {
                            app.navigate_favourites(1);
                        } else {
                            let len = app.browser.entries.len();
                            if len > 0 { app.browser.selected_index = (app.browser.selected_index + 1).min(len - 1); }
                        }
                    } else {
                        match app.focus_target {
                            FocusTarget::MediaPool => {
                                let ni = (app.media_pool_index + 1).min(app.imported_files.len().saturating_sub(1));
                                if ni != app.media_pool_index { app.switch_media_pool_item(ni); }
                            }
                            FocusTarget::Queue => {
                                let len = app.queue.len();
                                if len > 0 { app.queue_index = (app.queue_index + 1).min(len - 1); }
                            }
                            FocusTarget::ExportSettings => {
                                match app.export_focus {
                                    ExportFocus::CodecFamily => app.cycle_codec(true),
                                    ExportFocus::ColorSpace => {
                                        app.export_color_space = app.export_color_space.next();
                                        app.status_message = format!("Gamut: {}", app.export_color_space.name());
                                    }
                                    ExportFocus::TransferFunction => {
                                        app.export_transfer_function = app.export_transfer_function.next();
                                        app.status_message = format!("Transfer: {}", app.export_transfer_function.name());
                                    }
                                    ExportFocus::Profile => app.cycle_profile(true),
                                    ExportFocus::RateControl => app.cycle_rate_control(),
                                    ExportFocus::Fps => app.cycle_export_fps(),
                                }
                            }
                            FocusTarget::Grade => {
                                let step = if mouse_event.modifiers.contains(crossterm::event::KeyModifiers::SHIFT) {
                                    GradeSliders::step_large(app.grade_focus)
                                } else {
                                    GradeSliders::step_small(app.grade_focus)
                                };
                                app.grade_sliders.apply_delta(app.grade_focus, -step);
                            }
                        }
                    }
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    let col = mouse_event.column;
                    let row = mouse_event.row;
                    for region in click_regions.iter().rev() {
                        if col >= region.area.x && col < region.area.x + region.area.width
                            && row >= region.area.y && row < region.area.y + region.area.height {
                            match &region.action {
                                ClickAction::GradeSlider(i) => {
                                    let now = Instant::now();
                                    let is_double = app.last_grade_click.as_ref()
                                        .map(|&(t, idx)| idx == *i && now.duration_since(t).as_millis() < 400)
                                        .unwrap_or(false);
                                    if is_double {
                                        // Double-click: reset to default
                                        let def = GradeSliders::default_val(*i);
                                        app.grade_sliders.set(*i, def);
                                        app.last_grade_click = None;
                                        app.status_message = format!("Reset {} to default", GradeSliders::name(*i));
                                    } else {
                                        // Single click: set value from x position + start drag
                                        let x_offset = col.saturating_sub(region.area.x);
                                        let norm = (x_offset as f32 / region.area.width.max(1) as f32).clamp(0.0, 1.0);
                                        let lo = GradeSliders::min(*i);
                                        let hi = GradeSliders::max(*i);
                                        app.grade_sliders.set(*i, lo + norm * (hi - lo));
                                        app.grade_focus = *i;
                                        app.grade_dragging = Some((*i, region.area.x, region.area.width));
                                        app.last_grade_click = Some((now, *i));
                                    }
                                }
                                _ => execute_click_action(app, region.action.clone()),
                            }
                            break;
                        }
                    }
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    if let Some((i, track_x, track_w)) = app.grade_dragging {
                        let col = mouse_event.column;
                        let x_offset = col.saturating_sub(track_x);
                        let norm = (x_offset as f32 / track_w.max(1) as f32).clamp(0.0, 1.0);
                        let lo = GradeSliders::min(i);
                        let hi = GradeSliders::max(i);
                        app.grade_sliders.set(i, lo + norm * (hi - lo));
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    app.grade_dragging = None;
                }
                _ => {}
            }
        }

        // -------------------------------------------------------------------
        // Keyboard events
        // -------------------------------------------------------------------
        Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
            if let crossterm::event::KeyCode::Char('c') = key_event.code {
                if key_event.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
                    tracing::info!("ctrl+c received, quitting");
                    app.running = false;
                    return;
                }
            }
            // Ctrl+X cancels an in-progress export. Outside of an export it
            // is a no-op so it never accidentally trashes the queue.
            if let crossterm::event::KeyCode::Char('x') = key_event.code {
                if key_event.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
                    if app.is_exporting {
                        tracing::info!("ctrl+x received, cancelling export");
                        app.cancel_export();
                    }
                    return;
                }
            }

            tracing::debug!("key event: code={:?} modifiers={:?}", key_event.code, key_event.modifiers);

            // ----------------------------------------------------------------
            // Preset naming (inline text entry)
            // ----------------------------------------------------------------
            if app.preset_naming.is_some() {
                let naming = app.preset_naming.clone().unwrap();
                match key_event.code {
                    crossterm::event::KeyCode::Char(c) => {
                        if let Some(state) = app.preset_naming.as_mut() {
                            state.name.push(c);
                        }
                    }
                    crossterm::event::KeyCode::Backspace => {
                        if let Some(state) = app.preset_naming.as_mut() {
                            state.name.pop();
                        }
                    }
                    crossterm::event::KeyCode::Enter => {
                        app.commit_naming_preset();
                    }
                    crossterm::event::KeyCode::Esc => {
                        app.cancel_naming_preset();
                        app.status_message = "Preset save cancelled".to_string();
                    }
                    _ => {}
                }
                let _ = naming; // Silence unused warning if not used.
                return;
            }

            // ----------------------------------------------------------------
            // Preset picker overlay
            // ----------------------------------------------------------------
            if app.preset_picker.open {
                match key_event.code {
                    crossterm::event::KeyCode::Esc => app.close_preset_picker(),
                    crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                        if app.preset_picker.index > 0 {
                            app.preset_picker.index -= 1;
                        }
                        app.preset_picker.message = None;
                    }
                    crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                        if app.preset_picker.index + 1 < app.presets.len() {
                            app.preset_picker.index += 1;
                        }
                        app.preset_picker.message = None;
                    }
                    crossterm::event::KeyCode::Enter => {
                        let idx = app.preset_picker.index;
                        app.close_preset_picker();
                        app.apply_preset(idx);
                    }
                    crossterm::event::KeyCode::Delete | crossterm::event::KeyCode::Backspace => {
                        let idx = app.preset_picker.index;
                        app.delete_preset(idx);
                    }
                    _ => {}
                }
                return;
            }

            // ----------------------------------------------------------------
            // Import popup
            // ----------------------------------------------------------------
            if app.import_popup != ImportPopupState::Hidden {
                let has_option2 = if let ImportPopupState::DroppedFiles { files, all_in_folder, .. } = &app.import_popup {
                    all_in_folder.len() > files.len()
                } else {
                    false
                };

                match key_event.code {
                    crossterm::event::KeyCode::Char('1') => {
                        let files = if let ImportPopupState::DroppedFiles { files, .. } = &app.import_popup {
                            files.clone()
                        } else {
                            Vec::new()
                        };
                        if !files.is_empty() {
                            let count = files.len();
                            app.status_message = format!("Importing {} file(s)...", count);
                            let (imported, failed) = app.load_files_batch(&files);
                            if failed > 0 {
                                app.status_message = format!("Imported {} file(s), {} failed", imported, failed);
                            } else {
                                app.status_message = format!("Imported {} file(s)", imported);
                            }
                        }
                        app.import_popup = ImportPopupState::Hidden;
                        app.show_browser = false;
                    }
                    crossterm::event::KeyCode::Char('2') if has_option2 => {
                        let all_in_folder = if let ImportPopupState::DroppedFiles { all_in_folder, .. } = &app.import_popup {
                            all_in_folder.clone()
                        } else {
                            Vec::new()
                        };
                        if !all_in_folder.is_empty() {
                            let count = all_in_folder.len();
                            app.status_message = format!("Importing all {} file(s) from folder...", count);
                            let (imported, failed) = app.load_files_batch(&all_in_folder);
                            if failed > 0 {
                                app.status_message = format!("Imported {} file(s), {} failed", imported, failed);
                            } else {
                                app.status_message = format!("Imported all {} file(s)", imported);
                            }
                        }
                        app.import_popup = ImportPopupState::Hidden;
                        app.show_browser = false;
                    }
                    crossterm::event::KeyCode::Enter => {
                        let files = if let ImportPopupState::DroppedFiles { files, .. } = &app.import_popup {
                            files.clone()
                        } else {
                            Vec::new()
                        };
                        if !files.is_empty() {
                            let count = files.len();
                            app.status_message = format!("Importing {} file(s)...", count);
                            let (imported, failed) = app.load_files_batch(&files);
                            if failed > 0 {
                                app.status_message = format!("Imported {} file(s), {} failed", imported, failed);
                            } else {
                                app.status_message = format!("Imported {} file(s)", imported);
                            }
                        }
                        app.import_popup = ImportPopupState::Hidden;
                        app.show_browser = false;
                    }
                    crossterm::event::KeyCode::Esc => {
                        app.import_popup = ImportPopupState::Hidden;
                    }
                    _ => {}
                }
                return;
            }

            // ----------------------------------------------------------------
            // Custom rate inline editing
            // ----------------------------------------------------------------
            if app.is_editing_custom_rate {
                match key_event.code {
                    crossterm::event::KeyCode::Char(c) => {
                        if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == 'M' || c == 'k' || c == 'm' {
                            if let RateControl::Custom(ref mut val) = app.active_rate_control {
                                val.push(c);
                            }
                        }
                    }
                    crossterm::event::KeyCode::Backspace => {
                        if let RateControl::Custom(ref mut val) = app.active_rate_control {
                            val.pop();
                        }
                    }
                    crossterm::event::KeyCode::Enter | crossterm::event::KeyCode::Esc => {
                        app.is_editing_custom_rate = false;
                        app.status_message = format!("Rate: {}", app.active_rate_control.name());
                    }
                    _ => {}
                }
                return;
            }

            // ----------------------------------------------------------------
            // Normal character-key dispatch
            // ----------------------------------------------------------------
            if let crossterm::event::KeyCode::Char(c) = key_event.code {
                match c {
                    'q' => {
                        app.running = false;
                    }
                    '?' => {
                        app.show_help = !app.show_help;
                    }
                    'b' => {
                        // In grade mode, 'b' does before/after; otherwise browser toggle
                        if app.show_grade_screen || app.focus_target == FocusTarget::Grade {
                            if app.grade_before_snapshot.is_none() {
                                app.grade_before_snapshot = Some(app.grade_sliders);
                                app.grade_sliders = GradeSliders::default();
                                app.shockwave_ticks_remaining = 8;
                                app.status_message = "BEFORE — holding original values".to_string();
                            }
                        } else {
                            app.show_browser = !app.show_browser;
                            app.status_message = if app.show_browser {
                                "Browser shown"
                            } else {
                                "Browser hidden"
                            }.to_string();
                        }
                    }
                    'B' => {
                        // Release before/after: restore snapshot
                        if let Some(snap) = app.grade_before_snapshot.take() {
                            app.grade_sliders = snap;
                            app.shockwave_ticks_remaining = 5;
                            app.status_message = "AFTER — restored grade".to_string();
                        }
                    }
                    'e' => {
                        app.set_focus(FocusTarget::ExportSettings);
                    }
                    'a' => {
                        app.add_selected_to_queue();
                    }
                    'A' => {
                        app.add_all_to_queue();
                    }
                    'D' => {
                        if app.focus_target == FocusTarget::MediaPool {
                            app.remove_selected_from_media_pool();
                        }
                    }
                    'd' => {
                        // Remove the last-clicked favourite (within 2 seconds)
                        if app.show_browser && app.show_favourites_bar {
                            if let Some((ts, idx)) = app.last_clicked_favourite.take() {
                                if ts.elapsed() < Duration::from_secs(2) && idx < app.favourite_folders.len() {
                                    app.favourite_folders.remove(idx);
                                    app.status_message = "Removed from favourites".to_string();
                                    app.save_favourites();
                                    return;
                                }
                            }
                        }
                        match app.focus_target {
                            FocusTarget::MediaPool => app.remove_from_media_pool(),
                            FocusTarget::Queue => app.remove_from_queue(),
                            FocusTarget::ExportSettings => {}
                            FocusTarget::Grade => {}
                        }
                    }
                    'x' => {
                        // When an export is running, `x` (and Ctrl+X) cancel it.
                        // Otherwise it clears completed/failed items from the queue.
                        if app.is_exporting {
                            app.cancel_export();
                        } else {
                            app.clear_completed_queue();
                        }
                    }
                    'X' => {
                        if app.is_exporting {
                            app.cancel_export();
                        } else {
                            app.clear_completed_queue();
                        }
                    }
                    'v' => {
                        app.render_selected();
                    }
                    'R' => {
                        app.render_all();
                    }
                    'r' => {
                        if app.show_grade_screen || app.focus_target == FocusTarget::Grade {
                            let def = GradeSliders::default_val(app.grade_focus);
                            app.grade_sliders.set(app.grade_focus, def);
                            app.status_message = format!("Reset {} to default", GradeSliders::name(app.grade_focus));
                            app.grade_strip_active = true;
                            app.grade_strip_idle_ticks = 15;
                        } else if app.focus_target == FocusTarget::ExportSettings {
                            app.export_focus = ExportFocus::RateControl;
                            app.cycle_rate_control();
                        }
                    }
                    't' => {
                        if app.focus_target == FocusTarget::ExportSettings {
                            app.export_focus = ExportFocus::TransferFunction;
                            app.export_transfer_function = app.export_transfer_function.next();
                            app.status_message = format!("Transfer: {}", app.export_transfer_function.name());
                        }
                    }
                    'g' => {
                        if app.focus_target == FocusTarget::ExportSettings {
                            app.export_focus = ExportFocus::ColorSpace;
                            app.export_color_space = app.export_color_space.next();
                            app.status_message = format!("Gamut: {}", app.export_color_space.name());
                        }
                    }
                    'c' => {
                        if app.focus_target == FocusTarget::ExportSettings {
                            app.cycle_codec(true);
                        }
                    }
                    'o' => {
                        if app.show_browser {
                            app.set_export_folder(app.browser.current_path.clone());
                        }
                    }
                    'f' => {
                        if app.show_browser {
                            // `f` toggles between the normal folder view and
                            // a flat list of favourite folders. The bar at
                            // the top of the browser (when visible) is still
                            // mouse-only; this gives a keyboard-first path
                            // through the favourites and also fixes the
                            // `..` occlusion bug because the favourites are
                            // rendered through the normal list widget.
                            if app.browsing_favourites {
                                app.browsing_favourites = false;
                                app.status_message = "Folder view".to_string();
                            } else if app.favourite_folders.is_empty() {
                                app.status_message = "No favourites yet — press [F] to add the current folder".to_string();
                            } else {
                                app.browsing_favourites = true;
                                app.favourites_scroll_offset = Cell::new(0);
                                app.status_message = "Favourites view (press [f] or [Esc] to return)".to_string();
                            }
                        } else if app.focus_target == FocusTarget::ExportSettings {
                            app.cycle_export_fps();
                        }
                    }
                    'F' => {
                        if app.show_browser {
                            app.toggle_favourite_folder(app.browser.current_path.clone());
                        }
                    }
                    'i' => {
                        if app.focus_target == FocusTarget::ExportSettings
                            && matches!(app.active_rate_control, RateControl::Custom(_))
                        {
                            app.is_editing_custom_rate = !app.is_editing_custom_rate;
                            if app.is_editing_custom_rate {
                                app.status_message = "Type a rate value (e.g. 20, 400M, 50000k). Press Enter to confirm, Esc to cancel.".to_string();
                            }
                        } else {
                            app.show_full_info = !app.show_full_info;
                            if app.show_full_info {
                                app.status_message = "Full file info shown (press i or Esc to close)".to_string();
                            }
                        }
                    }
                    'p' => {
                        if app.focus_target == FocusTarget::ExportSettings {
                            // Save the current export settings as a new preset.
                            app.begin_naming_preset();
                        } else {
                            app.cycle_profile(true);
                        }
                    }
                    'P' => {
                        // Open the preset picker (regardless of focus —
                        // most useful from the Export Settings panel but
                        // works from anywhere for power users).
                        app.open_preset_picker();
                    }
                    's' => {
                        app.toggle_select_all();
                    }
                    'n' => {
                        if let Some(info) = app.focused_file_info().cloned().or_else(|| app.file_info.clone()) {
                            let output_path = "naked_dump.raw";
                            app.status_message = "Starting naked raw dump...".to_string();
                            match crate::pipeline::run_naked(&info, output_path) {
                                Ok(_) => {
                                    app.status_message = format!("Naked dump done: {}", output_path);
                                }
                                Err(e) => {
                                    app.status_message = format!("Naked dump failed: {}", e);
                                }
                            }
                        }
                    }
                    '.' => {
                        if app.show_browser {
                            app.browser.toggle_hidden();
                            app.status_message = if app.browser.show_hidden {
                                "Showing hidden files"
                            } else {
                                "Hiding hidden files"
                            }.to_string();
                        }
                    }
                    'L' => {
                        let folder = app.browser.current_path.clone();
                        app.load_all_in_folder(&folder);
                        app.show_browser = false;
                    }
                    'I' => {
                        if app.show_browser {
                            app.import_selected_from_browser();
                        }
                    }
                    'C' => {
                        if !app.imported_files.is_empty() {
                            app.show_culling = !app.show_culling;
                            app.status_message = if app.show_culling { "Culling mode" } else { "Normal mode" }.to_string();
                        }
                    }
                    'G' => {
                        app.show_grade_screen = !app.show_grade_screen;
                        if app.show_grade_screen {
                            // Clear stale sixel thumbnail from terminal canvas
                            if let Some((lx, ly, lw, lh)) = app.sixel_occupy_size.get() {
                                let clear_line: Vec<u8> = vec![b' '; lw as usize];
                                for row in ly..(ly + lh).min(9999) {
                                    let _ = std::io::stdout()
                                        .queue(MoveTo(lx, row))
                                        .and_then(|out| out.write_all(&clear_line));
                                }
                                app.sixel_occupy_size.set(None);
                            }
                            app.set_focus(FocusTarget::Grade);
                            app.status_message = "Grade screen — Esc to exit".to_string();
                        } else {
                            app.grade_dragging = None;
                            app.set_focus(FocusTarget::MediaPool);
                            app.status_message = "Normal view".to_string();
                        }
                    }
                    _ => {}
                }
            }

            // ----------------------------------------------------------------
            // Non-character keys
            // ----------------------------------------------------------------
            match key_event.code {
                crossterm::event::KeyCode::Esc => {
                    if app.import_popup != ImportPopupState::Hidden {
                        app.import_popup = ImportPopupState::Hidden;
                    } else if app.show_full_info {
                        app.show_full_info = false;
                    } else if app.browsing_favourites {
                        app.browsing_favourites = false;
                        app.status_message = "Folder view".to_string();
                    } else if app.show_browser {
                        app.show_browser = false;
                    } else if app.show_grade_screen {
                        app.show_grade_screen = false;
                        app.grade_dragging = None;
                        app.set_focus(FocusTarget::MediaPool);
                        app.status_message = "Normal view".to_string();
                    } else if app.show_help {
                        app.show_help = false;
                    } else {
                        app.running = false;
                    }
                }
                crossterm::event::KeyCode::Delete => {
                    if app.browsing_favourites {
                        app.delete_selected_favourite();
                    }
                }
                crossterm::event::KeyCode::Tab => {
                    app.cycle_focus();
                }
                crossterm::event::KeyCode::Enter => {
                    if app.focus_target == FocusTarget::ExportSettings
                        && matches!(app.active_rate_control, RateControl::Custom(_))
                    {
                        app.is_editing_custom_rate = !app.is_editing_custom_rate;
                        if app.is_editing_custom_rate {
                            app.status_message = "Type a rate value. Enter to confirm, Esc to cancel.".to_string();
                        }
                    } else if app.browsing_favourites {
                        app.open_selected_favourite();
                    } else if app.show_browser {
                        app.navigate_browser(BrowserDirection::Enter);
                    }
                }
                crossterm::event::KeyCode::Right | crossterm::event::KeyCode::Char('l') => {
                    if app.focus_target == FocusTarget::Grade {
                        let step = if key_event.modifiers.contains(crossterm::event::KeyModifiers::SHIFT) {
                            GradeSliders::step_large(app.grade_focus)
                        } else {
                            GradeSliders::step_small(app.grade_focus)
                        };
                        let old_norm = app.grade_sliders.normalized(app.grade_focus);
                        app.grade_sliders.apply_delta(app.grade_focus, step);
                        app.phosphor_trail.push((old_norm, 4));
                        app.grade_strip_active = true;
                        app.grade_strip_idle_ticks = 15;
                    }
                }
                crossterm::event::KeyCode::Left | crossterm::event::KeyCode::Char('h') => {
                    if app.focus_target == FocusTarget::Grade {
                        let step = if key_event.modifiers.contains(crossterm::event::KeyModifiers::SHIFT) {
                            GradeSliders::step_large(app.grade_focus)
                        } else {
                            GradeSliders::step_small(app.grade_focus)
                        };
                        let old_norm = app.grade_sliders.normalized(app.grade_focus);
                        app.grade_sliders.apply_delta(app.grade_focus, -step);
                        app.phosphor_trail.push((old_norm, 4));
                        app.grade_strip_active = true;
                        app.grade_strip_idle_ticks = 15;
                    }
                }
                crossterm::event::KeyCode::Char('L') => {
                    if app.focus_target == FocusTarget::Grade {
                        let step = GradeSliders::step_large(app.grade_focus);
                        let old_norm = app.grade_sliders.normalized(app.grade_focus);
                        app.grade_sliders.apply_delta(app.grade_focus, step);
                        app.phosphor_trail.push((old_norm, 4));
                        app.grade_strip_active = true;
                        app.grade_strip_idle_ticks = 15;
                    }
                }
                crossterm::event::KeyCode::Char('H') => {
                    if app.focus_target == FocusTarget::Grade {
                        let step = GradeSliders::step_large(app.grade_focus);
                        let old_norm = app.grade_sliders.normalized(app.grade_focus);
                        app.grade_sliders.apply_delta(app.grade_focus, -step);
                        app.phosphor_trail.push((old_norm, 4));
                        app.grade_strip_active = true;
                        app.grade_strip_idle_ticks = 15;
                    }
                }
                crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                    if app.show_help {
                        app.help_scroll = app.help_scroll.saturating_sub(1);
                    } else if app.browsing_favourites {
                        app.navigate_favourites(-1);
                    } else if app.show_browser {
                        app.navigate_browser(BrowserDirection::Up);
                    } else {
                        match app.focus_target {
                            FocusTarget::MediaPool => {
                                if app.media_pool_index > 0 {
                                    app.switch_media_pool_item(app.media_pool_index - 1);
                                }
                            }
                            FocusTarget::Queue => {
                                if app.queue_index > 0 {
                                    app.queue_index -= 1;
                                }
                            }
                            FocusTarget::ExportSettings => {
                                app.export_focus = match app.export_focus {
                                    ExportFocus::ColorSpace => ExportFocus::Fps,
                                    ExportFocus::Fps => ExportFocus::RateControl,
                                    ExportFocus::TransferFunction => ExportFocus::ColorSpace,
                                    ExportFocus::CodecFamily => ExportFocus::TransferFunction,
                                    ExportFocus::Profile => ExportFocus::CodecFamily,
                                    ExportFocus::RateControl => ExportFocus::Profile,
                                };
                            }
                            FocusTarget::Grade => {
                                if app.grade_focus > 0 {
                                    app.grade_morph = Some((app.grade_focus, 4));
                                    app.grade_focus -= 1;
                                    app.grade_strip_active = true;
                                    app.grade_strip_idle_ticks = 15;
                                }
                            }
                        }
                    }
                }
                crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                    if app.show_help {
                        app.help_scroll = app.help_scroll.saturating_add(1);
                    } else if app.browsing_favourites {
                        app.navigate_favourites(1);
                    } else if app.show_browser {
                        app.navigate_browser(BrowserDirection::Down);
                    } else {
                        match app.focus_target {
                            FocusTarget::MediaPool => {
                                if app.media_pool_index + 1 < app.imported_files.len() {
                                    app.switch_media_pool_item(app.media_pool_index + 1);
                                }
                            }
                            FocusTarget::Queue => {
                                if app.queue_index + 1 < app.queue.len() {
                                    app.queue_index += 1;
                                }
                            }
                            FocusTarget::ExportSettings => {
                                app.export_focus = match app.export_focus {
                                    ExportFocus::ColorSpace => ExportFocus::TransferFunction,
                                    ExportFocus::TransferFunction => ExportFocus::CodecFamily,
                                    ExportFocus::CodecFamily => ExportFocus::Profile,
                                    ExportFocus::Profile => ExportFocus::RateControl,
                                    ExportFocus::RateControl => ExportFocus::Fps,
                                    ExportFocus::Fps => ExportFocus::ColorSpace,
                                };
                            }
                            FocusTarget::Grade => {
                                if app.grade_focus + 1 < GradeSliders::count() {
                                    app.grade_morph = Some((app.grade_focus, 4));
                                    app.grade_focus += 1;
                                    app.grade_strip_active = true;
                                    app.grade_strip_idle_ticks = 15;
                                }
                            }
                        }
                    }
                }
                crossterm::event::KeyCode::Char(' ') => {
                    if app.show_browser {
                        app.browser.toggle_selection();
                    } else {
                        match app.focus_target {
                            FocusTarget::MediaPool => app.toggle_media_pool_selection(),
                            FocusTarget::Queue => app.toggle_queue_selection(),
                            FocusTarget::ExportSettings => {}
                            FocusTarget::Grade => {}
                        }
                    }
                }
                crossterm::event::KeyCode::PageUp => {
                    if app.show_help {
                        app.help_scroll = app.help_scroll.saturating_sub(10);
                    } else if app.browsing_favourites {
                        app.navigate_favourites(-10);
                    } else if app.show_browser {
                        let entries_len = app.browser.entries.len();
                        if entries_len > 0 {
                            let new_index = app.browser.selected_index.saturating_sub(10.min(entries_len));
                            app.browser.selected_index = new_index;
                        }
                    } else if app.focus_target == FocusTarget::MediaPool {
                        let len = app.imported_files.len();
                        if len > 0 {
                            let new_index = app.media_pool_index.saturating_sub(10.min(len));
                            app.switch_media_pool_item(new_index);
                        }
                    } else if app.focus_target == FocusTarget::Queue {
                        let len = app.queue.len();
                        if len > 0 {
                            app.queue_index = app.queue_index.saturating_sub(10.min(len));
                        }
                    }
                }
                crossterm::event::KeyCode::PageDown => {
                    if app.show_help {
                        app.help_scroll = app.help_scroll.saturating_add(10);
                    } else if app.browsing_favourites {
                        app.navigate_favourites(10);
                    } else if app.show_browser {
                        let entries_len = app.browser.entries.len();
                        if entries_len > 0 {
                            let new_index = (app.browser.selected_index + 10).min(entries_len - 1);
                            app.browser.selected_index = new_index;
                        }
                    } else if app.focus_target == FocusTarget::MediaPool {
                        let len = app.imported_files.len();
                        if len > 0 {
                            let new_index = (app.media_pool_index + 10).min(len - 1);
                            app.switch_media_pool_item(new_index);
                        }
                    } else if app.focus_target == FocusTarget::Queue {
                        let len = app.queue.len();
                        if len > 0 {
                            app.queue_index = (app.queue_index + 10).min(len - 1);
                        }
                    }
                }
                crossterm::event::KeyCode::Home => {
                    if app.browsing_favourites {
                        app.favourites_scroll_offset = Cell::new(0);
                    } else if app.show_browser {
                        app.browser.selected_index = 0;
                    } else if app.focus_target == FocusTarget::MediaPool {
                        app.switch_media_pool_item(0);
                    } else if app.focus_target == FocusTarget::Queue {
                        app.queue_index = 0;
                    }
                }
                crossterm::event::KeyCode::End => {
                    if app.browsing_favourites {
                        if !app.favourite_folders.is_empty() {
                            app.favourites_scroll_offset
                                .set(app.favourite_folders.len() - 1);
                        }
                    } else if app.show_browser {
                        let entries_len = app.browser.entries.len();
                        if entries_len > 0 {
                            app.browser.selected_index = entries_len - 1;
                        }
                    } else if app.focus_target == FocusTarget::MediaPool {
                        if !app.imported_files.is_empty() {
                            app.switch_media_pool_item(app.imported_files.len() - 1);
                        }
                    } else if app.focus_target == FocusTarget::Queue {
                        if !app.queue.is_empty() {
                            app.queue_index = app.queue.len() - 1;
                        }
                    }
                }
                crossterm::event::KeyCode::Backspace => {
                    if app.browsing_favourites {
                        app.browsing_favourites = false;
                        app.status_message = "Folder view".to_string();
                    } else if app.show_browser {
                        app.navigate_browser(BrowserDirection::GoUp);
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}


