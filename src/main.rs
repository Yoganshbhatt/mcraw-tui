mod agx;
mod app;
mod cli;
mod color;
mod decoder;
mod dng_writer;
mod encoder;
mod error;
mod export;
mod file;
mod file_browser;
mod hardware;
mod metadata;
mod pipeline;
mod ui;

use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = cli::Cli::parse();
    app::run(args).await
}
