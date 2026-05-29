//! `build-image` — assemble a Project Oberon disk image with our in-process
//! `shim` engine (a Rust take on project-norebo's `build-image.py`).
//!
//! Usage: `build-image <sources_dir> <output.dsk>`
//!
//! `sources_dir` is a Project Oberon source tree fetched by project-norebo's
//! `fetch-sources.py` (Wirth's PO2013 sources plus the emulator's `Input`/
//! `Display`, fonts, etc.). The Norebo host modules and the bootstrap inner core
//! are embedded in this binary (`assets/`, vendored from project-norebo), so no
//! external checkout is needed. Only the finished image is written, to `<output.dsk>`
//! (intermediate compile trees live in a temp scratch dir and are removed).
//!
//! Every file in `sources_dir` is compiled as Oberon source and packed into the
//! image, except those named in the tree's required `.packonly` manifest, which
//! are packed verbatim (data files, and reference modules not meant to compile);
//! see [`resolve`]. Custom modules thus need no special handling: drop them
//! in, leave them off `.packonly`, and they compile and pack like any other.

use std::path::{Path, PathBuf};
use std::process::exit;
use std::{fs, io};

use clap::Parser;

use host_tools::resolve;
use host_tools::shim::run;

/// Modules compiled to seed the host toolchain, then linked into a fresh inner
/// core (project-norebo's `build_norebo` set). The host versions of
/// `Kernel`/`Files`/`Oberon`/`FileDir`/`CoreLinker`/`VDisk…` come from the
/// embedded toolchain; the rest (`Modules`/`Fonts`/`Texts`/`RS232`/`OR*`) from sources.
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

/// The embedded toolchain seed, vendored from project-norebo (see `assets/`):
/// the Norebo host `*.Mod` sources and the prebuilt `Bootstrap/` objects + inner
/// core. Names are flat (`.Mod` vs `.rsc` never collide) and written to a single
/// scratch search directory at runtime.
const TOOLCHAIN: &[(&str, &[u8])] = &[
    (
        "Norebo.Mod",
        include_bytes!("../../../assets/Norebo/Norebo.Mod"),
    ),
    (
        "Kernel.Mod",
        include_bytes!("../../../assets/Norebo/Kernel.Mod"),
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
        "Oberon.Mod",
        include_bytes!("../../../assets/Norebo/Oberon.Mod"),
    ),
    (
        "CoreLinker.Mod",
        include_bytes!("../../../assets/Norebo/CoreLinker.Mod"),
    ),
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
    (
        "InnerCore",
        include_bytes!("../../../assets/Bootstrap/InnerCore"),
    ),
    (
        "Kernel.rsc",
        include_bytes!("../../../assets/Bootstrap/Kernel.rsc"),
    ),
    (
        "FileDir.rsc",
        include_bytes!("../../../assets/Bootstrap/FileDir.rsc"),
    ),
    (
        "Files.rsc",
        include_bytes!("../../../assets/Bootstrap/Files.rsc"),
    ),
    (
        "Modules.rsc",
        include_bytes!("../../../assets/Bootstrap/Modules.rsc"),
    ),
    (
        "Norebo.rsc",
        include_bytes!("../../../assets/Bootstrap/Norebo.rsc"),
    ),
    (
        "Oberon.rsc",
        include_bytes!("../../../assets/Bootstrap/Oberon.rsc"),
    ),
    (
        "CoreLinker.rsc",
        include_bytes!("../../../assets/Bootstrap/CoreLinker.rsc"),
    ),
    (
        "Fonts.rsc",
        include_bytes!("../../../assets/Bootstrap/Fonts.rsc"),
    ),
    (
        "Texts.rsc",
        include_bytes!("../../../assets/Bootstrap/Texts.rsc"),
    ),
    (
        "RS232.rsc",
        include_bytes!("../../../assets/Bootstrap/RS232.rsc"),
    ),
    (
        "ORS.rsc",
        include_bytes!("../../../assets/Bootstrap/ORS.rsc"),
    ),
    (
        "ORB.rsc",
        include_bytes!("../../../assets/Bootstrap/ORB.rsc"),
    ),
    (
        "ORG.rsc",
        include_bytes!("../../../assets/Bootstrap/ORG.rsc"),
    ),
    (
        "ORP.rsc",
        include_bytes!("../../../assets/Bootstrap/ORP.rsc"),
    ),
];

/// The `.packonly` manifest section appended to `--help` (see [`resolve`]).
const PACKONLY_HELP: &str = "\
The .packonly manifest:
  Every file in SOURCES_DIR is compiled as Oberon source and packed into the
  image, except those listed in `.packonly` (at the tree root), which are packed
  verbatim: data such as fonts and tools, and reference modules that ship as
  source but are not meant to compile.

  The manifest is required; an empty one compiles everything. One file name per
  line; blank lines and `#` comments are ignored.

  Custom modules need no special handling — drop your .Mod files into the tree,
  leave them off .packonly, and they compile (in dependency order, worked out from
  their IMPORT lists) and pack like any other module. A file left off .packonly
  that is not valid Oberon source fails the build with a clear error.

  extract-source generates .packonly; for a fetch-sources.py tree, derive it from
  the source manifest's non-`source` rows.";

/// Build a runnable Project Oberon disk image from a source tree.
///
/// Compiles Project Oberon with the embedded Norebo toolchain and assembles a
/// bootable disk image. The sources are fetched separately (e.g. by
/// project-norebo's `fetch-sources.py`); only the finished image is written.
#[derive(Parser, Debug)]
#[command(name = "build-image", version, after_long_help = PACKONLY_HELP)]
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
    if let Err(e) = build_image(&cli.sources, &cli.output) {
        eprintln!("build-image: {e}");
        exit(1);
    }
    println!("Done: {}", cli.output.display());
}

/// Build the image in a temp scratch directory and, on success, copy just the
/// finished `Oberon.dsk` to `output` — the intermediate compile trees are thrown
/// away. On failure the scratch dir is left behind for inspection.
fn build_image(sources: &Path, output: &Path) -> io::Result<()> {
    // Settle what to compile before touching the toolchain, so a bad source tree
    // (no .packonly, a data file left off it, a duplicate module, an import cycle)
    // fails fast and clearly — with no half-built scratch dir to explain.
    let visible = sorted_visible(sources)?;
    let plan = resolve::resolve(sources, &visible)?;

    let scratch = std::env::temp_dir().join(format!("norebo-build-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch); // clear any stale run
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
                "build-image: build failed; intermediates left in {}",
                scratch.display()
            );
            Err(e)
        }
    }
}

/// Compile and link everything inside `scratch`, returning the path to the
/// finished disk image (`scratch/Oberon.dsk`).
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
    bulk_rename(&norebo_dir, "rsc", "rsx")?;
    run_checked(
        &["CoreLinker.LinkSerial", "Modules", "InnerCore"],
        &norebo_dir,
        &[toolchain],
    )?;
    bulk_rename(&norebo_dir, "rsx", "rsc")?;

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

    // Drop symbol files so the full build can't accidentally link against the
    // host-side (Norebo) core modules.
    bulk_delete(&norebo_dir, "smb")?;
    bulk_delete(&compiler_dir, "smb")?;

    eprintln!("Compiling {} module(s) from the source tree", plan.len());
    let order: Vec<&str> = plan.iter().map(|c| c.file.as_str()).collect();
    compile(&order, &oberon_dir, &std_path)?;
    // Fail loudly on a module that produced no object rather than shipping a
    // broken image. Objects are named by the module, not the source file.
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

    eprintln!("Linking the Inner Core");
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
    // The freshly built objects (renamed .rsc->.rsx above) and their symbol
    // files, taken from what actually compiled rather than a fixed list, each
    // mapped back to the .rsc name the on-target loader expects.
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

/// Run one `ORP.Compile a/s b/s …` over `modules`.
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

#[cfg(test)]
mod tests {
    use super::{bulk_delete, bulk_rename, extract_toolchain, sorted_visible, TOOLCHAIN};
    use std::fs;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("build-image-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bulk_rename_only_touches_matching_extensions() {
        let dir = scratch("rename");
        fs::write(dir.join("A.rsc"), b"a").unwrap();
        fs::write(dir.join("B.rsc"), b"b").unwrap();
        fs::write(dir.join("keep.smb"), b"k").unwrap();
        bulk_rename(&dir, "rsc", "rsx").unwrap();
        assert!(dir.join("A.rsx").exists());
        assert!(dir.join("B.rsx").exists());
        assert!(!dir.join("A.rsc").exists());
        assert!(dir.join("keep.smb").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bulk_delete_only_removes_matching_extensions() {
        let dir = scratch("delete");
        fs::write(dir.join("A.smb"), b"a").unwrap();
        fs::write(dir.join("B.rsc"), b"b").unwrap();
        bulk_delete(&dir, "smb").unwrap();
        assert!(!dir.join("A.smb").exists());
        assert!(dir.join("B.rsc").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sorted_visible_skips_dotfiles_and_sorts() {
        let dir = scratch("visible");
        fs::write(dir.join("b.txt"), b"").unwrap();
        fs::write(dir.join("a.txt"), b"").unwrap();
        fs::write(dir.join(".hidden"), b"").unwrap();
        assert_eq!(sorted_visible(&dir).unwrap(), ["a.txt", "b.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_toolchain_writes_the_embedded_seed() {
        let dir = scratch("toolchain");
        extract_toolchain(&dir).unwrap();
        assert_eq!(fs::read_dir(&dir).unwrap().count(), TOOLCHAIN.len());
        assert!(dir.join("InnerCore").exists()); // the boot seed
        assert!(dir.join("Kernel.Mod").exists()); // a Norebo host module
        assert!(dir.join("ORP.rsc").exists()); // a Bootstrap object
        fs::remove_dir_all(&dir).unwrap();
    }
}
