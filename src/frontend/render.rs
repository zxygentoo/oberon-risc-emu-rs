//! Framebuffer rendering: 1-bit -> ARGB expansion with damage tracking, plus a
//! bilinear scale into the window (port of `update_texture` and `scale_display`
//! from `sdl-main.c`).
//!
//! Strategy: keep a persistent native-resolution `u32` texture (the SDL
//! streaming-texture analog) and refresh only the damaged words into it. The
//! bilinear scale (matching SDL's "best"/linear filter, far gentler on 1-bit
//! text than nearest-neighbour) is the per-frame hot path, so it runs only over
//! the window region the damage maps to ([`window_dirty`] + [`scale_region`]),
//! into a persistent window-sized buffer the caller then copies to the surface.
//! A full rescale ([`scale_into`]) is only needed on resize.

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

/// A pixel rectangle `[x0, x1) x [y0, y1)` (exclusive upper bounds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
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
/// leftmost) and flipping the Oberon bottom-up framebuffer to top-down. Returns
/// the touched texture-pixel rectangle, or `None` if nothing was damaged.
pub fn blit_damage(
    texture: &mut [u32],
    risc: &mut Risc,
    black: u32,
    white: u32,
) -> Option<PixelRect> {
    let damage = risc.framebuffer_damage();
    if damage.y1 > damage.y2 {
        return None;
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

    // Texture-pixel bounds of the touched region (Y-flipped from line rows).
    Some(PixelRect {
        x0: (damage.x1 * 32) as usize,
        x1: ((damage.x2 + 1) * 32) as usize,
        y0: (fb_height - 1 - damage.y2) as usize,
        y1: (fb_height - damage.y1) as usize,
    })
}

/// Map a damaged texture-pixel rect to the window-pixel rect that must be
/// re-scaled: widen by one source pixel each way (so the bilinear taps of the
/// edge output pixels are covered), map through the display rect, and clamp to
/// the window. Pixels falling in the letterbox are handled as `BORDER` by
/// [`scale_region`].
pub fn window_dirty(tex: PixelRect, rect: DisplayRect, win_w: u32, win_h: u32) -> PixelRect {
    let to_win = |t: usize, off: i32| off as f64 + t as f64 * rect.scale;
    let clamp = |v: f64, max: u32| v.clamp(0.0, f64::from(max)) as usize;
    PixelRect {
        x0: clamp(to_win(tex.x0.saturating_sub(1), rect.x).floor(), win_w),
        y0: clamp(to_win(tex.y0.saturating_sub(1), rect.y).floor(), win_h),
        x1: clamp(to_win(tex.x1 + 1, rect.x).ceil(), win_w),
        y1: clamp(to_win(tex.y1 + 1, rect.y).ceil(), win_h),
    }
}

/// Bilinearly scale the whole `texture` into the window-sized `out` buffer,
/// letterboxing the border. Used for the initial paint and after a resize.
pub fn scale_into(
    out: &mut [u32],
    win_w: u32,
    win_h: u32,
    texture: &[u32],
    tex_w: usize,
    tex_h: usize,
    rect: DisplayRect,
) {
    let full = PixelRect {
        x0: 0,
        y0: 0,
        x1: win_w as usize,
        y1: win_h as usize,
    };
    scale_region(out, win_w, win_h, texture, tex_w, tex_h, rect, full);
}

/// Bilinearly scale `texture` into the `dirty` window-pixel rect of `out`,
/// leaving the rest of `out` untouched. This matches the linear ("best")
/// filtering the C frontend asks SDL for. With `dirty` covering the whole
/// window this is identical to a full rescale; restricting it to the damaged
/// span is what keeps the per-frame cost off the full-window bilinear.
// A leaf pixel routine: window/texture dimensions, placement, and the dirty
// rect are all genuinely independent inputs, so the arg count is inherent.
#[allow(clippy::too_many_arguments)]
pub fn scale_region(
    out: &mut [u32],
    win_w: u32,
    win_h: u32,
    texture: &[u32],
    tex_w: usize,
    tex_h: usize,
    rect: DisplayRect,
    dirty: PixelRect,
) {
    let win_w = win_w as usize;
    let win_h = win_h as usize;
    let x0 = dirty.x0.min(win_w);
    let x1 = dirty.x1.min(win_w);
    let y0 = dirty.y0.min(win_h);
    let y1 = dirty.y1.min(win_h);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let inv = 1.0 / rect.scale;

    // Per dirty window column: the two source columns and the horizontal blend
    // weight (0..256 fixed point), or None outside the display rect.
    let cols: Vec<Option<(usize, usize, u32)>> = (x0..x1)
        .map(|sx| {
            let sx = sx as i32;
            if sx < rect.x || sx >= rect.x + rect.w {
                return None;
            }
            let fx = (sx - rect.x) as f64 * inv;
            let cx0 = (fx as usize).min(tex_w - 1);
            let cx1 = (cx0 + 1).min(tex_w - 1);
            Some((cx0, cx1, ((fx - cx0 as f64) * 256.0) as u32))
        })
        .collect();

    for sy in y0..y1 {
        let out_row = &mut out[sy * win_w + x0..sy * win_w + x1];
        let syi = sy as i32;
        if syi < rect.y || syi >= rect.y + rect.h {
            out_row.fill(BORDER);
            continue;
        }
        let fy = (syi - rect.y) as f64 * inv;
        let ry0 = (fy as usize).min(tex_h - 1);
        let ry1 = (ry0 + 1).min(tex_h - 1);
        let wy = ((fy - ry0 as f64) * 256.0) as u32;
        let row0 = &texture[ry0 * tex_w..ry0 * tex_w + tex_w];
        let row1 = &texture[ry1 * tex_w..ry1 * tex_w + tex_w];

        for (px, col) in out_row.iter_mut().zip(&cols) {
            *px = match *col {
                Some((cx0, cx1, wx)) => {
                    let top = lerp(row0[cx0], row0[cx1], wx);
                    let bot = lerp(row1[cx0], row1[cx1], wx);
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

    // Build a varied texture so the bilinear weights actually differ per pixel.
    fn sample_texture(tex_w: usize, tex_h: usize) -> Vec<u32> {
        (0..tex_w * tex_h)
            .map(|i| {
                let v = (i * 37 % 251) as u32;
                (v << 16) | ((v ^ 0x5A) << 8) | (v.wrapping_mul(3) & 0xFF)
            })
            .collect()
    }

    #[test]
    fn scale_region_full_window_equals_scale_into() {
        // Region scaling over the whole window must reproduce a full rescale.
        let (tex_w, tex_h) = (16usize, 12usize);
        let texture = sample_texture(tex_w, tex_h);
        let (win_w, win_h) = (40u32, 30u32); // non-integer scale + letterbox
        let rect = scale_display(win_w, win_h, tex_w as u32, tex_h as u32);

        let mut full = vec![0u32; (win_w * win_h) as usize];
        scale_into(&mut full, win_w, win_h, &texture, tex_w, tex_h, rect);

        let mut piece = vec![0u32; (win_w * win_h) as usize];
        let dirty = PixelRect {
            x0: 0,
            y0: 0,
            x1: win_w as usize,
            y1: win_h as usize,
        };
        scale_region(
            &mut piece, win_w, win_h, &texture, tex_w, tex_h, rect, dirty,
        );
        assert_eq!(piece, full);
    }

    #[test]
    fn scale_region_only_touches_its_rect_and_matches_full() {
        // Scaling a sub-rect must (a) leave other pixels untouched and (b) match
        // the full rescale within that rect — the property the damaged-span path
        // relies on.
        let (tex_w, tex_h) = (16usize, 12usize);
        let texture = sample_texture(tex_w, tex_h);
        let (win_w, win_h) = (48u32, 36u32);
        let rect = scale_display(win_w, win_h, tex_w as u32, tex_h as u32);

        let mut full = vec![0u32; (win_w * win_h) as usize];
        scale_into(&mut full, win_w, win_h, &texture, tex_w, tex_h, rect);

        let sentinel = 0xDEAD_BEEFu32;
        let mut piece = vec![sentinel; (win_w * win_h) as usize];
        let dirty = PixelRect {
            x0: 10,
            y0: 8,
            x1: 30,
            y1: 24,
        };
        scale_region(
            &mut piece, win_w, win_h, &texture, tex_w, tex_h, rect, dirty,
        );

        for sy in 0..win_h as usize {
            for sx in 0..win_w as usize {
                let i = sy * win_w as usize + sx;
                let inside = sx >= dirty.x0 && sx < dirty.x1 && sy >= dirty.y0 && sy < dirty.y1;
                if inside {
                    assert_eq!(
                        piece[i], full[i],
                        "rescaled pixel ({sx},{sy}) must match full"
                    );
                } else {
                    assert_eq!(
                        piece[i], sentinel,
                        "pixel ({sx},{sy}) outside rect was touched"
                    );
                }
            }
        }
    }

    #[test]
    fn window_dirty_bounds_every_changed_output_pixel() {
        // The optimisation's correctness hinges on this: when a texture region
        // changes, every output pixel whose value changes must lie within
        // window_dirty(that region). Otherwise a re-scale of only that span would
        // leave a stale pixel behind. Verify against a full before/after rescale.
        let (tex_w, tex_h) = (64usize, 48usize);
        let mut tex = sample_texture(tex_w, tex_h);
        let (win_w, win_h) = (200u32, 150u32); // deliberately non-integer scale
        let rect = scale_display(win_w, win_h, tex_w as u32, tex_h as u32);

        let mut before = vec![0u32; (win_w * win_h) as usize];
        scale_into(&mut before, win_w, win_h, &tex, tex_w, tex_h, rect);

        let dmg = PixelRect {
            x0: 20,
            y0: 15,
            x1: 31,
            y1: 23,
        };
        for ty in dmg.y0..dmg.y1 {
            for tx in dmg.x0..dmg.x1 {
                tex[ty * tex_w + tx] ^= 0x00FF_FFFF;
            }
        }
        let mut after = vec![0u32; (win_w * win_h) as usize];
        scale_into(&mut after, win_w, win_h, &tex, tex_w, tex_h, rect);

        let wd = window_dirty(dmg, rect, win_w, win_h);
        for sy in 0..win_h as usize {
            for sx in 0..win_w as usize {
                let i = sy * win_w as usize + sx;
                if before[i] != after[i] {
                    assert!(
                        sx >= wd.x0 && sx < wd.x1 && sy >= wd.y0 && sy < wd.y1,
                        "changed output pixel ({sx},{sy}) lies outside window_dirty {wd:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn window_dirty_covers_the_mapped_region_with_margin() {
        // A texture-pixel rect maps to a window rect that includes the scaled
        // image of the damage plus a one-source-pixel margin, clamped to window.
        let rect = scale_display(2048, 1536, 1024, 768); // 2x, no letterbox
        let wd = window_dirty(
            PixelRect {
                x0: 100,
                y0: 50,
                x1: 110,
                y1: 60,
            },
            rect,
            2048,
            1536,
        );
        // 2x: pixel 100 -> 200; widened by one source pixel (=2 window px) each way.
        assert!(wd.x0 <= 200 - 2 && wd.x1 >= 110 * 2 + 2);
        assert!(wd.y0 <= 100 - 2 && wd.y1 >= 60 * 2 + 2);
        assert!(wd.x1 <= 2048 && wd.y1 <= 1536);
    }
}
