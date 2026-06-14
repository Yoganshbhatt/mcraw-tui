//! D1: global allocator. mimalloc is significantly faster than the
//! system's default allocator (glibc malloc on Linux, NT heap on
//! Windows) for the heavy bursts the export path does — 18 MB bayer
//! Vec, 9 MB upload staging, 73 MB readback buffer, all per frame.

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;
