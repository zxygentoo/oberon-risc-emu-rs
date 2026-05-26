//! A faithful Rust port of Peter De Wachter's [`oberon-risc-emu`], a standalone
//! emulator for Niklaus Wirth's Project Oberon RISC5 machine.
//!
//! This is a structurally-1:1 port of the C reference: each module corresponds
//! to one C source file so behaviour can be diffed file-by-file. See `plan.md`.
//!
//! The pure-`std` core (CPU, FP, MMIO, disk, devices) lives in the library and
//! has no external dependencies. The windowing/render/input frontend is gated
//! behind the default-on `frontend` feature.
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
pub mod io;
pub mod risc;
pub mod serial;

#[cfg(feature = "frontend")]
pub mod error;

#[cfg(feature = "frontend")]
pub mod frontend;

#[cfg(feature = "cosim")]
pub mod cosim;
