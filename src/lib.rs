//! `oberon-risc-emu`: pixel-scaling helpers for the windowed frontend, exposed
//! as a library only so the `render` benchmark — a separate Cargo target, which
//! can link a library but not a binary — can reach them. The actual `risc`
//! frontend (window, clock loop, CLI, input, clipboard) lives in the binary,
//! `src/main.rs` and its sibling modules; the pure machine is in the
//! [`risc_core`] crate.

#![deny(unsafe_code)]

// `pub` for the `render` benchmark only (see the crate docs); not a real API.
#[doc(hidden)]
pub mod render;
