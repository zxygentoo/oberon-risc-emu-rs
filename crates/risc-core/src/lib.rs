//! `risc-core`: a faithful, bit-exact Rust port of Peter De Wachter's
//! [`oberon-risc-emu`] — the pure RISC5 machine (CPU, software FP, MMIO, disk,
//! serial, clipboard bridge) with no windowing or platform UI.
//!
//! It is a structurally-1:1 port of the C reference (each module corresponds to
//! one C source file) and is `std` but otherwise dependency-light, so it can
//! back a winit frontend, a headless runner, or — in future — wasm/libretro.
//! The windowed `oberon-risc-emu` crate is the reference consumer.
//!
//! [`oberon-risc-emu`]: https://github.com/pdewacht/oberon-risc-emu

// No `unsafe` in this crate except two audited spots, each with a module-level
// `#![allow(unsafe_code)]` + safety note: the `cosim` FFI bindings and the unix
// `raw_serial` device (`libc::poll`/`read`/`write`). We use `deny`, not
// `forbid`, precisely because `forbid` cannot be relaxed by those local allows.
#![deny(unsafe_code)]

pub mod boot_rom;
pub mod clipboard;
pub mod disk;
pub mod fp;
pub mod headless;
pub mod io;
pub mod pclink;
#[cfg(unix)]
pub mod raw_serial;
pub mod risc;

#[cfg(feature = "cosim")]
pub mod cosim;
