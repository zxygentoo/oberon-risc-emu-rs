//! `eo-driver` — a headless driver for a full Extended Oberon image, and a
//! host-side developer tool for hacking on the EO bootstrap. It boots EO with no
//! window on the `risc-core` CPU (deterministic 60 Hz synthetic clock), drives it
//! from the host (move the pointer, middle-click to execute, push files over
//! `PCLink`), and observes it (framebuffer hash/PGM dump, captured serial) — enough
//! to build, boot, and watch EO headless, e.g. to regenerate the `build-eo-image`
//! seed.
//!
//! Driving is deliberately crude — scripted screen coordinates for a click, files
//! pushed over `PCLink` — because it pokes the emulator from *outside*. It is not an
//! interface used from within Oberon: an on-EO coding agent would run as an Oberon
//! module and drive the system through EO's own internal interfaces, not this. See
//! crates/host-tools/BUILD-EO-IMAGE.md.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::rc::Rc;

use clap::Parser;
use risc_core::disk::Disk;
use risc_core::headless::{framebuffer_hash, CPU_HZ, FPS};
use risc_core::io::Serial;
use risc_core::pclink::PcLink;
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

    /// After booting, move the pointer to screen pixel X,Y (top-left origin).
    #[arg(long, value_name = "X,Y")]
    move_to: Option<String>,

    /// After moving, middle-click there — Oberon's "execute the command under the
    /// pointer" gesture. Requires --move-to.
    #[arg(long)]
    mid_click: bool,

    /// Use the `PCLink` file-transfer serial backend, watching this directory for
    /// `PCLink.REC`/`PCLink.SND` job files (start `PCLink1.Run` on EO first, e.g.
    /// via --move-to/--mid-click). Replaces the default serial-capture backend.
    #[arg(long, value_name = "DIR")]
    pclink_dir: Option<PathBuf>,

    /// Frames to run after the pointer/click step — time for a command to finish
    /// or a `PCLink` transfer to complete.
    #[arg(long, default_value_t = 180)]
    after: u32,

    /// Push a host file onto EO over `PCLink` (repeatable). Each is sent under its
    /// basename, after the pointer/click step (which must start `PCLink1.Run`).
    /// Requires --pclink-dir.
    #[arg(long, value_name = "HOSTFILE")]
    push: Vec<PathBuf>,
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
    match &cli.pclink_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("pclink dir {}: {e}", dir.display()))?;
            risc.set_serial(Box::new(PcLink::in_dir(dir.clone())));
        }
        None => risc.set_serial(Box::new(CaptureSerial(serial.clone()))),
    }

    let frame_ms = 1000 / FPS;
    let cycles = CPU_HZ / FPS;
    eprintln!(
        "Booting {} headless: {} frames @ {FPS} Hz (~{} guest-seconds)...",
        cli.image.display(),
        cli.frames,
        cli.frames / FPS
    );

    // Boot/settle: detect when the screen stops changing (two consecutive samples
    // match; any later change resets it).
    let (mut prev_hash, mut settled_at) = (None::<u64>, None::<u32>);
    let mut frame = 0u32;
    while frame < cli.frames {
        risc.set_time(frame.wrapping_mul(frame_ms));
        risc.run(cycles);
        if cli.sample != 0 && frame.is_multiple_of(cli.sample) {
            let h = framebuffer_hash(&risc);
            if prev_hash == Some(h) {
                settled_at.get_or_insert(frame);
            } else {
                prev_hash = Some(h);
                settled_at = None;
            }
        }
        frame += 1;
    }

    // Optional input: move the pointer (and middle-click to execute) after boot.
    // Oberon's y origin is bottom-left, so flip the screen y the screenshot uses.
    if let Some(spec) = &cli.move_to {
        let (x, y) = parse_xy(spec)?;
        let y_oberon = (risc.fb_height() - 1) - y;
        eprintln!("Pointer -> screen ({x},{y}) = Oberon ({x},{y_oberon})");
        risc.mouse_moved(x, y_oberon);
        advance(&mut risc, &mut frame, 15, frame_ms, cycles);
        if cli.mid_click {
            eprintln!("Middle-click (execute) at screen ({x},{y})");
            risc.mouse_button(2, true);
            advance(&mut risc, &mut frame, 8, frame_ms, cycles);
            risc.mouse_button(2, false);
        }
    }

    // Push files onto EO over PCLink (PCLink1.Run must already be running, e.g.
    // started by the --move-to/--mid-click above). Each push stages the file plus
    // a PCLink.REC job, then runs frames for the transfer.
    if !cli.push.is_empty() {
        let dir = cli
            .pclink_dir
            .as_ref()
            .ok_or("--push requires --pclink-dir")?;
        for hostfile in &cli.push {
            let name = hostfile
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| format!("bad --push file name: {}", hostfile.display()))?;
            std::fs::copy(hostfile, dir.join(name)).map_err(|e| format!("stage {name}: {e}"))?;
            std::fs::write(dir.join("PCLink.REC"), name).map_err(|e| format!("PCLink.REC: {e}"))?;
            eprintln!("Pushing {name} -> EO");
            advance(&mut risc, &mut frame, 6000, frame_ms, cycles);
        }
        let _ = std::fs::remove_file(dir.join("PCLink.REC"));
    }

    // Settle / transfer time after the input step (a command finishing, or a
    // PCLink transfer running while EO's PCLink1.Run polls the serial line).
    advance(&mut risc, &mut frame, cli.after, frame_ms, cycles);

    report(&risc, &serial.borrow(), settled_at);
    if let Some(dir) = &cli.pclink_dir {
        report_pclink_dir(dir);
    }
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

/// Run `n` frames on the synthetic clock, advancing the shared frame counter.
fn advance(risc: &mut Risc, frame: &mut u32, n: u32, frame_ms: u32, cycles: u32) {
    for _ in 0..n {
        risc.set_time(frame.wrapping_mul(frame_ms));
        risc.run(cycles);
        *frame += 1;
    }
}

/// List what landed in the `PCLink` working directory (received transfers + jobs).
fn report_pclink_dir(dir: &Path) {
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            let mut names: Vec<String> = entries
                .filter_map(Result::ok)
                .map(|e| {
                    let len = e.metadata().map_or(0, |m| m.len());
                    format!("{} ({len}B)", e.file_name().to_string_lossy())
                })
                .collect();
            names.sort();
            eprintln!("PCLink dir {}: [{}]", dir.display(), names.join(", "));
        }
        Err(e) => eprintln!("PCLink dir {}: {e}", dir.display()),
    }
}

/// Parse an `X,Y` pair of screen pixels (top-left origin).
fn parse_xy(s: &str) -> Result<(i32, i32), String> {
    let (a, b) = s.split_once(',').ok_or("expected X,Y")?;
    let x = a.trim().parse().map_err(|_| format!("bad X in {s:?}"))?;
    let y = b.trim().parse().map_err(|_| format!("bad Y in {s:?}"))?;
    Ok((x, y))
}
