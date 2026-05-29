//! `eo-driver` — a headless driver for the Extended Oberon system. The bootstrap
//! mechanism for `build-eo-image` (and the eventual on-EO coding-agent control
//! plane): boot EO with no windowing, drive it, and observe it from the host.
//!
//! Milestone 1 (this file): boot EO's `RISC.img` on the `risc-core` CPU, run it
//! on the deterministic 60 Hz synthetic clock, attach a programmable serial line
//! that captures everything EO transmits (and can feed it input), and report when
//! the framebuffer settles — the sign the system has reached its desktop. Command
//! injection (compile + `ORL.Link` over keyboard/mouse or a serial protocol) is
//! the next milestone; see crates/host-tools/BUILD-EO-IMAGE.md.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::rc::Rc;

use clap::Parser;
use risc_core::disk::Disk;
use risc_core::headless::{framebuffer_hash, CPU_HZ, FPS};
use risc_core::io::Serial;
use risc_core::risc::Risc;

/// Boot an Extended Oberon disk image headless and observe it.
#[derive(Parser, Debug)]
#[command(name = "eo-driver", version)]
struct Cli {
    /// The Extended Oberon disk image (the full `RISC.img` SD image or a `.dsk`).
    #[arg(value_name = "IMAGE")]
    image: PathBuf,

    /// Frames to run on the 60 Hz synthetic clock (~1/60 s of guest time each).
    #[arg(long, default_value_t = 2000)]
    frames: u32,

    /// Sample the framebuffer every N frames to detect when it settles.
    #[arg(long, default_value_t = 30)]
    sample: u32,

    /// Dump the final framebuffer as a binary PGM (P5), for a visual check.
    #[arg(long, value_name = "FILE.pgm")]
    fb_out: Option<PathBuf>,

    /// Dump everything EO transmitted on the serial line to this file.
    #[arg(long, value_name = "FILE")]
    serial_out: Option<PathBuf>,
}

/// Bytes in flight on the serial line, shared between the driver and the device.
#[derive(Default)]
struct SerialState {
    to_guest: VecDeque<u8>, // host -> EO (EO reads these)
    from_guest: Vec<u8>,    // EO -> host (captured)
}

/// A programmable RS232 line: feeds queued bytes to EO and captures EO's output.
/// Per `io::Serial`, `read_status` bit 0 = rx ready, bit 1 = tx ready.
struct CaptureSerial(Rc<RefCell<SerialState>>);

impl Serial for CaptureSerial {
    fn read_status(&mut self) -> u32 {
        2 | u32::from(!self.0.borrow().to_guest.is_empty())
    }
    fn read_data(&mut self) -> u32 {
        self.0.borrow_mut().to_guest.pop_front().map_or(0, u32::from)
    }
    fn write_data(&mut self, value: u32) {
        self.0.borrow_mut().from_guest.push(value as u8);
    }
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        eprintln!("eo-driver: {e}");
        exit(1);
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    let serial = Rc::new(RefCell::new(SerialState::default()));
    let mut risc = Risc::new();
    let disk =
        Disk::new(Some(&cli.image)).map_err(|e| format!("open disk {}: {e}", cli.image.display()))?;
    risc.set_spi(1, Box::new(disk));
    risc.set_serial(Box::new(CaptureSerial(serial.clone())));

    let frame_ms = 1000 / FPS;
    let cycles = CPU_HZ / FPS;
    eprintln!(
        "Booting {} headless: {} frames @ {FPS} Hz (~{} guest-seconds)...",
        cli.image.display(),
        cli.frames,
        cli.frames / FPS
    );

    // Detect when the screen stops changing: the first frame after which two
    // consecutive samples match (any later change resets it).
    let (mut prev_hash, mut settled_at) = (None::<u64>, None::<u32>);
    for frame in 0..cli.frames {
        risc.set_time(frame.wrapping_mul(frame_ms));
        risc.run(cycles);
        if cli.sample != 0 && frame % cli.sample == 0 {
            let h = framebuffer_hash(&risc);
            if prev_hash == Some(h) {
                settled_at.get_or_insert(frame);
            } else {
                prev_hash = Some(h);
                settled_at = None;
            }
        }
    }

    report(&risc, &serial.borrow(), settled_at);
    if let Some(p) = &cli.fb_out {
        write_pgm(&risc, p).map_err(|e| format!("write {}: {e}", p.display()))?;
        eprintln!(
            "Wrote framebuffer to {} ({}x{} PGM)",
            p.display(),
            risc.fb_width() * 32,
            risc.fb_height()
        );
    }
    if let Some(p) = &cli.serial_out {
        std::fs::write(p, &serial.borrow().from_guest)
            .map_err(|e| format!("write {}: {e}", p.display()))?;
        eprintln!("Wrote serial capture to {}", p.display());
    }
    Ok(())
}

/// Print what we observed: settle frame, framebuffer hash, ink density, serial.
fn report(risc: &Risc, serial: &SerialState, settled_at: Option<u32>) {
    eprintln!("Final framebuffer hash: {:#018x}", framebuffer_hash(risc));
    eprintln!("Ink density: {:.1}% of pixels set", ink_density(risc) * 100.0);
    match settled_at {
        Some(f) => eprintln!("Framebuffer settled by frame ~{f} (likely reached the desktop)."),
        None => eprintln!("Framebuffer never settled — try more --frames, or the config differs."),
    }
    eprintln!("Captured {} serial byte(s) from EO.", serial.from_guest.len());
    if !serial.from_guest.is_empty() {
        let preview: String = String::from_utf8_lossy(&serial.from_guest)
            .chars()
            .take(200)
            .collect();
        eprintln!("  serial (lossy): {preview:?}");
    }
}

/// Fraction of framebuffer bits set — a coarse "is there a populated desktop
/// here" signal (a blank or a trap screen reads very differently).
fn ink_density(risc: &Risc) -> f64 {
    let words = (risc.fb_width() * risc.fb_height()) as usize;
    let set: u32 = risc.framebuffer()[..words]
        .iter()
        .map(|w| w.count_ones())
        .sum();
    f64::from(set) / (words as f64 * 32.0)
}

/// Dump the 1-bpp framebuffer as a binary PGM (P5), top row first. Oberon stores
/// the screen bottom-up with bit 0 the leftmost pixel, so we flip rows.
fn write_pgm(risc: &Risc, path: &Path) -> std::io::Result<()> {
    let (wwords, h) = (risc.fb_width(), risc.fb_height());
    let fb = risc.framebuffer();
    let mut out = format!("P5\n{} {h}\n255\n", wwords * 32).into_bytes();
    for y in (0..h).rev() {
        for xw in 0..wwords {
            let word = fb[(y * wwords + xw) as usize];
            for b in 0..32 {
                out.push(if (word >> b) & 1 != 0 { 255 } else { 0 });
            }
        }
    }
    std::fs::write(path, out)
}
