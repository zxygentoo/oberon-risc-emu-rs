//! `build-po-image` — assemble a bootable Project Oberon 2013 disk image.
//!
//! Usage: `build-po-image <sources_dir> <output.dsk>`
//!
//! `sources_dir` is a Project Oberon source tree fetched by project-norebo's
//! `fetch-sources.py` (Wirth's PO2013 sources plus the emulator's `Input`/
//! `Display`, fonts, etc.). The Norebo host modules and the bootstrap inner core
//! are embedded in this binary (`assets/common` + `assets/po`, vendored from
//! project-norebo), so no external checkout is needed. Only the finished image is
//! written, to `<output.dsk>`.
//!
//! All the work is the shared [`host_tools::pipeline`]; this binary supplies
//! only the embedded PO2013 [`Seed`] and the CLI. Its Extended Oberon twin is
//! `build-eo-image` — same pipeline, different seed.

use std::path::PathBuf;
use std::process::exit;

use clap::Parser;

use host_tools::pipeline::{self, Seed, PACKONLY_HELP};

/// The embedded PO2013 toolchain seed (vendored under `assets/`): the shared host
/// glue (`assets/common`), the PO2013-specific glue (`assets/po/glue`), and the
/// prebuilt bootstrap objects + inner core (`assets/po/bootstrap`) that seed the
/// first compile. Written flat to one scratch directory at runtime; the host glue
/// `.Mod` override the stock PO2013 modules of the same name, talking to the host
/// instead of FPGA hardware.
const TOOLCHAIN: &[(&str, &[u8])] = &[
    // Shared host glue.
    (
        "Norebo.Mod",
        include_bytes!("../../assets/common/Norebo.Mod"),
    ),
    (
        "FileDir.Mod",
        include_bytes!("../../assets/common/FileDir.Mod"),
    ),
    ("Files.Mod", include_bytes!("../../assets/common/Files.Mod")),
    // PO2013-specific glue.
    (
        "Kernel.Mod",
        include_bytes!("../../assets/po/glue/Kernel.Mod"),
    ),
    (
        "Oberon.Mod",
        include_bytes!("../../assets/po/glue/Oberon.Mod"),
    ),
    (
        "CoreLinker.Mod",
        include_bytes!("../../assets/po/glue/CoreLinker.Mod"),
    ),
    // The VDisk family (host-side virtual disk; shared with EO).
    ("VDisk.Mod", include_bytes!("../../assets/common/VDisk.Mod")),
    (
        "VFileDir.Mod",
        include_bytes!("../../assets/common/VFileDir.Mod"),
    ),
    (
        "VFiles.Mod",
        include_bytes!("../../assets/common/VFiles.Mod"),
    ),
    (
        "VDiskUtil.Mod",
        include_bytes!("../../assets/common/VDiskUtil.Mod"),
    ),
    // Prebuilt bootstrap inner core + objects (the shim boots/loads these to run
    // the first compile + link).
    (
        "InnerCore",
        include_bytes!("../../assets/po/bootstrap/InnerCore"),
    ),
    (
        "Kernel.rsc",
        include_bytes!("../../assets/po/bootstrap/Kernel.rsc"),
    ),
    (
        "FileDir.rsc",
        include_bytes!("../../assets/po/bootstrap/FileDir.rsc"),
    ),
    (
        "Files.rsc",
        include_bytes!("../../assets/po/bootstrap/Files.rsc"),
    ),
    (
        "Modules.rsc",
        include_bytes!("../../assets/po/bootstrap/Modules.rsc"),
    ),
    (
        "Norebo.rsc",
        include_bytes!("../../assets/po/bootstrap/Norebo.rsc"),
    ),
    (
        "Oberon.rsc",
        include_bytes!("../../assets/po/bootstrap/Oberon.rsc"),
    ),
    (
        "CoreLinker.rsc",
        include_bytes!("../../assets/po/bootstrap/CoreLinker.rsc"),
    ),
    (
        "Fonts.rsc",
        include_bytes!("../../assets/po/bootstrap/Fonts.rsc"),
    ),
    (
        "Texts.rsc",
        include_bytes!("../../assets/po/bootstrap/Texts.rsc"),
    ),
    (
        "RS232.rsc",
        include_bytes!("../../assets/po/bootstrap/RS232.rsc"),
    ),
    (
        "ORS.rsc",
        include_bytes!("../../assets/po/bootstrap/ORS.rsc"),
    ),
    (
        "ORB.rsc",
        include_bytes!("../../assets/po/bootstrap/ORB.rsc"),
    ),
    (
        "ORG.rsc",
        include_bytes!("../../assets/po/bootstrap/ORG.rsc"),
    ),
    (
        "ORP.rsc",
        include_bytes!("../../assets/po/bootstrap/ORP.rsc"),
    ),
];

/// The committed golden inner core: the inner core re-linked during the build must
/// reproduce it byte-for-byte (compiler + `CoreLinker` are deterministic) — a
/// self-consistency check on the seed. Same bytes as the `InnerCore` entry above.
const GOLDEN_INNER_CORE: &[u8] = include_bytes!("../../assets/po/bootstrap/InnerCore");

const SEED: Seed = Seed {
    toolchain: TOOLCHAIN,
    golden_inner_core: GOLDEN_INNER_CORE,
    name: "build-po-image",
};

/// Build a runnable Project Oberon disk image from a source tree.
///
/// Compiles Project Oberon with the embedded Norebo toolchain and assembles a
/// bootable disk image. The sources are fetched separately (e.g. by
/// project-norebo's `fetch-sources.py`); only the finished image is written.
#[derive(Parser, Debug)]
#[command(name = "build-po-image", version, after_long_help = PACKONLY_HELP)]
struct Cli {
    /// Project Oberon source tree (e.g. from project-norebo's fetch-sources.py)
    #[arg(value_name = "SOURCES_DIR")]
    sources: PathBuf,

    /// Path to write the disk image to
    #[arg(value_name = "OUTPUT")]
    output: PathBuf,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = pipeline::build(&SEED, &cli.sources, &cli.output) {
        eprintln!("build-po-image: {e}");
        exit(1);
    }
    println!("Done: {}", cli.output.display());
}
