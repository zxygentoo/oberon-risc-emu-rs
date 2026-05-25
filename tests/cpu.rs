//! Headless boot smoke test: drive the core against a real Oberon disk image
//! with a synthetic clock and assert the display framebuffer comes alive (i.e.
//! the desktop actually renders). Gated on the `OBERON_DISK` environment
//! variable (path to a `.dsk`); skipped when unset so default CI stays hermetic.

use oberon_risc_emu::disk::Disk;
use oberon_risc_emu::risc::Risc;

const CPU_HZ: u32 = 25_000_000;
const FPS: u32 = 60;

#[test]
fn boots_to_a_live_framebuffer() {
    let Ok(src) = std::env::var("OBERON_DISK") else {
        eprintln!("OBERON_DISK not set; skipping boot smoke test");
        return;
    };

    // Booting writes to the disk, so run against a throwaway copy.
    let mut tmp = std::env::temp_dir();
    tmp.push(format!("oberon_boot_smoke_{}.dsk", std::process::id()));
    std::fs::copy(&src, &tmp).expect("copy disk image");

    let mut risc = Risc::new();
    let disk = Disk::new(Some(&tmp)).expect("open disk image");
    risc.set_spi(1, Box::new(disk));

    // Deterministic synthetic 60 Hz clock (independent of wall time).
    let frame_ms = 1000 / FPS;
    for frame in 0..600u32 {
        risc.set_time(frame.wrapping_mul(frame_ms));
        risc.run(CPU_HZ / FPS);
    }

    let words = (risc.fb_width() * risc.fb_height()) as usize;
    let fb = &risc.framebuffer()[..words];
    let zeros = fb.iter().filter(|&&w| w == 0).count();
    let mut distinct: Vec<u32> = fb.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    let _ = std::fs::remove_file(&tmp);

    eprintln!("framebuffer: {words} words, {zeros} blank, {} distinct", distinct.len());
    assert!(zeros < words, "framebuffer entirely blank -> nothing rendered");
    assert!(
        distinct.len() > 16,
        "framebuffer too uniform ({} distinct words) -> desktop did not render",
        distinct.len()
    );
}
