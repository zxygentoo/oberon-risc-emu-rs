//! `oberon-risc-emu`: the windowed frontend (winit + softbuffer) and the `risc`
//! binary for the Project Oberon RISC5 emulator.
//!
//! The pure machine — CPU, software FP, MMIO, disk, serial, clipboard bridge —
//! lives in the [`risc_core`] crate. This crate adds the window, 60 fps clock
//! loop, 1-bit→ARGB rendering, input/PS-2 handling, CLI, and a deterministic
//! headless runner.

#![deny(unsafe_code)]

pub mod error;
pub mod frontend;
