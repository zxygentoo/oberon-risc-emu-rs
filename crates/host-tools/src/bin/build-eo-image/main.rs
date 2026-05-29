//! `build-eo-image` — assemble a bootable Extended Oberon (EO) disk image with
//! our in-process `shim` engine, the EO counterpart of `build-image`.
//!
//! Usage: `build-eo-image <sources_dir> <output.dsk>`
//!
//! `sources_dir` is an Extended Oberon source tree (e.g. produced by
//! `extract-source` from an EO `RISC.img`): the real EO modules and data, with a
//! `.packonly` manifest naming what to pack verbatim instead of compiling. The
//! headless host glue (`Kernel`/`Files`/`FileDir`/`Oberon`/`Norebo`/`CoreLinker` +
//! the `VDisk` family) and a prebuilt EO bootstrap inner core are embedded in this
//! binary (`assets/`), so no external toolchain is needed.
//!
//! It compiles the EO toolchain through the shim, links a fresh inner core, then
//! compiles the *whole* EO source tree and assembles a bootable `Oberon.dsk` — the
//! same pipeline `build-image` uses for Project Oberon 2013, with two EO specifics:
//!  * the inner-core top module is **`Modules`** (its body runs `Init` →
//!    `Files.Init` → `Kernel.Init`, then `Load("Oberon")`), the standard EO boot
//!    sequence — linking `Oberon` as a self-contained core never inits the heap;
//!  * the glue `CoreLinker` reads `.rsx`, so the freshly compiled objects (renamed
//!    `.rsc`->`.rsx` around each link) don't collide with the live `.rsc` the shim
//!    loads to run the linker — exactly as in `build-image`.
//!
//! The result boots to the EO desktop in the emulator (`risc` / `eo-driver`).

use std::path::{Path, PathBuf};
use std::process::exit;
use std::{fs, io};

use clap::Parser;

use host_tools::resolve;
use host_tools::shim::run;

/// The EO toolchain modules, compiled to seed the host toolchain and then linked
/// into a fresh inner core. The host glue (`Kernel`/`Files`/`Oberon`/`FileDir`/
/// `Norebo`/`CoreLinker`/`VDisk…`) comes from the embedded seed; the rest
/// (`Modules`/`Fonts`/`Texts`/`RS232`/`OR*`) from the source tree. Same set as
/// `build-image`'s — only the embedded glue sources differ.
const NOREBO_MODULES: &[&str] = &[
    "Norebo.Mod",
    "Kernel.Mod",
    "FileDir.Mod",
    "Files.Mod",
    "Modules.Mod",
    "Fonts.Mod",
    "Texts.Mod",
    "RS232.Mod",
    "Oberon.Mod",
    "ORS.Mod",
    "ORB.Mod",
    "ORG.Mod",
    "ORP.Mod",
    "CoreLinker.Mod",
    "VDisk.Mod",
    "VFileDir.Mod",
    "VFiles.Mod",
    "VDiskUtil.Mod",
];

/// The embedded EO toolchain seed (vendored under `assets/`): the host-adapted
/// glue + `VDisk`-family sources, plus the prebuilt bootstrap objects + inner core
/// that let the shim run the EO compiler/linker for the first build. Written flat
/// to one scratch directory at runtime, mirroring `build-image`.
const TOOLCHAIN: &[(&str, &[u8])] = &[
    // Host glue sources (override the stock EO modules of the same name).
    (
        "Norebo.Mod",
        include_bytes!("../../../assets/Norebo/Norebo.Mod"),
    ),
    (
        "FileDir.Mod",
        include_bytes!("../../../assets/Norebo/FileDir.Mod"),
    ),
    (
        "Files.Mod",
        include_bytes!("../../../assets/Norebo/Files.Mod"),
    ),
    (
        "Kernel.Mod",
        include_bytes!("../../../assets/eo-norebo/Kernel.Mod"),
    ),
    (
        "Oberon.Mod",
        include_bytes!("../../../assets/eo-norebo/Oberon.Mod"),
    ),
    (
        "CoreLinker.Mod",
        include_bytes!("../../../assets/eo-norebo/CoreLinker.Mod"),
    ),
    // The VDisk family (host-side virtual disk; shared with PO2013 — the on-disk
    // FS format is identical, and they compile against the EO glue unchanged).
    (
        "VDisk.Mod",
        include_bytes!("../../../assets/Norebo/VDisk.Mod"),
    ),
    (
        "VFileDir.Mod",
        include_bytes!("../../../assets/Norebo/VFileDir.Mod"),
    ),
    (
        "VFiles.Mod",
        include_bytes!("../../../assets/Norebo/VFiles.Mod"),
    ),
    (
        "VDiskUtil.Mod",
        include_bytes!("../../../assets/Norebo/VDiskUtil.Mod"),
    ),
    // Prebuilt bootstrap inner core + objects (the shim boots/loads these to run
    // the first compile + link). All glue-compiled; see assets/eo-Bootstrap.
    (
        "InnerCore",
        include_bytes!("../../../assets/eo-Bootstrap/InnerCore"),
    ),
    (
        "Kernel.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/Kernel.rsc"),
    ),
    (
        "FileDir.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/FileDir.rsc"),
    ),
    (
        "Files.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/Files.rsc"),
    ),
    (
        "Modules.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/Modules.rsc"),
    ),
    (
        "Norebo.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/Norebo.rsc"),
    ),
    (
        "Oberon.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/Oberon.rsc"),
    ),
    (
        "CoreLinker.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/CoreLinker.rsc"),
    ),
    (
        "Fonts.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/Fonts.rsc"),
    ),
    (
        "Texts.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/Texts.rsc"),
    ),
    (
        "RS232.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/RS232.rsc"),
    ),
    (
        "ORS.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/ORS.rsc"),
    ),
    (
        "ORB.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/ORB.rsc"),
    ),
    (
        "ORG.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/ORG.rsc"),
    ),
    (
        "ORP.rsc",
        include_bytes!("../../../assets/eo-Bootstrap/ORP.rsc"),
    ),
];

/// The golden bootstrap inner core: the fresh glue inner core re-linked during the
/// build must reproduce it byte-for-byte (the EO compiler + `CoreLinker` are
/// deterministic) — a self-consistency check on the embedded seed.
const GOLDEN_INNER_CORE: &[u8] = include_bytes!("../../../assets/eo-Bootstrap/InnerCore");

/// The `.packonly` manifest section appended to `--help` (see [`resolve`]).
const PACKONLY_HELP: &str = "\
The .packonly manifest:
  Every file in SOURCES_DIR is compiled as Oberon source and packed into the
  image, except those listed in `.packonly` (at the tree root), which are packed
  verbatim: data such as fonts and tools, and reference modules that ship as
  source but are not meant to compile (e.g. hardware-only modules).

  The manifest is required; an empty one compiles everything. One file name per
  line; blank lines and `#` comments are ignored. extract-source generates it.";

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
    if let Err(e) = build_eo_image(&cli.sources, &cli.output) {
        eprintln!("build-eo-image: {e}");
        exit(1);
    }
    println!("Done: {}", cli.output.display());
}

/// Build in a temp scratch directory and, on success, copy the finished
/// `Oberon.dsk` to `output`. On failure the scratch dir is left for inspection.
fn build_eo_image(sources: &Path, output: &Path) -> io::Result<()> {
    // Settle what to compile before touching the toolchain, so a bad source tree
    // fails fast and clearly with no half-built scratch dir to explain.
    let visible = sorted_visible(sources)?;
    let plan = resolve::resolve(sources, &visible)?;

    let scratch = std::env::temp_dir().join(format!("eo-build-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    match build(sources, &scratch, &visible, &plan) {
        Ok(dsk) => {
            if let Some(parent) = output.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent)?;
                }
            }
            fs::copy(&dsk, output)?;
            let _ = fs::remove_dir_all(&scratch);
            Ok(())
        }
        Err(e) => {
            eprintln!(
                "build-eo-image: build failed; intermediates left in {}",
                scratch.display()
            );
            Err(e)
        }
    }
}

/// Compile and link everything inside `scratch`, returning the path to the
/// finished disk image (`scratch/Oberon.dsk`). Mirrors `build-image`'s pipeline.
fn build(
    sources: &Path,
    scratch: &Path,
    visible: &[String],
    plan: &[resolve::Candidate],
) -> io::Result<PathBuf> {
    fs::create_dir_all(scratch)?;
    let toolchain = mksubdir(scratch, "toolchain")?;
    extract_toolchain(&toolchain)?;
    let norebo_dir = mksubdir(scratch, "norebo")?;
    let compiler_dir = mksubdir(scratch, "compiler")?;
    let oberon_dir = mksubdir(scratch, "oberon")?;

    eprintln!("Building the host toolchain");
    compile(
        NOREBO_MODULES,
        &norebo_dir,
        &[toolchain.clone(), sources.to_path_buf()],
    )?;
    // EO's CoreLinker reads `.rsx`, so the to-be-linked objects are renamed out of
    // the way of the live `.rsc` the shim loads to run it (as in build-image).
    bulk_rename(&norebo_dir, "rsc", "rsx")?;
    run_checked(
        &["CoreLinker.LinkSerial", "Modules", "InnerCore"],
        &norebo_dir,
        std::slice::from_ref(&toolchain),
    )?;
    bulk_rename(&norebo_dir, "rsx", "rsc")?;
    // Sanity: the fresh glue inner core must match the committed golden.
    if fs::read(norebo_dir.join("InnerCore"))? == GOLDEN_INNER_CORE {
        eprintln!("  inner core reproduces the golden bootstrap");
    } else {
        eprintln!("  warning: rebuilt inner core differs from the embedded golden seed");
    }

    eprintln!("Building a cross-compiler");
    let std_path = [
        sources.to_path_buf(),
        compiler_dir.clone(),
        norebo_dir.clone(),
    ];
    compile(
        &["ORS.Mod", "ORB.Mod", "ORG.Mod", "ORP.Mod"],
        &compiler_dir,
        &std_path,
    )?;

    // Drop symbol files so the full build links against the real (source-tree)
    // modules rather than the host-side (glue) core.
    bulk_delete(&norebo_dir, "smb")?;
    bulk_delete(&compiler_dir, "smb")?;

    eprintln!("Compiling {} module(s) from the source tree", plan.len());
    let order: Vec<&str> = plan.iter().map(|c| c.file.as_str()).collect();
    compile(&order, &oberon_dir, &std_path)?;
    for c in plan {
        let rsc = oberon_dir.join(format!("{}.rsc", c.module));
        if !rsc.exists() {
            return Err(io::Error::other(format!(
                "{} (MODULE {}) did not compile (no {})",
                c.file,
                c.module,
                rsc.display()
            )));
        }
    }

    eprintln!("Linking the inner core onto the disk");
    bulk_rename(&oberon_dir, "rsc", "rsx")?;
    run_checked(
        &["CoreLinker.LinkDisk", "Modules", "Oberon.dsk"],
        scratch,
        &[oberon_dir.clone(), norebo_dir.clone()],
    )?;

    eprintln!("Installing files");
    let mut install = vec![
        "VDiskUtil.InstallFiles".to_string(),
        "Oberon.dsk".to_string(),
    ];
    for name in visible {
        install.push(format!("{name}=>{name}"));
    }
    // The freshly built objects (renamed .rsc->.rsx above) and their symbol files,
    // each mapped back to the .rsc name the on-target loader expects.
    for rsx in files_with_ext(&oberon_dir, "rsx")? {
        let rsc = Path::new(&rsx).with_extension("rsc");
        install.push(format!("{rsx}=>{}", rsc.display()));
    }
    for smb in files_with_ext(&oberon_dir, "smb")? {
        install.push(format!("{smb}=>{smb}"));
    }
    let install: Vec<&str> = install.iter().map(String::as_str).collect();
    run_checked(
        &install,
        scratch,
        &[oberon_dir, sources.to_path_buf(), norebo_dir],
    )?;

    Ok(scratch.join("Oberon.dsk"))
}

/// Write the embedded toolchain seed into `dir`.
fn extract_toolchain(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    for (name, bytes) in TOOLCHAIN {
        fs::write(dir.join(name), bytes)?;
    }
    Ok(())
}

/// Run one `ORP.Compile a/s b/s …` over `modules` (the `/s` selects strict
/// EO/Oberon-07 mode), erroring on a non-zero exit.
fn compile(modules: &[&str], cwd: &Path, path: &[PathBuf]) -> io::Result<()> {
    let mut args = vec!["ORP.Compile".to_string()];
    args.extend(modules.iter().map(|m| format!("{m}/s")));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run_checked(&args, cwd, path)
}

/// Run one Oberon command through the shim, erroring on a non-zero exit code.
fn run_checked(args: &[&str], cwd: &Path, path: &[PathBuf]) -> io::Result<()> {
    let owned: Vec<String> = args.iter().map(|&s| s.to_owned()).collect();
    let code = run(&owned, cwd, path)?;
    if code != 0 {
        return Err(io::Error::other(format!(
            "{} exited with code {code}",
            args.first().copied().unwrap_or("?")
        )));
    }
    Ok(())
}

fn mksubdir(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let p = parent.join(name);
    fs::create_dir(&p)?;
    Ok(p)
}

/// Rename every `*.old_ext` in `dir` to `*.new_ext`.
fn bulk_rename(dir: &Path, old_ext: &str, new_ext: &str) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let p = entry?.path();
        if p.extension().and_then(|e| e.to_str()) == Some(old_ext) {
            fs::rename(&p, p.with_extension(new_ext))?;
        }
    }
    Ok(())
}

/// Delete every `*.ext` in `dir`.
fn bulk_delete(dir: &Path, ext: &str) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let p = entry?.path();
        if p.extension().and_then(|e| e.to_str()) == Some(ext) {
            fs::remove_file(&p)?;
        }
    }
    Ok(())
}

/// Non-hidden entries of `dir`, sorted (the install order).
fn sorted_visible(dir: &Path) -> io::Result<Vec<String>> {
    let mut names: Vec<String> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    Ok(names)
}

/// File names in `dir` with extension `ext`, sorted — a deterministic install order.
fn files_with_ext(dir: &Path, ext: &str) -> io::Result<Vec<String>> {
    let mut names: Vec<String> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| Path::new(n).extension().and_then(|e| e.to_str()) == Some(ext))
        .collect();
    names.sort();
    Ok(names)
}
