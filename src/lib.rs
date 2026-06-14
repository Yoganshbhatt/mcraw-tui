//! Public library surface for `mcraw-tui`. The binary in `src/main.rs`
//! is a thin wrapper around the same modules; integration tests under
//! `tests/` import from this crate to exercise the public API.

pub mod agx;
pub mod allocator;
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
pub mod hardware;
pub mod metadata;
pub mod pipeline;
pub mod preset;
pub mod stats;
pub mod ui;
