//! `risc-core`: a faithful, bit-exact Rust port of Peter De Wachter's
//! [`oberon-risc-emu`] — the RISC5 machine (CPU, software FP, MMIO, disk, serial,
//! clipboard bridge) with no windowing or platform UI — plus the headless [`shim`]
//! that drives the same CPU for the host build toolchain.
//!
//! The machine modules are a structurally-1:1 port of the C reference (each maps to
//! one C source file). The [`shim`] runs the CPU in a second mode — the whole MMIO
//! region routed to host file syscalls + stdio (a port of project-norebo's
//! `norebo.c`), booting an `InnerCore` rather than the boot ROM — so an Oberon
//! source tree can be compiled on the host with no emulated disk or display.
//!
//! `std` but otherwise dependency-light, so it can back a winit frontend, a headless
//! runner, or — in future — wasm/libretro. The windowed `oberon-risc-emu` crate is
//! the reference consumer; [`shim::run`] is the toolchain entry point.
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
pub mod shim;

#[cfg(feature = "cosim")]
pub mod cosim;

/// Whether byte `ch` is legal at 0-based position `i` of a Project Oberon file
/// name: a leading ASCII letter, then letters, digits, or `.` (the `FileDir.Mod`
/// rule, mirrored by `norebo.c`'s `files_check_name`). Used by the [`shim`] syscall
/// ABI here, and by the host-side Oberon filesystem reader (`extract-source`).
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
