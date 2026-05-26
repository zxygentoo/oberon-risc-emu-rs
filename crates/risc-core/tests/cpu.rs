//! Headless boot smoke test: drive the core against a real Oberon disk image
//! with a synthetic clock and assert the display framebuffer comes alive (i.e.
//! the desktop actually renders). Gated on the `OBERON_DISK` environment
//! variable (path to a `.dsk`); skipped when unset so default CI stays hermetic.

use risc_core::disk::Disk;
use risc_core::risc::Risc;

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

    eprintln!(
        "framebuffer: {words} words, {zeros} blank, {} distinct",
        distinct.len()
    );
    assert!(
        zeros < words,
        "framebuffer entirely blank -> nothing rendered"
    );
    assert!(
        distinct.len() > 16,
        "framebuffer too uniform ({} distinct words) -> desktop did not render",
        distinct.len()
    );
}

// Byte size of Oberon-2020-08-18.dsk, the image the golden hashes were captured
// from (other shipped images differ in size, so we can identify it cheaply).
const GOLDEN_IMAGE_SIZE: u64 = 990208;

// (frames-run, framebuffer FNV-1a, {PC,R,H,flags} FNV-1a), captured from the C
// reference by tools/gen_boot_golden.c. Regenerate both together.
const BOOT_GOLDEN: &[(u32, u64, u64)] = &[
    (1, 0xf5edab31b6802325, 0x03869b4b0b926433),
    (2, 0xf5edab31b6802325, 0x2926f3cc7568ea25),
    (5, 0xf5edab31b6802325, 0xdba6e0006e93fd52),
    (15, 0xf5edab31b6802325, 0x1f7a42198e5e3891),
    (45, 0xb9bdbf56ba51298d, 0x66a3e6fd77a6b491),
    (120, 0xb9bdbf56ba51298d, 0x66a3e6fd77a6b491),
    (250, 0xb9bdbf56ba51298d, 0x7531e8819ea3aac1),
];

fn fnv1a(words: impl IntoIterator<Item = u32>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for w in words {
        for b in w.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// Differential boot: drive the core with the same deterministic schedule as
/// `tools/gen_boot_golden.c` and assert the framebuffer + CPU state hash
/// bit-identically to the C reference at every checkpoint. This proves the port
/// matches C through a full boot, not merely that it boots.
#[test]
fn boot_matches_c_reference() {
    let Ok(src) = std::env::var("OBERON_DISK") else {
        eprintln!("OBERON_DISK not set; skipping golden boot test");
        return;
    };
    let size = std::fs::metadata(&src).map_or(0, |m| m.len());
    if size != GOLDEN_IMAGE_SIZE {
        eprintln!(
            "OBERON_DISK is {size} bytes, not the golden image ({GOLDEN_IMAGE_SIZE}); \
             skipping golden boot test"
        );
        return;
    }

    let mut tmp = std::env::temp_dir();
    tmp.push(format!("oberon_golden_{}.dsk", std::process::id()));
    std::fs::copy(&src, &tmp).expect("copy disk image");

    let mut risc = Risc::new();
    risc.set_spi(1, Box::new(Disk::new(Some(&tmp)).expect("open disk")));

    let frame_ms = 1000 / FPS;
    let total = BOOT_GOLDEN.last().unwrap().0;
    let mut ci = 0;
    for frame in 0..total {
        risc.set_time(frame.wrapping_mul(frame_ms));
        risc.run(CPU_HZ / FPS);

        if ci < BOOT_GOLDEN.len() && BOOT_GOLDEN[ci].0 == frame + 1 {
            let words = (risc.fb_width() * risc.fb_height()) as usize;
            let fb_hash = fnv1a(risc.framebuffer()[..words].iter().copied());

            let s = risc.cpu_state();
            let flags = u32::from(s.flags.bits());
            let state = std::iter::once(s.pc).chain(s.r).chain([s.h, flags]);
            let state_hash = fnv1a(state);

            let (n, gfb, gstate) = BOOT_GOLDEN[ci];
            assert_eq!(
                fb_hash, gfb,
                "frame {n}: framebuffer diverged from C reference"
            );
            assert_eq!(
                state_hash, gstate,
                "frame {n}: CPU state diverged from C reference"
            );
            ci += 1;
        }
    }

    let _ = std::fs::remove_file(&tmp);
    assert_eq!(ci, BOOT_GOLDEN.len(), "did not reach all checkpoints");
    eprintln!(
        "boot matches C reference at all {} checkpoints",
        BOOT_GOLDEN.len()
    );
}
