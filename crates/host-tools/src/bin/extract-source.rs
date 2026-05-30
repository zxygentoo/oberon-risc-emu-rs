//! `extract-source` — extract the source files from a Project Oberon `.dsk`
//! image into a host directory: every file *except* the compiled artifacts
//! (`.rsc` object code and `.smb` symbol files), which the image builders
//! regenerate from source anyway. The result is a build-ready tree — edit it and
//! feed it straight back to [`build-po-image`](../build-po-image) (or `build-eo-image`).
//!
//! It also writes the `.packonly` manifest the builders require, derived from
//! the image: a source `X.Mod` is a compile candidate when the image carries its
//! `X.rsc` object, and every other extracted file (data, plus reference modules
//! that ship as source with no object) is recorded as pack-only.
//!
//! It reads the Oberon on-disk filesystem directly (see [`dsk`]); no emulator, no
//! boot. Usage: `extract-source <DISK_IMAGE> <OUTPUT_DIR>`.
//!
//! By default the compiled artifacts are skipped; pass `--keep-objects` to also
//! extract them, to harvest a compiler/toolchain *seed* from a prebuilt image
//! (e.g. Extended Oberon's `RISC.img`).
//!
//! Extracted files are byte-for-byte as stored. Oberon sources (`*.Mod`,
//! `*.Tool`, `*.Text`) are plain Latin-1 with CR line endings; run them through
//! [`ob2txt`](../ob2txt) to read them as host (UTF-8/LF) text.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::{fs, io};

use clap::Parser;

use host_tools::dsk::Image;

/// The `.packonly` manifest section appended to `--help`.
const PACKONLY_HELP: &str = "\
The .packonly manifest:
  extract-source also writes `.packonly` into OUTPUT_DIR: the manifest the
  builders use to tell source from data. A source X.Mod is a compile candidate when the
  image carries its X.rsc object; every other extracted file (data, and reference
  modules shipped as source with no object) is recorded as pack-only.

  One file name per line; blank lines and `#` comments are ignored; an empty list
  means the builders compile everything. The tree feeds straight back into
  build-po-image (or build-eo-image).";

/// Extract the source files from a Project Oberon `.dsk` image.
///
/// Reads the Oberon on-disk filesystem directly (no emulator) and writes each
/// file into the output directory, skipping the compiled object and symbol files.
/// The result is a build-ready source tree.
#[derive(Parser, Debug)]
#[command(name = "extract-source", version, after_long_help = PACKONLY_HELP)]
struct Cli {
    /// The `.dsk` image to read
    #[arg(value_name = "DISK_IMAGE")]
    image: PathBuf,

    /// Directory to extract the sources into (created if needed)
    #[arg(value_name = "OUTPUT_DIR")]
    output: PathBuf,

    /// Also extract compiled objects (`.rsc`) and symbol files (`.smb`), which are
    /// skipped by default. Use this to harvest a toolchain *seed* from a prebuilt
    /// image (e.g. Extended Oberon's `RISC.img`). The result is seed material, not
    /// a tree to feed back into the builders — kept objects would shadow a rebuild.
    #[arg(long)]
    keep_objects: bool,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        eprintln!("extract-source: {e}");
        exit(1);
    }
}

/// A compiled artifact that the image builders regenerate from source — and that, if
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
    let entries = image.entries()?;

    // Module names with a compiled object on the image: their `.Mod` source is a
    // compile candidate; everything else extracted is packed verbatim.
    let compiled: HashSet<&str> = entries
        .iter()
        .filter_map(|e| e.name.strip_suffix(".rsc"))
        .collect();

    let mut packonly = BTreeSet::new();
    let (mut extracted, mut skipped, mut objects) = (0usize, 0usize, 0usize);
    for e in &entries {
        if is_compiled(&e.name) && !cli.keep_objects {
            skipped += 1;
            continue;
        }
        let is_module_source = e
            .name
            .strip_suffix(".Mod")
            .is_some_and(|stem| compiled.contains(stem));
        match image.read_file(e.header) {
            Ok(data) => {
                write_file(&cli.output, &e.name, &data)?;
                extracted += 1;
                // Record pack-only only once the file is actually written, so the
                // manifest never names a file we failed to extract. Compiled objects
                // (present only with --keep-objects) are seed material, not pack-only
                // data, so they stay off the manifest.
                if is_compiled(&e.name) {
                    objects += 1;
                } else if !is_module_source {
                    packonly.insert(e.name.clone());
                }
            }
            // Best effort: skip an unreadable file rather than abort.
            Err(err) => eprintln!("extract-source: skipping '{}': {err}", e.name),
        }
    }

    // The manifest is required by the builders and always regenerated, so the tree
    // is build-ready as-is (an empty list — every module compiled — still writes).
    fs::write(
        cli.output.join(".packonly"),
        host_tools::packonly::render(&packonly),
    )?;

    let tail = if cli.keep_objects {
        format!("kept {objects} .rsc/.smb objects")
    } else {
        format!("skipped {skipped} compiled .rsc/.smb")
    };
    println!(
        "extracted {extracted} files to {} ({} pack-only; {tail})",
        cli.output.display(),
        packonly.len(),
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
