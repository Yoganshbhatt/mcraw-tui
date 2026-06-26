//! Public library surface for `mcraw-tui`. The binary in `src/main.rs`
//! is a thin wrapper around the same modules; integration tests under
//! `tests/` import from this crate to exercise the public API.

pub mod agx;
pub mod app;
pub mod cli;
pub mod color;
pub mod decoder;
pub mod dng_writer;
pub mod encoder;
pub mod error;
pub mod export;
pub mod file;
pub mod file_browser;
pub mod gpu;
pub mod gradient;
pub mod grading;
pub mod gui;
pub mod hardware;
pub mod metadata;
pub mod pipeline;
pub mod preset;
pub mod preview;
pub mod stats;
pub mod terminal;
pub mod thumbnail;
pub mod thumbnail_worker;
pub mod ui;
