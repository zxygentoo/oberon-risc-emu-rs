//! `oberon-risc-emu`: the windowed frontend (winit + softbuffer) and the `risc`
//! binary for the Project Oberon RISC5 emulator.
//!
//! The pure machine — CPU, software FP, MMIO, disk, serial, clipboard bridge —
//! lives in the [`risc_core`] crate. This crate adds the window, 60 fps clock
//! loop, 1-bit→ARGB rendering, input/PS-2 handling, CLI, and a deterministic
//! headless runner; the windowed entry point is [`run`].

#![deny(unsafe_code)]

pub mod error;

mod app;
mod cli;
mod clipboard;
mod input;
mod ps2;
// `pub` only so the benches/render.rs microbench (a separate target) can reach
// the scaling functions; not part of this crate's real API, so doc-hidden.
#[doc(hidden)]
pub mod render;

pub use app::run;
