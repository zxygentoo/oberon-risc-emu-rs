//! Typed errors for the emulator frontend.
//!
//! The pure-core devices keep returning `std::io::Result` (a file-backed disk or
//! serial line has no richer failure mode), so this aggregate [`enum@Error`] is what
//! the windowed `risc` binary surfaces to the user — its [`Error::Window`] variant
//! wraps a `winit` error. The dependency-light machine itself lives in the
//! separate `risc-core` crate.

use thiserror::Error;

/// An error raised while building or running the emulator frontend.
#[derive(Debug, Error)]
pub enum Error {
    /// A file/device I/O failure: opening the disk image or a raw serial line.
    /// `std::io::Result`s from the core fold in here via `?`.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Invalid command-line options or display/memory configuration.
    #[error("{0}")]
    Config(String),

    /// The windowing system / event loop failed to start or run.
    #[error("window system error: {0}")]
    Window(#[from] winit::error::EventLoopError),
}

/// Convenience alias for fallible frontend operations.
pub type Result<T> = std::result::Result<T, Error>;
