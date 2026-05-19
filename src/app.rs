use anyhow::Result;
use crossterm::{
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    event::{Event, KeyEventKind},
};
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::ListState;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tokio::time;

use crate::cli::{Cli, CliCommands, ResolvedCli};
use crate::color::{ColorSpace, TransferFunction};
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
use crate::ui;

#[derive(Debug)]
pub enum ExportEvent {
    Progress(f64),
    Done(Result<()>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Browse,
    Info,
    Export,
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
    pub list_state: ListState,

    pub is_exporting: bool,
    pub export_cancelled: bool,
    pub export_progress: f64,
    pub export_rx: Option<mpsc::Receiver<ExportEvent>>,
    pub cancel_token: Option<Arc<AtomicBool>>,

    // Persistent export settings (survive file loads)
    pub export_color_space: ColorSpace,
    pub export_transfer_function: TransferFunction,
    pub export_codec_family: CodecFamily,
    pub export_focus: ExportFocus,
    pub export_start_time: Option<Instant>,

    // Sticky per-codec profiles (NOT reset when switching codec family)
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFocus {
    ColorSpace,
    TransferFunction,
    CodecFamily,
    Profile,
    RateControl,
}

impl App {
    pub fn new() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        let caps = probe_hardware();
        App {
            running: true,
            screen: Screen::Browse,
            file_path: None,
            file_info: None,
            frame_index: 0,
            frame_count: 0,
            encode_jobs: Vec::new(),
            status_message: String::from("No file loaded"),
            show_help: false,
            error: None,
            browser: FileBrowser::new(),
            list_state,

            is_exporting: false,
            export_cancelled: false,
            export_progress: 0.0,
            export_rx: None,
            cancel_token: None,

            export_color_space: ColorSpace::Rec709,
            export_transfer_function: TransferFunction::Gamma24,
            export_codec_family: CodecFamily::HEVC,
            export_focus: ExportFocus::ColorSpace,
            export_start_time: None,

            prores_profile: ProResProfile::HQ,
            dnxhr_profile: DnxhrProfile::HQX,
            hevc_profile: HevcProfile::Main10_420,
            h264_profile: H264Profile::Main_8bit,
            av1_profile: Av1Profile::Profile0_420_10bit,
            vp9_profile: Vp9Profile::Profile2_420_10bit,

            hardware_caps: caps,
            active_rate_control: RateControl::Lossless,
            is_editing_custom_rate: false,
        }
    }

    pub fn load_file(&mut self, path: String) {
        self.error = None;
        self.frame_index = 0;
        self.frame_count = 0;
        self.file_info = None;
        self.file_path = None;
        self.status_message = String::new();
        match McrawFileInfo::from_path(&path) {
            Ok(mut info) => {
                if let Ok(decoder) = Decoder::new(&path) {
                    if let Ok(container_meta) = decoder.container_metadata() {
                        let as_f64 = |v: &[f32; 9]| -> [f64; 9] {
                            let mut r = [0.0; 9];
                            for (i, &x) in v.iter().enumerate() { r[i] = x as f64; }
                            r
                        };
                        let non_zero = |m: &[f32; 9]| m.iter().any(|&x| x != 0.0);

                        info.camera_metadata.color_matrix = Some(as_f64(&container_meta.color_matrix1));
                        log::info!("color_matrix1 loaded from decoder container metadata");

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
                            log::info!("White level from container metadata: {}", info.white_level);
                        }
                        if container_meta.black_level_count > 0 {
                            info.black_level = container_meta.black_level[0];
                            log::info!("Black level from container metadata: {}", info.black_level);
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
                            log::info!(
                                "FFI metadata: {}x{}, fps={} loaded from per-frame metadata",
                                first_frame_meta.width,
                                first_frame_meta.height,
                                info.fps
                            );
                        }
                    }
                }
                self.file_info = Some(info.clone());
                self.frame_count = info.frame_count as usize;
                self.file_path = Some(path.clone());
                self.status_message = format!("Loaded: {}", path);
                log::info!("Loaded file info: {:?} ({} frames)", path, info.frame_count);
            }
            Err(e) => {
                self.error = Some(format!("Failed to load file: {}", e));
                self.status_message = format!("Error: {}", e);
                log::error!("Failed to load file {:?}: {}", path, e);
            }
        }
    }

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

    pub fn active_encoder_label(&self) -> String {
        match self.export_codec_family {
            CodecFamily::HEVC => format!(
                "{} {}",
                self.hevc_profile.name(),
                self.hardware_caps.best_hevc_encoder,
            ),
            _ => self.active_profile_name().to_string(),
        }
    }

    pub fn cycle_codec(&mut self, forward: bool) {
        self.export_codec_family = if forward {
            self.export_codec_family.next()
        } else {
            self.export_codec_family.prev()
        };
        self.status_message = format!("Codec: {}", self.export_codec_family.name());
    }

    pub fn cycle_profile(&mut self, forward: bool) {
        match self.export_codec_family {
            CodecFamily::ProRes => {
                self.prores_profile = if forward {
                    self.prores_profile.next()
                } else {
                    self.prores_profile.prev()
                };
                self.status_message = format!("Profile: {}", self.prores_profile.name());
            }
            CodecFamily::DNxHR => {
                self.dnxhr_profile = if forward {
                    self.dnxhr_profile.next()
                } else {
                    self.dnxhr_profile.prev()
                };
                self.status_message = format!("Profile: {}", self.dnxhr_profile.name());
            }
            CodecFamily::HEVC => {
                self.hevc_profile = if forward {
                    self.hevc_profile.next()
                } else {
                    self.hevc_profile.prev()
                };
                self.status_message = format!("Profile: {}", self.hevc_profile.name());
            }
            CodecFamily::H264 => {
                self.h264_profile = if forward {
                    self.h264_profile.next()
                } else {
                    self.h264_profile.prev()
                };
                self.status_message = format!("Profile: {}", self.h264_profile.name());
            }
            CodecFamily::AV1 => {
                self.av1_profile = if forward {
                    self.av1_profile.next()
                } else {
                    self.av1_profile.prev()
                };
                self.status_message = format!("Profile: {}", self.av1_profile.name());
            }
            CodecFamily::VP9 => {
                self.vp9_profile = if forward {
                    self.vp9_profile.next()
                } else {
                    self.vp9_profile.prev()
                };
                self.status_message = format!("Profile: {}", self.vp9_profile.name());
            }
        }
    }

    pub fn start_export(&mut self) {
        if self.is_exporting {
            self.cancel_export();
            self.status_message = "Export cancelled. Press V again to restart.".to_string();
            return;
        }
        let info = match self.file_info.clone() {
            Some(i) => i,
            None => {
                self.status_message = "No file loaded".to_string();
                return;
            }
        };

        if self.export_transfer_function.requires_10bit() && self.active_profile_is_8bit() {
            self.status_message = "Status: Cannot export Log/HDR to 8-bit codec".to_string();
            return;
        }

        let input_path = std::path::Path::new(&info.path);
        let parent = input_path.parent().unwrap_or_else(|| std::path::Path::new("."));
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

        self.is_exporting = true;
        self.export_cancelled = false;
        self.export_progress = 0.0;
        self.export_start_time = Some(Instant::now());
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

        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::pipeline::run_export(
                    info,
                    output_path,
                    progress_cb,
                    cancel_flag,
                    cs, tf, cf, pp, dp, hp, h4p, ap, vp,
                    hevc_enc,
                    h264_enc,
                    av1_enc,
                    rate_control,
                )
            }));
            match result {
                Ok(export_result) => {
                    let _ = tx.send(ExportEvent::Done(export_result));
                }
                Err(panic) => {
                    log::error!("Export thread panicked: {:?}", panic);
                    let _ = tx.send(ExportEvent::Done(Err(anyhow::anyhow!("Export thread panicked"))));
                }
            }
        });
    }

    pub fn cancel_export(&mut self) {
        if let Some(ref token) = self.cancel_token {
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
                }
                ExportEvent::Done(result) => {
                    self.is_exporting = false;
                    keep_rx = false;
                    self.cancel_token = None;
                    if self.export_cancelled {
                        self.status_message = "Export cancelled".to_string();
                        self.export_cancelled = false;
                    } else {
                        let elapsed = self.export_start_time
                            .take()
                            .map(|t| t.elapsed())
                            .unwrap_or_default();
                        let mins = elapsed.as_secs() / 60;
                        let secs = elapsed.as_secs() % 60;
                        match result {
                            Ok(()) => {
                                self.status_message = format!(
                                    "Video export completed ({:02}m {:02}s)", mins, secs
                                );
                            }
                            Err(e) => {
                                self.status_message = format!("Export failed: {}", e);
                            }
                        }
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

    pub fn select_file(&mut self) {
        let entry_data = self.browser.selected_entry().map(|e| (e.is_dir, e.name.clone(), e.path.clone()));
        if let Some((is_dir, name, path)) = entry_data {
            if is_dir {
                self.browser.enter();
                self.list_state.select(Some(0));
                self.status_message = format!("Entered: {}", name);
            } else if name.ends_with(".mcraw") {
                let path_str = path.to_string_lossy().into_owned();
                self.load_file(path_str);
                if self.file_info.is_some() {
                    self.screen = Screen::Info;
                }
            } else {
                self.status_message = format!("Cannot open: {} (not a .mcraw file)", name);
            }
        }
    }

    pub fn navigate_browser(&mut self, direction: BrowserDirection) {
        match direction {
            BrowserDirection::Up => {
                self.browser.navigate_up();
                self.list_state.select(Some(self.browser.selected_index));
            }
            BrowserDirection::Down => {
                self.browser.navigate_down();
                self.list_state.select(Some(self.browser.selected_index));
            }
            BrowserDirection::Enter => self.select_file(),
            BrowserDirection::GoUp => {
                self.browser.go_up();
                self.list_state.select(Some(0));
            }
            BrowserDirection::ToggleHidden => self.browser.toggle_hidden(),
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
    let mut app = App::new();

    match args.resolve() {
        ResolvedCli::Command(CliCommands::Open { file }) => {
            if let Some(path) = file {
                app.load_file(path);
            }
        }
        ResolvedCli::Command(CliCommands::Info { file }) => {
            let path = match file {
                Some(p) => p,
                None => {
                    // eprintln!("Error: No file specified. Use: mcraw-tui info -f <path>");
                    return Err(anyhow::anyhow!("No file specified"));
                }
            };
            match McrawFileInfo::from_path(&path) {
                Ok(mut info) => {
                    info.enhance_with_decoder();
                    // let display = crate::metadata::format_metadata_for_display(&info);
                    // for line in display {
                    //     println!("{}", line);
                    // }
                    return Ok(());
                }
                Err(e) => {
                    // eprintln!("Error: {}", e);
                    return Err(e);
                }
            }
        }
        ResolvedCli::Command(CliCommands::Export {
            file,
            format,
            output,
        }) => {
            if file.is_none() {
                // eprintln!("Error: No file specified. Use: mcraw-tui export -f <path>");
                return Err(anyhow::anyhow!("No file specified"));
            }
            if let Err(e) = Cli::validate_export_format(&format) {
                anyhow::bail!("{}", e);
            }
            let format = match format.to_lowercase().as_str() {
                "dng" => OutputFormat::DNG {
                    output_path: std::path::PathBuf::from(&output),
                },
                "prores" => OutputFormat::ProRes {
                    output_path: std::path::PathBuf::from(&output),
                },
                "h264" => OutputFormat::H264 {
                    output_path: std::path::PathBuf::from(&output),
                },
                "hevc" => OutputFormat::HEVC {
                    output_path: std::path::PathBuf::from(&output),
                },
                _ => anyhow::bail!("Invalid format: {}", format),
            };

            let encoder = Encoder::new();
            let mut job = EncodeJob::new("cli-export".to_string(), format.clone());
            job.status = EncodeStatus::Running;

            match encoder.start_job(job.clone()).await {
                Ok(()) => {
                    job.status = EncodeStatus::Completed;
                    // println!("Export (stub) completed: {:?}", job.output_path().map(|p| p.to_string_lossy()));
                }
                Err(e) => {
                    job.status = EncodeStatus::Failed(e.to_string());
                    // eprintln!("Export failed: {}", e);
                }
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
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen)?;
    terminal.hide_cursor()?;

    enable_raw_mode()?;

    let event_loop_running = Arc::new(AtomicBool::new(true));
    let elr = event_loop_running.clone();

    let (tx, rx) = mpsc::channel();
    tokio::spawn(async move {
        event_loop(tx, elr).await;
    });

    let encoder = Encoder::new();

    while app.running {
        app.poll_export();
        app.browser.try_refresh();

        terminal.draw(|frame| ui::render(frame, &app))?;

        if let Ok(event) = rx.try_recv() {
            handle_event(&mut app, event, &encoder).await;
        }

        time::sleep(Duration::from_millis(33)).await;
    }

    // Signal event loop to stop and drop the receiver so the sender unblocks.
    event_loop_running.store(false, Ordering::Relaxed);
    drop(rx);
    // Brief yield so the event-loop task can observe the flag and exit.
    tokio::task::yield_now().await;

    disable_raw_mode()?;
    terminal.show_cursor()?;
    crossterm::execute!(std::io::stdout(), LeaveAlternateScreen)?;

    Ok(())
}

async fn event_loop(tx: mpsc::Sender<Event>, running: Arc<AtomicBool>) {
    while running.load(Ordering::Relaxed) {
        if crossterm::event::poll(Duration::from_millis(16)).unwrap() {
            if let Ok(event) = crossterm::event::read() {
                if tx.send(event).is_err() {
                    break;
                }
            }
        }
    }
}

async fn handle_event(app: &mut App, event: Event, _encoder: &Encoder) {
    match event {
        Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
            // Ctrl-C always quits
            if let crossterm::event::KeyCode::Char('c') = key_event.code {
                if key_event.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
                    app.running = false;
                    return;
                }
            }

            // ----------------------------------------------------------------
            // Custom rate-control inline editing — intercepts ALL keystrokes
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
                    'i' => {
                        // When Custom rate is active on the Export screen, 'i' toggles edit mode
                        if app.screen == Screen::Export
                            && matches!(app.active_rate_control, RateControl::Custom(_))
                        {
                            app.is_editing_custom_rate = !app.is_editing_custom_rate;
                            if app.is_editing_custom_rate {
                                app.status_message = "Type a rate value (e.g. 20, 400M, 50000k). Press Enter to confirm, Esc to cancel.".to_string();
                            }
                        } else if app.file_info.is_some() {
                            app.screen = Screen::Info;
                        }
                    }
                    'e' => {
                        app.screen = Screen::Export;
                    }
                    'b' => {
                        app.screen = Screen::Browse;
                    }
                    'd' => {
                        app.screen = match app.screen {
                            Screen::Browse => Screen::Info,
                            Screen::Info => Screen::Browse,
                            Screen::Export => Screen::Browse,
                        };
                    }
                    's' => {
                        app.status_message = "Settings (coming soon)".to_string();
                    }
                    'x' => {
                        if app.is_exporting {
                            app.cancel_export();
                        }
                    }
                    'v' => {
                        app.start_export();
                    }
                    't' => {
                        if app.screen == Screen::Export {
                            app.export_focus = ExportFocus::TransferFunction;
                            app.export_transfer_function = app.export_transfer_function.next();
                            app.status_message = format!("Transfer: {}", app.export_transfer_function.name());
                        }
                    }
                    'g' => {
                        if app.screen == Screen::Export {
                            app.export_focus = ExportFocus::ColorSpace;
                            app.export_color_space = app.export_color_space.next();
                            app.status_message = format!("Gamut: {}", app.export_color_space.name());
                        }
                    }
                    'c' => {
                        if app.screen == Screen::Export {
                            app.export_focus = ExportFocus::CodecFamily;
                            app.cycle_codec(true);
                        }
                    }
                    'p' => {
                        if app.screen == Screen::Export {
                            app.export_focus = ExportFocus::Profile;
                            app.cycle_profile(true);
                        }
                    }
                    'r' => {
                        if app.screen == Screen::Export {
                            app.export_focus = ExportFocus::RateControl;
                            app.cycle_rate_control();
                        }
                    }
                    'n' => {
                        if let Some(ref info) = app.file_info {
                            let output_path = "naked_dump.raw";
                            app.status_message = "Starting naked raw dump...".to_string();
                            match crate::pipeline::run_naked(info, output_path) {
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
                        if app.screen == Screen::Browse {
                            app.navigate_browser(BrowserDirection::ToggleHidden);
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
                    match app.screen {
                        Screen::Browse => app.running = false,
                        _ => app.screen = Screen::Browse,
                    }
                }
                crossterm::event::KeyCode::Enter => {
                    // Enter on Custom rate toggles editing
                    if app.screen == Screen::Export
                        && matches!(app.active_rate_control, RateControl::Custom(_))
                    {
                        app.is_editing_custom_rate = !app.is_editing_custom_rate;
                        if app.is_editing_custom_rate {
                            app.status_message = "Type a rate value. Enter to confirm, Esc to cancel.".to_string();
                        }
                    } else {
                        match app.screen {
                            Screen::Browse => {
                                app.navigate_browser(BrowserDirection::Enter);
                            }
                            Screen::Export => {
                                if let Some(ref info) = app.file_info {
                                    let format = OutputFormat::DNG {
                                        output_path: std::path::PathBuf::from(
                                            format!("/tmp/export_{}.dng", info.path.split('/').last().unwrap_or("file")),
                                        ),
                                    };
                                    app.add_encode_job(format);
                                }
                            }
                            Screen::Info => {}
                        }
                    }
                }
                crossterm::event::KeyCode::Right | crossterm::event::KeyCode::Char('l') => {
                    if app.frame_index < app.frame_count.saturating_sub(1) {
                        app.frame_index += 1;
                    }
                }
                crossterm::event::KeyCode::Left | crossterm::event::KeyCode::Char('h') => {
                    if app.frame_index > 0 {
                        app.frame_index -= 1;
                    }
                }
                crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                    if app.screen == Screen::Browse {
                        app.navigate_browser(BrowserDirection::Up);
                    } else if app.screen == Screen::Export {
                        match app.export_focus {
                            ExportFocus::ColorSpace => {
                                app.export_color_space = app.export_color_space.prev();
                                app.status_message = format!("Gamut: {}", app.export_color_space.name());
                            }
                            ExportFocus::TransferFunction => {
                                app.export_transfer_function = app.export_transfer_function.prev();
                                app.status_message = format!("Transfer: {}", app.export_transfer_function.name());
                            }
                            ExportFocus::CodecFamily => {
                                app.cycle_codec(false);
                            }
                            ExportFocus::Profile => {
                                app.cycle_profile(false);
                            }
                            ExportFocus::RateControl => {
                                app.cycle_rate_control();
                            }
                        }
                    }
                }
                crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                    if app.screen == Screen::Browse {
                        app.navigate_browser(BrowserDirection::Down);
                    } else if app.screen == Screen::Export {
                        match app.export_focus {
                            ExportFocus::ColorSpace => {
                                app.export_color_space = app.export_color_space.next();
                                app.status_message = format!("Gamut: {}", app.export_color_space.name());
                            }
                            ExportFocus::TransferFunction => {
                                app.export_transfer_function = app.export_transfer_function.next();
                                app.status_message = format!("Transfer: {}", app.export_transfer_function.name());
                            }
                            ExportFocus::CodecFamily => {
                                app.cycle_codec(true);
                            }
                            ExportFocus::Profile => {
                                app.cycle_profile(true);
                            }
                            ExportFocus::RateControl => {
                                app.cycle_rate_control();
                            }
                        }
                    }
                }
                crossterm::event::KeyCode::PageUp => {
                    if app.screen == Screen::Browse {
                        let entries_len = app.browser.entries.len();
                        if entries_len > 0 {
                            let new_index = app.browser.selected_index.saturating_sub(10.min(entries_len));
                            app.browser.selected_index = new_index;
                            app.list_state.select(Some(new_index));
                        }
                    }
                }
                crossterm::event::KeyCode::PageDown => {
                    if app.screen == Screen::Browse {
                        let entries_len = app.browser.entries.len();
                        if entries_len > 0 {
                            let new_index = (app.browser.selected_index + 10).min(entries_len - 1);
                            app.browser.selected_index = new_index;
                            app.list_state.select(Some(new_index));
                        }
                    }
                }
                crossterm::event::KeyCode::Home => {
                    if app.screen == Screen::Browse {
                        app.browser.selected_index = 0;
                        app.list_state.select(Some(0));
                    }
                }
                crossterm::event::KeyCode::End => {
                    if app.screen == Screen::Browse {
                        let entries_len = app.browser.entries.len();
                        if entries_len > 0 {
                            app.browser.selected_index = entries_len - 1;
                            app.list_state.select(Some(entries_len - 1));
                        }
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}
