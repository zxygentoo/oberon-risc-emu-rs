//! The `risc` binary and windowed frontend (winit + softbuffer) for the Project
//! Oberon RISC5 emulator: the window, 60 fps clock loop, 1-bit->ARGB rendering,
//! input/PS-2 handling, CLI, and a deterministic headless runner.
//!
//! The pure machine — CPU, software FP, MMIO, disk, serial, clipboard bridge —
//! lives in the `risc_core` crate. The pixel-scaling helpers in `render` sit in
//! this crate's library (`src/lib.rs`) so the `render` benchmark can reach them;
//! everything else is here in the binary.

#![deny(unsafe_code)]

mod app;
mod cli;
mod clipboard;
mod error;
mod input;
mod ps2;

fn main() {
    if let Err(e) = app::run() {
        eprintln!("risc: {e}");
        std::process::exit(1);
    }
}
