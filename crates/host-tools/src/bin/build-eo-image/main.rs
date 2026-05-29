//! `build-eo-image` — assemble a headless Extended Oberon (EO) system with our
//! in-process `shim` engine, the EO counterpart of `build-image`.
//!
//! Usage: `build-eo-image <sources_dir> <output_dir>`
//!
//! `sources_dir` is an Extended Oberon source tree (e.g. produced by
//! `extract-source` from an `RISC.img`); it supplies the stock EO modules
//! (`Modules`, `Fonts`, `Texts`, `RS232`, and the `OR*` compiler). The headless
//! host glue (`Kernel`/`Files`/`FileDir`/`Oberon`/`Norebo`/`CoreLinker`) and a
//! prebuilt EO bootstrap inner core are embedded in this binary (`assets/`), so
//! no external toolchain is needed.
//!
//! What it produces (into `output_dir`): a fresh `InnerCore` — the bootable EO
//! inner core, in the `(len,addr,bytes)` serial format the `shim`/`eo-shim`
//! runtime loads — plus the compiled `.rsc`/`.smb` objects of the whole EO
//! toolchain. Point `eo-shim` at `output_dir` to compile, link, and run EO
//! commands headlessly:
//!
//! ```text
//! build-eo-image eo-sources/ eo-system/
//! eo-shim eo-system/ ORP.Compile Foo.Mod/s     # compile a module
//! eo-shim eo-system/ Foo.Bar                    # run a command
//! ```
//!
//! How it differs from `build-image` (Project Oberon 2013):
//!  * The inner-core top module is **`Modules`** (whose body runs `Init` →
//!    `Files.Init` → `Kernel.Init`, then `Load("Oberon")`), so the standard EO
//!    boot sequence initialises the heap and dynamically loads the rest. Linking
//!    `Oberon` as the top — as a self-contained core — does *not* boot: only the
//!    top body runs, so `Kernel.Init` never fires.
//!  * EO's loader reads `.rsc` (in-system objects are not renamed `.rsx`), so
//!    there is no rsc→rsx dance: the freshly compiled objects are linked in place.
//!
//! Not yet produced: a GUI-bootable disk image. EO writes its boot file to the
//! disk's boot area with `ORL.Link`/`ORL.Load` via `Disk` sectors, which the
//! headless glue `Disk` deliberately stubs out; that step needs a sector backend
//! (or an EO `VDisk` port) and is left as follow-up. See BUILD-EO-IMAGE.md.

use std::path::{Path, PathBuf};
use std::process::exit;
use std::{fs, io};

use clap::Parser;

use host_tools::shim::run;

/// The embedded EO toolchain seed (vendored under `assets/`): the host-adapted
/// glue sources, plus the prebuilt bootstrap objects + inner core that let the
/// `shim` run the EO compiler/linker for the first build. Names are flat and
/// written to a single scratch directory at runtime, mirroring `build-image`.
const SEED: &[(&str, &[u8])] = &[
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
    // Prebuilt bootstrap inner core + objects (the shim boots/loads these to run
    // the first compile + link). All compiled against the glue above.
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

/// The golden bootstrap inner core: a freshly built `InnerCore` must reproduce it
/// byte-for-byte (the EO compiler + `CoreLinker` are deterministic), which is the
/// build's self-consistency check.
const GOLDEN_INNER_CORE: &[u8] = include_bytes!("../../../assets/eo-Bootstrap/InnerCore");

/// The EO toolchain, in compile-then-link order. Each group is compiled in one
/// `ORP.Compile` (so a module sees its same-group dependencies' freshly written
/// `.smb`); groups run as separate shim boots, so each starts with a clean heap.
/// The host glue comes from [`SEED`]; the rest from the source tree.
const TOOLCHAIN_GROUPS: &[&[&str]] = &[
    &[
        "Norebo.Mod",
        "Kernel.Mod",
        "FileDir.Mod",
        "Files.Mod",
        "Modules.Mod",
    ],
    &["Fonts.Mod", "Texts.Mod", "RS232.Mod", "Oberon.Mod"],
    &["ORS.Mod", "ORB.Mod", "ORG.Mod", "ORP.Mod"],
    &["CoreLinker.Mod"],
];

/// Build a headless Extended Oberon system from a source tree.
#[derive(Parser, Debug)]
#[command(name = "build-eo-image", version)]
struct Cli {
    /// Extended Oberon source tree (e.g. from `extract-source` on an `RISC.img`)
    #[arg(value_name = "SOURCES_DIR")]
    sources: PathBuf,

    /// Directory to write the built EO system to (`InnerCore` + objects)
    #[arg(value_name = "OUTPUT_DIR")]
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

/// Build in a temp scratch directory and, on success, copy the finished system
/// (`InnerCore` + `.rsc`/`.smb`) to `output`. On failure the scratch dir is left
/// behind for inspection.
fn build_eo_image(sources: &Path, output: &Path) -> io::Result<()> {
    if !sources.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("sources dir not found: {}", sources.display()),
        ));
    }
    let scratch = std::env::temp_dir().join(format!("eo-build-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    match build(sources, &scratch) {
        Ok(system) => {
            fs::create_dir_all(output)?;
            copy_dir_flat(&system, output)?;
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

/// Compile and link inside `scratch`, returning the path to the directory holding
/// the finished `InnerCore` and toolchain objects.
fn build(sources: &Path, scratch: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(scratch)?;
    let toolchain = mksubdir(scratch, "toolchain")?;
    extract_seed(&toolchain)?;
    let system = mksubdir(scratch, "system")?;

    // Stage the source `.Mod` only — never any `.rsc`/`.smb` the tree may carry
    // (e.g. an `extract-source --keep-objects` tree). A stale `.smb` in the search
    // path makes the compiler skip writing the fresh one (it looks unchanged), so
    // the built system would ship `.rsc` without their `.smb` and later imports
    // would fail with "import not available". Compiling from clean sources forces
    // every `.smb` to be regenerated into `system`.
    let src = mksubdir(scratch, "src")?;
    stage_sources(sources, &src)?;

    // Compile the toolchain in the shim: read glue/sources, write fresh objects
    // to `system`. The shim runs the seed compiler (from `toolchain`); freshly
    // written objects in `system` (the cwd) shadow the seed for later groups.
    let path = [toolchain.clone(), src];
    for (i, group) in TOOLCHAIN_GROUPS.iter().enumerate() {
        eprintln!(
            "Compiling toolchain group {}/{} ({})",
            i + 1,
            TOOLCHAIN_GROUPS.len(),
            group.join(" ")
        );
        compile(group, &system, &path)?;
    }
    for group in TOOLCHAIN_GROUPS {
        for m in *group {
            let rsc = system.join(Path::new(m).with_extension("rsc"));
            if !rsc.exists() {
                return Err(io::Error::other(format!(
                    "{m} did not compile (no {})",
                    rsc.display()
                )));
            }
        }
    }

    // Link the fresh Modules-topped inner core (EO reads .rsc in place — no
    // rsc→rsx rename). The shim loads the freshly built modules from `system`.
    eprintln!("Linking the inner core (CoreLinker.LinkSerial Modules InnerCore)");
    run_checked(
        &["CoreLinker.LinkSerial", "Modules", "InnerCore"],
        &system,
        &[toolchain],
    )?;

    let inner = system.join("InnerCore");
    let built = fs::read(&inner)?;
    if built == GOLDEN_INNER_CORE {
        eprintln!(
            "Inner core reproduces the golden bootstrap ({} bytes)",
            built.len()
        );
    } else {
        eprintln!(
            "warning: inner core ({} bytes) differs from the embedded golden ({} bytes); \
             the seed may be stale relative to these sources",
            built.len(),
            GOLDEN_INNER_CORE.len()
        );
    }
    Ok(system)
}

/// Write the embedded toolchain seed into `dir`.
fn extract_seed(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    for (name, bytes) in SEED {
        fs::write(dir.join(name), bytes)?;
    }
    Ok(())
}

/// Copy the compilable sources (`*.Mod`) from `sources` into `dst`, leaving
/// behind any prebuilt `.rsc`/`.smb`/data the tree may carry. See [`build`] for
/// why stale objects in the compile path must be excluded.
fn stage_sources(sources: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(sources)? {
        let p = entry?.path();
        if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("Mod") {
            if let Some(name) = p.file_name() {
                fs::copy(&p, dst.join(name))?;
            }
        }
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

/// Copy every regular file in `from` directly into `to` (flat, non-recursive).
fn copy_dir_flat(from: &Path, to: &Path) -> io::Result<()> {
    for entry in fs::read_dir(from)? {
        let p = entry?.path();
        if p.is_file() {
            if let Some(name) = p.file_name() {
                fs::copy(&p, to.join(name))?;
            }
        }
    }
    Ok(())
}
