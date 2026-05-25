//! Window input handling: mouse motion/buttons, modifier tracking, hotkeys, and
//! PS/2 keyboard forwarding (port of `sdl-main.c`'s event loop).

use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};

use super::{ps2, App};

/// Dispatch a window event that `mod.rs` didn't consume itself.
pub(super) fn handle(app: &mut App, event_loop: &ActiveEventLoop, event: WindowEvent) {
    match event {
        WindowEvent::ModifiersChanged(mods) => app.modifiers = mods.state(),

        WindowEvent::CursorMoved { position, .. } => {
            // Map window pixel -> Oberon framebuffer pixel through the display
            // rect, clamp, hide the cursor when it strays into the letterbox,
            // and invert Y (Oberon's origin is bottom-left).
            let scaled_x = ((position.x - app.rect.x as f64) / app.rect.scale).round() as i32;
            let scaled_y = ((position.y - app.rect.y as f64) / app.rect.scale).round() as i32;
            let w = app.tex_w as i32;
            let h = app.tex_h as i32;
            let x = scaled_x.clamp(0, w - 1);
            let y = scaled_y.clamp(0, h - 1);
            let offscreen = x != scaled_x || y != scaled_y;
            if offscreen != app.mouse_offscreen {
                if let Some(win) = &app.window {
                    win.set_cursor_visible(offscreen);
                }
                app.mouse_offscreen = offscreen;
            }
            app.risc.mouse_moved(x, h - y - 1);
        }

        WindowEvent::MouseInput { state, button, .. } => {
            let down = state == ElementState::Pressed;
            let n = match button {
                MouseButton::Left => 1,
                MouseButton::Middle => 2,
                MouseButton::Right => 3,
                _ => return,
            };
            app.risc.mouse_button(n, down);
        }

        WindowEvent::KeyboardInput { event, .. } => {
            let make = event.state == ElementState::Pressed;
            handle_key(app, event_loop, event.physical_key, make);
        }

        _ => {}
    }
}

enum Action {
    Quit,
    Reset,
    ToggleFullscreen,
}

fn handle_key(app: &mut App, event_loop: &ActiveEventLoop, key: PhysicalKey, make: bool) {
    let mods = app.modifiers;

    // Left Alt emulates the middle mouse button on both press and release;
    // it never reaches Oberon as a key.
    if key == PhysicalKey::Code(KeyCode::AltLeft) {
        app.risc.mouse_button(2, make);
        return;
    }

    // Hotkeys fire on make only (mirroring the C key map); the matching break
    // falls through to the keyboard.
    if make {
        let action = match key {
            PhysicalKey::Code(KeyCode::F4) if mods.alt_key() => Some(Action::Quit),
            PhysicalKey::Code(KeyCode::F12) => Some(Action::Reset),
            PhysicalKey::Code(KeyCode::Delete) if mods.control_key() && mods.shift_key() => {
                Some(Action::Reset)
            }
            PhysicalKey::Code(KeyCode::F11) => Some(Action::ToggleFullscreen),
            PhysicalKey::Code(KeyCode::Enter) if mods.alt_key() => Some(Action::ToggleFullscreen),
            PhysicalKey::Code(KeyCode::KeyF) if mods.super_key() && mods.shift_key() => {
                Some(Action::ToggleFullscreen)
            }
            _ => None,
        };
        if let Some(action) = action {
            match action {
                Action::Quit => event_loop.exit(),
                Action::Reset => app.risc.reset(),
                Action::ToggleFullscreen => app.toggle_fullscreen(),
            }
            return;
        }
    }

    let (bytes, len) = ps2::encode(key, make, mods);
    if len > 0 {
        app.risc.keyboard_input(&bytes[..len]);
    }
}
