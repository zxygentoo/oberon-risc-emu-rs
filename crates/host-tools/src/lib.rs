//! Shared host-side helpers for working with Project Oberon artifacts.
//!
//! The tools in this crate are separate `src/bin/` programs, but the heavy lifting
//! they have in common lives here, so there is a single source of truth (and one
//! set of tests) rather than a copy per binary that can quietly drift: the headless
//! [`shim`] runtime, the compile-order [`resolve`]r, the disk-[`image`] build
//! pipeline, the [`dsk`] image reader, the [`packonly`] manifest format, and the one
//! low-level Oberon rule below. What stays per-binary is just the embedded toolchain
//! seed and the CLI.

/// The headless `shim` runtime: boots an inner-core image on the `risc-core` CPU
/// and maps Oberon's `Kernel`/`Files` operations onto the host (a Rust port of
/// project-norebo). Used by the image builders, the EO bring-up, and `eo-inner-run`.
pub mod shim;

/// Decide which files of a source tree to compile (everything not in `.packonly`)
/// and in what order (topological sort of their `IMPORT` lists). Shared by
/// `build-po-image` and `build-eo-image`, which compile a whole Oberon source tree.
pub mod resolve;

/// The disk-image build pipeline shared by `build-po-image` and `build-eo-image`:
/// compile a source tree against an embedded toolchain, link a fresh inner core,
/// and assemble a bootable `Oberon.dsk`. Each binary supplies only its [`image::Seed`].
pub mod image;

/// A read-only reader for the Project Oberon on-disk filesystem — the inverse of the
/// image builders: parse a `.dsk`/`RISC.img` straight into its files, no emulator.
/// Used by `extract-source`.
pub mod dsk;

/// The `.packonly` manifest format (parse + render): the files a source tree packs
/// into the image verbatim rather than compiling. Shared by [`resolve`] (reads it,
/// to pick compile candidates) and `extract-source` (writes it).
pub mod packonly;

/// Whether byte `ch` is legal at 0-based position `i` of a Project Oberon file name:
/// a leading ASCII letter, then letters, digits, or `.` (the `FileDir.Mod` rule,
/// mirrored by `norebo.c`'s `files_check_name`). The one rule shared widely enough
/// to live at the crate root — used by [`shim`] (the syscall ABI) and [`dsk`] (the
/// directory reader).
///
/// This is the per-character predicate only; callers layer their own length and
/// termination rules on top (a NUL-terminated guest buffer, a fixed-width on-disk
/// field), so it intentionally says nothing about where a name ends.
pub fn name_char_ok(i: usize, ch: u8) -> bool {
    ch.is_ascii_alphabetic() || (i > 0 && (ch == b'.' || ch.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::name_char_ok;

    #[test]
    fn first_char_must_be_a_letter() {
        assert!(name_char_ok(0, b'K'));
        assert!(!name_char_ok(0, b'9')); // a digit may not lead
        assert!(!name_char_ok(0, b'.')); // nor a dot
    }

    #[test]
    fn later_chars_allow_letters_digits_and_dot() {
        assert!(name_char_ok(6, b'e'));
        assert!(name_char_ok(6, b'.'));
        assert!(name_char_ok(6, b'5'));
    }

    #[test]
    fn path_separators_are_never_legal() {
        assert!(!name_char_ok(0, b'/'));
        assert!(!name_char_ok(6, b'/'));
        assert!(!name_char_ok(6, b'-'));
    }
}
