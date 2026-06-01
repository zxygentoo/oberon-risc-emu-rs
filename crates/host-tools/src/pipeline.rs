//! The shared disk-image build pipeline behind `build-po-image` and
//! `build-eo-image`.
//!
//! Both tools do the same thing — compile a whole Oberon source tree against an
//! embedded host toolchain, link a fresh inner core, and assemble a bootable
//! `Oberon.dsk` — and differ only in the embedded [`Seed`] (the host glue plus the
//! bootstrap objects that let the headless [`shim`](crate::shim) run the first
//! compile) and a name for messages. That pipeline lives here; each binary supplies
//! only its `Seed` and CLI. A Rust take on project-norebo's `build-image.py`,
//! driving the shim one Oberon command at a time.

use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::resolve;
use crate::shim::run;

/// Modules compiled to seed the host toolchain, then linked into a fresh inner
/// core (project-norebo's `build_norebo` set). The host versions of
/// `Kernel`/`Files`/`Oberon`/`FileDir`/`CoreLinker`/`VDisk…` come from the embedded
/// seed; the rest (`Modules`/`Fonts`/`Texts`/`RS232`/`OR*`) from the source tree.
/// Identical for PO2013 and EO — only the embedded glue sources differ.
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

/// The `.packonly` manifest section appended to each builder's `--help`. Shared:
/// the manifest format and rules are identical for PO2013 and EO.
pub const PACKONLY_HELP: &str = "\
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

  extract-source generates .packonly.";

/// What sets one builder apart from the other: the embedded toolchain seed and a
/// name for messages. The compile/link/install pipeline ([`build`]) is shared.
pub struct Seed {
    /// Host-glue `.Mod` sources plus the prebuilt bootstrap `.rsc`/`InnerCore` that
    /// seed the first compile, written flat into a scratch toolchain directory at
    /// runtime. Flat names never collide (`.Mod` vs `.rsc`).
    pub toolchain: &'static [(&'static str, &'static [u8])],

    /// The committed golden inner core. The inner core freshly linked during the
    /// build must reproduce it byte-for-byte (the compiler and `CoreLinker` are
    /// deterministic) — a self-consistency check on the embedded seed.
    pub golden_inner_core: &'static [u8],

    /// The tool's name, used in messages and the scratch-directory name (e.g.
    /// `"build-po-image"`).
    pub name: &'static str,
}

/// Build a bootable disk image from the source tree `sources` into `output`, using
/// `seed`'s embedded toolchain.
///
/// Compiles in a temp scratch directory and, on success, copies just the finished
/// `Oberon.dsk` to `output` — the intermediate compile trees are thrown away. On
/// failure the scratch dir is left behind for inspection.
pub fn build(seed: &Seed, sources: &Path, output: &Path) -> io::Result<()> {
    // Settle what to compile before touching the toolchain, so a bad source tree
    // (no .packonly, a data file left off it, a duplicate module, an import cycle)
    // fails fast and clearly — with no half-built scratch dir to explain.
    let visible = sorted_visible(sources)?;
    let plan = resolve::resolve(sources, &visible)?;

    let scratch = std::env::temp_dir().join(format!("{}-{}", seed.name, std::process::id()));
    let _ = fs::remove_dir_all(&scratch); // clear any stale run
    match run_pipeline(seed, sources, &scratch, &visible, &plan) {
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
                "{}: build failed; intermediates left in {}",
                seed.name,
                scratch.display()
            );
            Err(e)
        }
    }
}

/// Compile and link everything inside `scratch`, returning the path to the finished
/// disk image (`scratch/Oberon.dsk`).
fn run_pipeline(
    seed: &Seed,
    sources: &Path,
    scratch: &Path,
    visible: &[String],
    plan: &[resolve::Candidate],
) -> io::Result<PathBuf> {
    fs::create_dir_all(scratch)?;
    let toolchain = mksubdir(scratch, "toolchain")?;
    extract_toolchain(seed.toolchain, &toolchain)?;
    let norebo_dir = mksubdir(scratch, "norebo")?;
    let compiler_dir = mksubdir(scratch, "compiler")?;
    let oberon_dir = mksubdir(scratch, "oberon")?;

    eprintln!("Building the host toolchain");
    compile(
        NOREBO_MODULES,
        &norebo_dir,
        &[toolchain.clone(), sources.to_path_buf()],
    )?;
    // The offline CoreLinker reads `.rsx`, so the to-be-linked objects are renamed
    // out of the way of the live `.rsc` the shim loads to *run* the linker.
    bulk_rename(&norebo_dir, "rsc", "rsx")?;
    run_checked(
        &["CoreLinker.LinkSerial", "Modules", "InnerCore"],
        &norebo_dir,
        std::slice::from_ref(&toolchain),
    )?;
    bulk_rename(&norebo_dir, "rsx", "rsc")?;
    // Self-check: the fresh inner core must reproduce the committed golden seed.
    if fs::read(norebo_dir.join("InnerCore"))? == seed.golden_inner_core {
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
    // Fail loudly on a module that produced no object rather than shipping a broken
    // image. Objects are named by the module, not the source file.
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
    // taken from what actually compiled rather than a fixed list, each mapped back
    // to the .rsc name the on-target loader expects.
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

/// Write a toolchain seed (`name -> bytes`) flat into `dir`.
fn extract_toolchain(toolchain: &[(&str, &[u8])], dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    for (name, bytes) in toolchain {
        fs::write(dir.join(name), bytes)?;
    }
    Ok(())
}

/// Run one `ORP.Compile a/s b/s …` over `modules` (the `/s` selects strict
/// Oberon-07 mode), erroring on a non-zero exit.
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
    use super::{bulk_delete, bulk_rename, extract_toolchain, sorted_visible};
    use std::fs;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("image-test-{}-{tag}", std::process::id()));
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
    fn extract_toolchain_writes_every_entry() {
        let dir = scratch("toolchain");
        let table: &[(&str, &[u8])] = &[
            ("InnerCore", b"core"),  // the boot seed
            ("Kernel.Mod", b"glue"), // a host glue source
            ("ORP.rsc", b"object"),  // a bootstrap object
        ];
        extract_toolchain(table, &dir).unwrap();
        assert_eq!(fs::read_dir(&dir).unwrap().count(), table.len());
        assert!(dir.join("InnerCore").exists());
        assert!(dir.join("Kernel.Mod").exists());
        assert!(dir.join("ORP.rsc").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
