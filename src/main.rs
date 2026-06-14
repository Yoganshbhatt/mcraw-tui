mod agx;
mod allocator;
mod app;
mod cli;
mod color;
mod decoder;
mod dng_writer;
mod gpu;

mod encoder;
mod error;
mod export;
mod file;
mod gradient;
mod grading;
mod gui;
mod file_browser;
mod hardware;
mod metadata;
mod pipeline;
mod preset;
mod preview;
mod stats;
mod ui;

use anyhow::Result;
use clap::Parser;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

fn log_dir() -> std::path::PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
        .join("mcraw-tui")
        .join("logs")
}

fn cleanup_old_logs(log_dir: &std::path::Path, max_age_days: u64) {
    let now = std::time::SystemTime::now();
    let max_age = std::time::Duration::from_secs(max_age_days * 86400);

    if let Ok(entries) = std::fs::read_dir(log_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "log") {
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        if now.duration_since(modified).unwrap_or(max_age) > max_age {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
            }
        }
    }
}

fn init_logging() -> WorkerGuard {
    let log_dir = log_dir();
    let _ = std::fs::create_dir_all(&log_dir);

    cleanup_old_logs(&log_dir, 7);

    let file_appender = tracing_appender::rolling::daily(&log_dir, "mcraw-tui.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("mcraw_tui=info"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(true)
        .with_line_number(true)
        .with_writer(non_blocking);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .init();

    guard
}

#[tokio::main]
async fn main() -> Result<()> {
    let _log_guard = init_logging();

    tracing::info!("mcraw-tui starting");
    if let Ok(exe) = std::env::current_exe() {
        tracing::info!("executable: {}", exe.display());
    }
    tracing::info!("platform: {}", std::env::consts::OS);
    tracing::info!("working_dir: {:?}", std::env::current_dir().ok());

    let args = cli::Cli::parse();
    tracing::info!("cli args: {:?}", args);

    let result = app::run(args).await;

    if let Err(ref e) = result {
        tracing::error!("application error: {}", e);
    }

    tracing::info!("mcraw-tui shutting down");
    result
}
