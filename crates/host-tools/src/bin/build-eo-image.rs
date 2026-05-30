//! `build-eo-image` — assemble a bootable Extended Oberon (EO) disk image, the EO
//! counterpart of `build-po-image`.
//!
//! Usage: `build-eo-image <sources_dir> <output.dsk>`
//!
//! `sources_dir` is an Extended Oberon source tree (e.g. produced by
//! `extract-source` from an EO `RISC.img`): the real EO modules and data, with a
//! `.packonly` manifest naming what to pack verbatim instead of compiling. The
//! headless host glue and a prebuilt EO bootstrap inner core are embedded in this
//! binary (`assets/common` + `assets/eo`), so no external toolchain is needed.
//!
//! All the work is the shared [`host_tools::image`] pipeline; this binary supplies
//! only the embedded EO [`Seed`] and the CLI. The EO specifics are baked into that
//! seed:
//!  * the inner-core top module is **`Modules`** (its body runs `Init` →
//!    `Files.Init` → `Kernel.Init`, then `Load("Oberon")`), the standard EO boot
//!    sequence — linking `Oberon` as a self-contained core never inits the heap;
//!  * the EO `CoreLinker` reads `.rsx`, so the freshly compiled objects (renamed
//!    `.rsc`->`.rsx` around each link by the pipeline) don't collide with the live
//!    `.rsc` the shim loads to run the linker.
//!
//! The result boots to the EO desktop in the emulator (`risc` / `eo-driver`).

use std::path::PathBuf;
use std::process::exit;

use clap::Parser;

use host_tools::image::{self, Seed, PACKONLY_HELP};

/// The embedded EO toolchain seed (vendored under `assets/`): the shared host glue
/// (`assets/common`), the EO-specific glue (`assets/eo/glue`), and the prebuilt
/// bootstrap objects + `Modules`-topped inner core (`assets/eo/bootstrap`) that seed
/// the first build. Written flat to one scratch directory at runtime.
const TOOLCHAIN: &[(&str, &[u8])] = &[
    // Shared host glue (override the stock EO modules of the same name).
    (
        "Norebo.Mod",
        include_bytes!("../../assets/common/Norebo.Mod"),
    ),
    (
        "FileDir.Mod",
        include_bytes!("../../assets/common/FileDir.Mod"),
    ),
    ("Files.Mod", include_bytes!("../../assets/common/Files.Mod")),
    // EO-specific glue.
    (
        "Kernel.Mod",
        include_bytes!("../../assets/eo/glue/Kernel.Mod"),
    ),
    (
        "Oberon.Mod",
        include_bytes!("../../assets/eo/glue/Oberon.Mod"),
    ),
    (
        "CoreLinker.Mod",
        include_bytes!("../../assets/eo/glue/CoreLinker.Mod"),
    ),
    // The VDisk family (host-side virtual disk; shared with PO2013 — the on-disk
    // FS format is identical, and they compile against the EO glue unchanged).
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
    // Prebuilt bootstrap inner core + objects; all glue-compiled (see
    // assets/eo/bootstrap).
    (
        "InnerCore",
        include_bytes!("../../assets/eo/bootstrap/InnerCore"),
    ),
    (
        "Kernel.rsc",
        include_bytes!("../../assets/eo/bootstrap/Kernel.rsc"),
    ),
    (
        "FileDir.rsc",
        include_bytes!("../../assets/eo/bootstrap/FileDir.rsc"),
    ),
    (
        "Files.rsc",
        include_bytes!("../../assets/eo/bootstrap/Files.rsc"),
    ),
    (
        "Modules.rsc",
        include_bytes!("../../assets/eo/bootstrap/Modules.rsc"),
    ),
    (
        "Norebo.rsc",
        include_bytes!("../../assets/eo/bootstrap/Norebo.rsc"),
    ),
    (
        "Oberon.rsc",
        include_bytes!("../../assets/eo/bootstrap/Oberon.rsc"),
    ),
    (
        "CoreLinker.rsc",
        include_bytes!("../../assets/eo/bootstrap/CoreLinker.rsc"),
    ),
    (
        "Fonts.rsc",
        include_bytes!("../../assets/eo/bootstrap/Fonts.rsc"),
    ),
    (
        "Texts.rsc",
        include_bytes!("../../assets/eo/bootstrap/Texts.rsc"),
    ),
    (
        "RS232.rsc",
        include_bytes!("../../assets/eo/bootstrap/RS232.rsc"),
    ),
    (
        "ORS.rsc",
        include_bytes!("../../assets/eo/bootstrap/ORS.rsc"),
    ),
    (
        "ORB.rsc",
        include_bytes!("../../assets/eo/bootstrap/ORB.rsc"),
    ),
    (
        "ORG.rsc",
        include_bytes!("../../assets/eo/bootstrap/ORG.rsc"),
    ),
    (
        "ORP.rsc",
        include_bytes!("../../assets/eo/bootstrap/ORP.rsc"),
    ),
];

/// The golden bootstrap inner core: the fresh glue inner core re-linked during the
/// build must reproduce it byte-for-byte — a self-consistency check on the seed.
/// Same bytes as the `InnerCore` entry above.
const GOLDEN_INNER_CORE: &[u8] = include_bytes!("../../assets/eo/bootstrap/InnerCore");

const SEED: Seed = Seed {
    toolchain: TOOLCHAIN,
    golden_inner_core: GOLDEN_INNER_CORE,
    name: "build-eo-image",
};

/// Build a bootable Extended Oberon disk image from a source tree.
#[derive(Parser, Debug)]
#[command(name = "build-eo-image", version, after_long_help = PACKONLY_HELP)]
struct Cli {
    /// Extended Oberon source tree (e.g. from `extract-source` on an `RISC.img`)
    #[arg(value_name = "SOURCES_DIR")]
    sources: PathBuf,

    /// Path to write the bootable disk image to
    #[arg(value_name = "OUTPUT")]
    output: PathBuf,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = image::build(&SEED, &cli.sources, &cli.output) {
        eprintln!("build-eo-image: {e}");
        exit(1);
    }
    println!("Done: {}", cli.output.display());
}
