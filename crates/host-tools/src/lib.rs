//! Shared host-side helpers for working with Project Oberon artifacts.
//!
//! The tools in this crate are separate `src/bin/` programs and deliberately
//! share little, but a few low-level Oberon rules must agree across them. Those
//! live here so there is a single source of truth (and one set of tests) rather
//! than a copy per binary that can quietly drift.

/// Whether byte `ch` is legal at 0-based position `i` of a Project Oberon file
/// name: a leading ASCII letter, then letters, digits, or `.` (the `FileDir.Mod`
/// rule, mirrored by `norebo.c`'s `files_check_name`).
///
/// This is the per-character predicate only; callers layer their own length and
/// termination rules on top (a NUL-terminated guest buffer, a fixed-width
/// on-disk field), so it intentionally says nothing about where a name ends.
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
