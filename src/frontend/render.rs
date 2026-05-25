//! Framebuffer rendering: 1-bit -> ARGB expansion with damage tracking, plus a
//! nearest-neighbour scale into the window (port of `update_texture` and
//! `scale_display` from `sdl-main.c`).
//!
//! Phase-1 strategy (per the plan): keep a persistent native-resolution `u32`
//! texture (the SDL streaming-texture analog), refresh only the damaged words
//! into it, then nearest-neighbour scale the whole texture into the softbuffer
//! surface every present.

use crate::risc::Risc;

/// Solarized "off"/"on" colours used by the C frontend. softbuffer wants
/// `0x00RRGGBB`, which these already are.
pub const BLACK: u32 = 0x0065_7B83;
pub const WHITE: u32 = 0x00FD_F6E3;

/// Letterbox colour for window area outside the display (SDL clears to black).
const BORDER: u32 = 0x0000_0000;

/// Placement of the scaled framebuffer inside the window.
#[derive(Clone, Copy, Debug)]
pub struct DisplayRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub scale: f64,
}

/// Centered, aspect-preserving placement of an `oberon_w` x `oberon_h`
/// framebuffer inside a `win_w` x `win_h` window (port of `scale_display`).
pub fn scale_display(win_w: u32, win_h: u32, oberon_w: u32, oberon_h: u32) -> DisplayRect {
    let oberon_aspect = oberon_w as f64 / oberon_h as f64;
    let window_aspect = win_w as f64 / win_h as f64;
    let scale = if oberon_aspect > window_aspect {
        win_w as f64 / oberon_w as f64
    } else {
        win_h as f64 / oberon_h as f64
    };
    let w = (oberon_w as f64 * scale).ceil() as i32;
    let h = (oberon_h as f64 * scale).ceil() as i32;
    DisplayRect { w, h, x: (win_w as i32 - w) / 2, y: (win_h as i32 - h) / 2, scale }
}

/// Refresh the persistent native-resolution `texture` from the framebuffer's
/// damaged region, expanding each 1-bit word into 32 ARGB pixels (LSB =
/// leftmost) and flipping the Oberon bottom-up framebuffer to top-down.
pub fn blit_damage(texture: &mut [u32], risc: &mut Risc, black: u32, white: u32) {
    let damage = risc.framebuffer_damage();
    if damage.y1 > damage.y2 {
        return;
    }
    let fb_width = risc.fb_width(); // words per line
    let fb_height = risc.fb_height();
    let tex_w = (fb_width * 32) as usize;
    let fb = risc.framebuffer();

    for line in damage.y1..=damage.y2 {
        let src_row = (line * fb_width) as usize;
        let dst_row = (fb_height - 1 - line) as usize * tex_w; // Y-flip
        for col in damage.x1..=damage.x2 {
            let mut pixels = fb[src_row + col as usize];
            let base = dst_row + col as usize * 32;
            for px in &mut texture[base..base + 32] {
                *px = if pixels & 1 != 0 { white } else { black };
                pixels >>= 1;
            }
        }
    }
}

/// Nearest-neighbour scale the whole `texture` into the window-sized `out`
/// buffer, letterboxing the surrounding border.
pub fn scale_into(
    out: &mut [u32],
    win_w: u32,
    win_h: u32,
    texture: &[u32],
    tex_w: usize,
    tex_h: usize,
    rect: DisplayRect,
) {
    let inv = 1.0 / rect.scale;
    let win_w = win_w as usize;

    // Precompute the source column for each window column once.
    let col_tx: Vec<Option<usize>> = (0..win_w as i32)
        .map(|sx| {
            if sx >= rect.x && sx < rect.x + rect.w {
                let tx = ((sx - rect.x) as f64 * inv) as usize;
                (tx < tex_w).then_some(tx)
            } else {
                None
            }
        })
        .collect();

    for sy in 0..win_h as i32 {
        let out_row = &mut out[sy as usize * win_w..sy as usize * win_w + win_w];
        if sy < rect.y || sy >= rect.y + rect.h {
            out_row.fill(BORDER);
            continue;
        }
        let ty = ((sy - rect.y) as f64 * inv) as usize;
        if ty >= tex_h {
            out_row.fill(BORDER);
            continue;
        }
        let tex_row = &texture[ty * tex_w..ty * tex_w + tex_w];
        for (px, tx) in out_row.iter_mut().zip(&col_tx) {
            *px = match tx {
                Some(tx) => tex_row[*tx],
                None => BORDER,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_display_letterboxes_wide_window() {
        // 4:3 framebuffer in a 2:1 window -> fit to height, centered horizontally.
        let r = scale_display(2000, 768, 1024, 768);
        assert_eq!(r.scale, 1.0);
        assert_eq!((r.w, r.h), (1024, 768));
        assert_eq!(r.x, (2000 - 1024) / 2);
        assert_eq!(r.y, 0);
    }

    #[test]
    fn scale_display_integer_zoom_fills() {
        let r = scale_display(2048, 1536, 1024, 768);
        assert_eq!(r.scale, 2.0);
        assert_eq!((r.x, r.y, r.w, r.h), (0, 0, 2048, 1536));
    }

    #[test]
    fn scale_into_1x_copies_and_borders() {
        let tex_w = 4;
        let tex_h = 2;
        let texture = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let (win_w, win_h) = (6u32, 2u32);
        let rect = scale_display(win_w, win_h, tex_w as u32, tex_h as u32);
        // 4:2 (2:1) in 6:2 (3:1) window -> fit to height, scale 1.0, x offset 1.
        assert_eq!((rect.scale, rect.x, rect.w), (1.0, 1, 4));
        let mut out = vec![0xDEADu32; (win_w * win_h) as usize];
        scale_into(&mut out, win_w, win_h, &texture, tex_w, tex_h, rect);
        // Row 0: border, tex row0 (1..4), border.
        assert_eq!(out[0], BORDER);
        assert_eq!(&out[1..5], &[1, 2, 3, 4]);
        assert_eq!(out[5], BORDER);
        // Row 1: tex row1 (5..8).
        assert_eq!(&out[7..11], &[5, 6, 7, 8]);
    }
}
