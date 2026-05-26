//! Microbenchmark for the per-frame render hot path: the bilinear rescale.
//!
//! `scale_into` is the worst case (a full-window rescale, only needed on resize
//! / first paint); `scale_region` is the steady-state path — it rescales only
//! the window span the framebuffer damage maps to. Run with `cargo bench`. Plain
//! `std` timing, no bench harness crate, so it adds no dependencies.

use std::hint::black_box;
use std::time::Instant;

use oberon_risc_emu::frontend::render::{
    scale_display, scale_into, scale_region, window_dirty, PixelRect, BLACK, WHITE,
};

fn texture(tex_w: usize, tex_h: usize) -> Vec<u32> {
    // Varied content so the lerps aren't trivially predictable.
    (0..tex_w * tex_h)
        .map(|i| {
            if (i / 7 + i / 131) & 1 == 0 {
                WHITE
            } else {
                BLACK
            }
        })
        .collect()
}

fn report(label: &str, win_w: u32, win_h: u32, per: std::time::Duration) {
    let fps_ceiling = 1e9 / per.as_nanos() as f64;
    println!(
        "  {label:<26} {win_w}x{win_h}  {:>7.3} ms/frame  (~{fps_ceiling:>6.0} fps ceiling)",
        per.as_secs_f64() * 1000.0
    );
}

/// Full-window rescale (resize / first paint).
fn bench_full(label: &str, win_w: u32, win_h: u32, tex_w: usize, tex_h: usize, iters: u32) {
    let tex = texture(tex_w, tex_h);
    let rect = scale_display(win_w, win_h, tex_w as u32, tex_h as u32);
    let mut out = vec![0u32; (win_w * win_h) as usize];
    scale_into(&mut out, win_w, win_h, &tex, tex_w, tex_h, rect); // warm up
    let start = Instant::now();
    for _ in 0..iters {
        scale_into(&mut out, win_w, win_h, &tex, tex_w, tex_h, rect);
        black_box(&out);
    }
    report(label, win_w, win_h, start.elapsed() / iters);
}

/// Steady-state: rescale only the span a small framebuffer damage maps to.
fn bench_span(label: &str, win_w: u32, win_h: u32, tex_w: usize, tex_h: usize, iters: u32) {
    let tex = texture(tex_w, tex_h);
    let rect = scale_display(win_w, win_h, tex_w as u32, tex_h as u32);
    let mut out = vec![0u32; (win_w * win_h) as usize];
    scale_into(&mut out, win_w, win_h, &tex, tex_w, tex_h, rect); // start from a full frame
                                                                  // A typical update: ~a word of text near the middle of the screen.
    let damage = PixelRect {
        x0: tex_w / 3,
        y0: tex_h / 2,
        x1: tex_w / 3 + 160,
        y1: tex_h / 2 + 24,
    };
    let wd = window_dirty(damage, rect, win_w, win_h);
    let start = Instant::now();
    for _ in 0..iters {
        scale_region(&mut out, win_w, win_h, &tex, tex_w, tex_h, rect, wd);
        black_box(&out);
    }
    report(label, win_w, win_h, start.elapsed() / iters);
}

fn main() {
    let (w, h) = (1024usize, 768usize);
    println!("1-bit {w}x{h} framebuffer -> window (60 fps budget = 16.67 ms/frame)");

    println!("full rescale (resize / first paint):");
    bench_full("1x", w as u32, h as u32, w, h, 500);
    bench_full(
        "2x (default auto-zoom)",
        (w * 2) as u32,
        (h * 2) as u32,
        w,
        h,
        300,
    );
    bench_full("3840x2160 (4K fullscreen)", 3840, 2160, w, h, 200);

    println!("damaged-span rescale (steady state, ~a word of text):");
    bench_span("1x", w as u32, h as u32, w, h, 5000);
    bench_span(
        "2x (default auto-zoom)",
        (w * 2) as u32,
        (h * 2) as u32,
        w,
        h,
        5000,
    );
    bench_span("3840x2160 (4K fullscreen)", 3840, 2160, w, h, 5000);
}
