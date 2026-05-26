//! Deterministic headless driver and state hashing — shared by the boot golden
//! tests and the `risc headless` subcommand.
//!
//! The synthetic 60 Hz clock (not wall time) makes a boot byte-for-byte
//! reproducible, which is exactly what the C-derived goldens and the live cosim
//! rely on.

use crate::risc::Risc;

/// The FPGA system clock the emulator models.
pub const CPU_HZ: u32 = 25_000_000;
/// Frames per second the frontend (and these helpers) pace the clock at.
pub const FPS: u32 = 60;

/// Advance `risc` by `frames`, driving the fixed 60 Hz synthetic clock the
/// frontend uses but independent of wall time, so the run is reproducible.
pub fn run_frames(risc: &mut Risc, frames: u32) {
    let frame_ms = 1000 / FPS;
    for frame in 0..frames {
        risc.set_time(frame.wrapping_mul(frame_ms));
        risc.run(CPU_HZ / FPS);
    }
}

/// FNV-1a over a stream of words (hashed as little-endian bytes).
pub fn fnv1a(words: impl IntoIterator<Item = u32>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for w in words {
        for b in w.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// FNV-1a of the active framebuffer (the visible `fb_width * fb_height` words).
pub fn framebuffer_hash(risc: &Risc) -> u64 {
    let words = (risc.fb_width() * risc.fb_height()) as usize;
    fnv1a(risc.framebuffer()[..words].iter().copied())
}

/// FNV-1a of the architectural CPU state (PC, R0..R15, H, flags).
pub fn state_hash(risc: &Risc) -> u64 {
    let s = risc.cpu_state();
    let flags = u32::from(s.flags.bits());
    fnv1a(std::iter::once(s.pc).chain(s.r).chain([s.h, flags]))
}
