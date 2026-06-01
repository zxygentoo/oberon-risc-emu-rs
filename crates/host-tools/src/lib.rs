//! Shared host-side helpers for working with Project Oberon artifacts.
//!
//! The tools in this crate are separate `src/bin/` programs, but the heavy lifting
//! they have in common lives here, so there is a single source of truth (and one
//! set of tests) rather than a copy per binary that can quietly drift: the
//! compile-order [`resolve`]r, the disk-image build [`pipeline`], the [`image`] disk
//! reader, and the [`packonly`] manifest format. The headless shim runtime they
//! drive lives in [`risc_core::shim`]. What stays per-binary is just the embedded
//! toolchain seed and the CLI.

/// Decide which files of a source tree to compile (everything not in `.packonly`)
/// and in what order (topological sort of their `IMPORT` lists). Shared by
/// `build-po-image` and `build-eo-image`, which compile a whole Oberon source tree.
pub mod resolve;

/// The disk-image build pipeline shared by `build-po-image` and `build-eo-image`:
/// compile a source tree against an embedded toolchain, link a fresh inner core,
/// and assemble a bootable `Oberon.dsk`. Each binary supplies only its [`pipeline::Seed`].
pub mod pipeline;

/// A read-only reader for the Project Oberon on-disk filesystem — the inverse of the
/// image builders: parse a `.dsk`/`RISC.img` straight into its files, no emulator.
/// Used by `extract-source`.
pub mod image;

/// The `.packonly` manifest format (parse + render): the files a source tree packs
/// into the image verbatim rather than compiling. Shared by [`resolve`] (reads it,
/// to pick compile candidates) and `extract-source` (writes it).
pub mod packonly;
