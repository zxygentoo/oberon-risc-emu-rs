//! `extract-source` — extract the source files from a Project Oberon `.dsk`
//! image into a host directory: every file *except* the compiled artifacts
//! (`.rsc` object code and `.smb` symbol files), which `build-image` regenerates
//! from source anyway. The result is a build-ready tree — edit it and feed it
//! straight back to [`build-image`](../build-image).
//!
//! It reads the Oberon on-disk filesystem directly (see [`dsk`]); no emulator, no
//! boot. Usage: `extract-source <DISK_IMAGE> <OUTPUT_DIR>`.
//!
//! Extracted files are byte-for-byte as stored. Oberon sources (`*.Mod`,
//! `*.Tool`, `*.Text`) are "Oberon Text", not plain UTF-8; pipe them through
//! [`ob2unix`](../ob2unix) to read them as plain text.

use std::path::{Path, PathBuf};
use std::process::exit;
use std::{fs, io};

use clap::Parser;

mod dsk;
use dsk::Image;

/// Extract the source files from a Project Oberon `.dsk` image.
///
/// Reads the Oberon on-disk filesystem directly (no emulator) and writes each
/// file into the output directory, skipping the compiled object and symbol files.
/// The result is a build-ready source tree.
#[derive(Parser, Debug)]
#[command(name = "extract-source", version)]
struct Cli {
    /// The `.dsk` image to read
    #[arg(value_name = "DISK_IMAGE")]
    image: PathBuf,

    /// Directory to extract the sources into (created if needed)
    #[arg(value_name = "OUTPUT_DIR")]
    output: PathBuf,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        eprintln!("extract-source: {e}");
        exit(1);
    }
}

/// A compiled artifact that `build-image` regenerates from source — and that, if
/// kept, shadows the freshly built one and breaks a rebuild — so we skip it.
fn is_compiled(name: &str) -> bool {
    matches!(
        Path::new(name).extension().and_then(|e| e.to_str()),
        Some("rsc" | "smb")
    )
}

fn run(cli: &Cli) -> io::Result<()> {
    let image = Image::open(&cli.image)?;
    fs::create_dir_all(&cli.output)?;

    let (mut extracted, mut skipped) = (0usize, 0usize);
    for e in image.entries()? {
        if is_compiled(&e.name) {
            skipped += 1;
            continue;
        }
        match image.read_file(e.header) {
            Ok(data) => {
                write_file(&cli.output, &e.name, &data)?;
                extracted += 1;
            }
            // Best effort: skip an unreadable file rather than abort.
            Err(err) => eprintln!("extract-source: skipping '{}': {err}", e.name),
        }
    }
    println!(
        "extracted {extracted} source files to {} (skipped {skipped} compiled .rsc/.smb)",
        cli.output.display()
    );
    Ok(())
}

/// Write one extracted file into `dir`. Rejects any name that doesn't resolve to
/// a direct child of `dir` — defense in depth on top of [`dsk`]'s name validation.
fn write_file(dir: &Path, name: &str, data: &[u8]) -> io::Result<()> {
    let path = dir.join(name);
    if path.parent() != Some(dir) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing suspicious file name '{name}'"),
        ));
    }
    fs::write(path, data)
}
