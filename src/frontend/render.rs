//! Framebuffer rendering: 1-bit -> ARGB expansion with damage tracking, plus a
//! bilinear scale into the window (port of `update_texture` and `scale_display`
//! from `sdl-main.c`).
//!
//! Strategy (per the plan): keep a persistent native-resolution `u32` texture
//! (the SDL streaming-texture analog), refresh only the damaged words into it,
//! then scale the whole texture into the softbuffer surface every present. The
//! scale is bilinear to match the linear filtering SDL uses (`"best"` scale
//! quality), which looks far smoother than nearest-neighbour on the 1-bit text.

use risc_core::risc::Risc;

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
    DisplayRect {
        w,
        h,
        x: (win_w as i32 - w) / 2,
        y: (win_h as i32 - h) / 2,
        scale,
    }
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

/// Bilinearly scale the whole `texture` into the window-sized `out` buffer,
/// letterboxing the surrounding border. This matches the linear ("best")
/// filtering the C frontend asks SDL for, which is far gentler on the eye than
/// nearest-neighbour for the 1-bit display.
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

    // Per window column: the two source columns and the horizontal blend weight
    // (0..256 fixed point), or None outside the display rect. Computed once.
    let cols: Vec<Option<(usize, usize, u32)>> = (0..win_w as i32)
        .map(|sx| {
            if sx < rect.x || sx >= rect.x + rect.w {
                return None;
            }
            let fx = (sx - rect.x) as f64 * inv;
            let x0 = (fx as usize).min(tex_w - 1);
            let x1 = (x0 + 1).min(tex_w - 1);
            Some((x0, x1, ((fx - x0 as f64) * 256.0) as u32))
        })
        .collect();

    for sy in 0..win_h as i32 {
        let out_row = &mut out[sy as usize * win_w..sy as usize * win_w + win_w];
        if sy < rect.y || sy >= rect.y + rect.h {
            out_row.fill(BORDER);
            continue;
        }
        let fy = (sy - rect.y) as f64 * inv;
        let y0 = (fy as usize).min(tex_h - 1);
        let y1 = (y0 + 1).min(tex_h - 1);
        let wy = ((fy - y0 as f64) * 256.0) as u32;
        let row0 = &texture[y0 * tex_w..y0 * tex_w + tex_w];
        let row1 = &texture[y1 * tex_w..y1 * tex_w + tex_w];

        for (px, col) in out_row.iter_mut().zip(&cols) {
            *px = match *col {
                Some((x0, x1, wx)) => {
                    let top = lerp(row0[x0], row0[x1], wx);
                    let bot = lerp(row1[x0], row1[x1], wx);
                    lerp(top, bot, wy)
                }
                None => BORDER,
            };
        }
    }
}

/// Linearly interpolate the three 8-bit channels of two `0x00RRGGBB` pixels;
/// `t` is a 0..=256 fixed-point weight (`a` at 0, `b` at 256).
#[inline]
fn lerp(a: u32, b: u32, t: u32) -> u32 {
    let s = 256 - t;
    let mut out = 0;
    for shift in [0u32, 8, 16] {
        let ca = (a >> shift) & 0xFF;
        let cb = (b >> shift) & 0xFF;
        out |= ((ca * s + cb * t) >> 8) << shift;
    }
    out
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
    fn scale_into_1x_is_an_exact_copy() {
        // At integer source positions the blend weights are zero, so 1x scaling
        // reproduces the texture exactly.
        let tex_w = 4;
        let tex_h = 2;
        let texture = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let (win_w, win_h) = (6u32, 2u32);
        let rect = scale_display(win_w, win_h, tex_w as u32, tex_h as u32);
        // 4:2 (2:1) in 6:2 (3:1) window -> fit to height, scale 1.0, x offset 1.
        assert_eq!((rect.scale, rect.x, rect.w), (1.0, 1, 4));
        let mut out = vec![0xDEADu32; (win_w * win_h) as usize];
        scale_into(&mut out, win_w, win_h, &texture, tex_w, tex_h, rect);
        assert_eq!(out[0], BORDER);
        assert_eq!(&out[1..5], &[1, 2, 3, 4]);
        assert_eq!(out[5], BORDER);
        assert_eq!(&out[7..11], &[5, 6, 7, 8]);
    }

    #[test]
    fn scale_into_2x_blends_between_pixels() {
        // Two horizontal texels (black, blue=255) scaled 2x: the in-between
        // output column is their average.
        let texture = vec![0x0000_0000u32, 0x0000_00FF];
        let (win_w, win_h) = (4u32, 2u32);
        let rect = scale_display(win_w, win_h, 2, 1);
        assert_eq!((rect.scale, rect.x, rect.w), (2.0, 0, 4));
        let mut out = vec![0u32; (win_w * win_h) as usize];
        scale_into(&mut out, win_w, win_h, &texture, 2, 1, rect);
        assert_eq!(out[0], 0x0000_0000); // exact first texel
        assert_eq!(out[1], 0x0000_007F); // midpoint: 255/2 ~= 127
        assert_eq!(out[2], 0x0000_00FF); // exact second texel
    }

    #[test]
    fn lerp_endpoints_and_midpoint() {
        assert_eq!(lerp(0x00_00_00, 0x10_20_40, 0), 0x00_00_00); // t=0 -> a
        assert_eq!(lerp(0x00_00_00, 0x10_20_40, 256), 0x10_20_40); // t=256 -> b
        assert_eq!(lerp(0, 0xFF_FF_FF, 128), 0x7F_7F_7F); // half
    }
}
