//! Translate winit physical key codes to PS/2 code-set-2 scancodes (port of
//! `sdl-ps2.c`, re-keyed from `SDL_Scancode` to winit's `KeyCode`).
//!
//! Every output byte and type-tag is preserved verbatim. The one live-state
//! dependency is the "shift hack" (keypad `/`), which the C reads via
//! `SDL_GetModState`; winit has no in-handler query, so the caller passes the
//! [`ModifiersState`] it tracks from `ModifiersChanged`.

use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};

/// Maximum number of bytes emitted for a single key event.
pub const MAX_PS2_CODE_LEN: usize = 8;

#[derive(Clone, Copy)]
enum KType {
    Normal,
    Extended,
    NumlockHack,
    ShiftHack,
}

/// PS/2 scancode + emission style for a physical key, or `None` for keys Oberon
/// does not use (the C's `K_UNKNOWN`).
fn lookup(key: KeyCode) -> Option<(u8, KType)> {
    use KType::{Extended, Normal, NumlockHack, ShiftHack};
    use KeyCode as K;
    Some(match key {
        K::KeyA => (0x1C, Normal),
        K::KeyB => (0x32, Normal),
        K::KeyC => (0x21, Normal),
        K::KeyD => (0x23, Normal),
        K::KeyE => (0x24, Normal),
        K::KeyF => (0x2B, Normal),
        K::KeyG => (0x34, Normal),
        K::KeyH => (0x33, Normal),
        K::KeyI => (0x43, Normal),
        K::KeyJ => (0x3B, Normal),
        K::KeyK => (0x42, Normal),
        K::KeyL => (0x4B, Normal),
        K::KeyM => (0x3A, Normal),
        K::KeyN => (0x31, Normal),
        K::KeyO => (0x44, Normal),
        K::KeyP => (0x4D, Normal),
        K::KeyQ => (0x15, Normal),
        K::KeyR => (0x2D, Normal),
        K::KeyS => (0x1B, Normal),
        K::KeyT => (0x2C, Normal),
        K::KeyU => (0x3C, Normal),
        K::KeyV => (0x2A, Normal),
        K::KeyW => (0x1D, Normal),
        K::KeyX => (0x22, Normal),
        K::KeyY => (0x35, Normal),
        K::KeyZ => (0x1A, Normal),

        K::Digit1 => (0x16, Normal),
        K::Digit2 => (0x1E, Normal),
        K::Digit3 => (0x26, Normal),
        K::Digit4 => (0x25, Normal),
        K::Digit5 => (0x2E, Normal),
        K::Digit6 => (0x36, Normal),
        K::Digit7 => (0x3D, Normal),
        K::Digit8 => (0x3E, Normal),
        K::Digit9 => (0x46, Normal),
        K::Digit0 => (0x45, Normal),

        K::Enter => (0x5A, Normal),
        K::Escape => (0x76, Normal),
        K::Backspace => (0x66, Normal),
        K::Tab => (0x0D, Normal),
        K::Space => (0x29, Normal),

        K::Minus => (0x4E, Normal),
        K::Equal => (0x55, Normal),
        K::BracketLeft => (0x54, Normal),
        K::BracketRight => (0x5B, Normal),
        K::Backslash => (0x5D, Normal),

        K::Semicolon => (0x4C, Normal),
        K::Quote => (0x52, Normal),
        K::Backquote => (0x0E, Normal),
        K::Comma => (0x41, Normal),
        K::Period => (0x49, Normal),
        K::Slash => (0x4A, Normal),

        K::F1 => (0x05, Normal),
        K::F2 => (0x06, Normal),
        K::F3 => (0x04, Normal),
        K::F4 => (0x0C, Normal),
        K::F5 => (0x03, Normal),
        K::F6 => (0x0B, Normal),
        K::F7 => (0x83, Normal),
        K::F8 => (0x0A, Normal),
        K::F9 => (0x01, Normal),
        K::F10 => (0x09, Normal),
        K::F11 => (0x78, Normal),
        K::F12 => (0x07, Normal),

        // Mostly unused by Oberon; the numlock hack assumes Num Lock is active.
        K::Insert => (0x70, NumlockHack),
        K::Home => (0x6C, NumlockHack),
        K::PageUp => (0x7D, NumlockHack),
        K::Delete => (0x71, NumlockHack),
        K::End => (0x69, NumlockHack),
        K::PageDown => (0x7A, NumlockHack),
        K::ArrowRight => (0x74, NumlockHack),
        K::ArrowLeft => (0x6B, NumlockHack),
        K::ArrowDown => (0x72, NumlockHack),
        K::ArrowUp => (0x75, NumlockHack),

        K::NumpadDivide => (0x4A, ShiftHack),
        K::NumpadMultiply => (0x7C, Normal),
        K::NumpadSubtract => (0x7B, Normal),
        K::NumpadAdd => (0x79, Normal),
        K::NumpadEnter => (0x5A, Extended),
        K::Numpad1 => (0x69, Normal),
        K::Numpad2 => (0x72, Normal),
        K::Numpad3 => (0x7A, Normal),
        K::Numpad4 => (0x6B, Normal),
        K::Numpad5 => (0x73, Normal),
        K::Numpad6 => (0x74, Normal),
        K::Numpad7 => (0x6C, Normal),
        K::Numpad8 => (0x75, Normal),
        K::Numpad9 => (0x7D, Normal),
        K::Numpad0 => (0x70, Normal),
        K::NumpadDecimal => (0x71, Normal),

        K::IntlBackslash => (0x61, Normal),
        K::ContextMenu => (0x2F, Extended),

        K::ControlLeft => (0x14, Normal),
        K::ShiftLeft => (0x12, Normal),
        K::AltLeft => (0x11, Normal),
        K::SuperLeft => (0x1F, Extended),
        K::ControlRight => (0x14, Extended),
        K::ShiftRight => (0x59, Normal),
        K::AltRight => (0x11, Extended),
        K::SuperRight => (0x27, Extended),

        _ => return None,
    })
}

/// Encode a key make (`true`) / break (`false`) into PS/2 set-2 bytes (port of
/// `ps2_encode`). Returns the buffer and the number of valid bytes; `0` means
/// "emit nothing".
pub fn encode(
    key: PhysicalKey,
    make: bool,
    mods: ModifiersState,
) -> ([u8; MAX_PS2_CODE_LEN], usize) {
    let mut out = [0u8; MAX_PS2_CODE_LEN];
    let mut i = 0;
    macro_rules! push {
        ($b:expr) => {{
            out[i] = $b;
            i += 1;
        }};
    }

    let PhysicalKey::Code(code) = key else {
        return (out, 0); // Unidentified -> nothing
    };
    let Some((scancode, ty)) = lookup(code) else {
        return (out, 0);
    };

    match ty {
        KType::Normal => {
            if !make {
                push!(0xF0);
            }
            push!(scancode);
        }
        KType::Extended => {
            push!(0xE0);
            if !make {
                push!(0xF0);
            }
            push!(scancode);
        }
        KType::NumlockHack => {
            if make {
                // Fake shift press around the make.
                push!(0xE0);
                push!(0x12);
                push!(0xE0);
                push!(scancode);
            } else {
                push!(0xE0);
                push!(0xF0);
                push!(scancode);
                // Fake shift release.
                push!(0xE0);
                push!(0xF0);
                push!(0x12);
            }
        }
        KType::ShiftHack => {
            // The C distinguishes left/right shift; winit's ModifiersState is
            // combined, so we use the left-shift (0x12) variant when any shift
            // is held (keypad-/ is effectively unused by Oberon).
            let shift = mods.shift_key();
            if make {
                if shift {
                    push!(0xE0);
                    push!(0xF0);
                    push!(0x12);
                }
                push!(0xE0);
                push!(scancode);
            } else {
                push!(0xE0);
                push!(0xF0);
                push!(scancode);
                if shift {
                    push!(0xE0);
                    push!(0x12);
                }
            }
        }
    }
    (out, i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::{KeyCode, NativeKeyCode};

    fn enc(key: KeyCode, make: bool, mods: ModifiersState) -> Vec<u8> {
        let (buf, n) = encode(PhysicalKey::Code(key), make, mods);
        buf[..n].to_vec()
    }

    #[test]
    fn normal_make_and_break() {
        let m = ModifiersState::empty();
        assert_eq!(enc(KeyCode::KeyA, true, m), vec![0x1C]);
        assert_eq!(enc(KeyCode::KeyA, false, m), vec![0xF0, 0x1C]);
    }

    #[test]
    fn extended_make_and_break() {
        let m = ModifiersState::empty();
        assert_eq!(enc(KeyCode::NumpadEnter, true, m), vec![0xE0, 0x5A]);
        assert_eq!(enc(KeyCode::NumpadEnter, false, m), vec![0xE0, 0xF0, 0x5A]);
    }

    #[test]
    fn numlock_hack() {
        let m = ModifiersState::empty();
        assert_eq!(enc(KeyCode::ArrowUp, true, m), vec![0xE0, 0x12, 0xE0, 0x75]);
        assert_eq!(
            enc(KeyCode::ArrowUp, false, m),
            vec![0xE0, 0xF0, 0x75, 0xE0, 0xF0, 0x12]
        );
    }

    #[test]
    fn shift_hack_depends_on_modifier() {
        let none = ModifiersState::empty();
        assert_eq!(enc(KeyCode::NumpadDivide, true, none), vec![0xE0, 0x4A]);
        let shift = ModifiersState::SHIFT;
        assert_eq!(
            enc(KeyCode::NumpadDivide, true, shift),
            vec![0xE0, 0xF0, 0x12, 0xE0, 0x4A]
        );
        assert_eq!(
            enc(KeyCode::NumpadDivide, false, shift),
            vec![0xE0, 0xF0, 0x4A, 0xE0, 0x12]
        );
    }

    #[test]
    fn unmapped_keys_emit_nothing() {
        let m = ModifiersState::empty();
        assert_eq!(enc(KeyCode::Pause, true, m), Vec::<u8>::new());
        let (_, n) = encode(
            PhysicalKey::Unidentified(NativeKeyCode::Unidentified),
            true,
            m,
        );
        assert_eq!(n, 0);
    }
}
