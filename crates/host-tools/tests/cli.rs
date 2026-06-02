//! End-to-end tests for the `ob2txt`, `txt2ob`, `extract-source`,
//! `build-po-image`, and `build-eo-image` binaries.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use risc_core::disk::Disk;
use risc_core::headless;
use risc_core::risc::Risc;

const OB2TXT: &str = env!("CARGO_BIN_EXE_ob2txt");
const TXT2OB: &str = env!("CARGO_BIN_EXE_txt2ob");
const EXTRACT_SOURCE: &str = env!("CARGO_BIN_EXE_extract-source");
const BUILD_PO_IMAGE: &str = env!("CARGO_BIN_EXE_build-po-image");
const BUILD_EO_IMAGE: &str = env!("CARGO_BIN_EXE_build-eo-image");

/// Run `bin` with `args`, feed `input` on stdin, and capture its output.
fn run(bin: &str, args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn binary");
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(input)
        .expect("failed to write to stdin");
    child.wait_with_output().expect("failed to wait for binary")
}

/// A fresh scratch directory under the test target dir.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// `ob2txt FILE` writes `FILE.txt` (CR->LF, Latin-1->UTF-8); `txt2ob FILE.txt`
/// reverses it back to `FILE`. The pair must round-trip plain Oberon source
/// (ASCII + CR) byte-for-byte.
#[test]
fn ob2txt_then_txt2ob_round_trips() {
    let dir = scratch_dir("ob2txt-roundtrip");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("A.Mod");
    let original = b"MODULE A;\r  IMPORT B;\rEND A.\r";
    std::fs::write(&src, original).unwrap();

    // A.Mod -> A.Mod.txt: CR becomes LF, content otherwise intact.
    let out = run(OB2TXT, &[src.to_str().unwrap()], b"");
    assert!(
        out.status.success(),
        "ob2txt: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(dir.join("A.Mod.txt")).unwrap(),
        b"MODULE A;\n  IMPORT B;\nEND A.\n"
    );

    // A.Mod.txt -> A.Mod: LF back to CR, byte-identical to the original.
    let out = run(TXT2OB, &[dir.join("A.Mod.txt").to_str().unwrap()], b"");
    assert!(
        out.status.success(),
        "txt2ob: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read(&src).unwrap(), original);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Non-ASCII round-trips through Latin-1: byte 0xE4 (`ä`) <-> UTF-8 `ä`.
#[test]
fn ob2txt_round_trips_latin1() {
    let dir = scratch_dir("ob2txt-latin1");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("U.Mod");
    std::fs::write(&src, [0xE4u8, b'\r']).unwrap(); // 'ä', CR

    let out = run(OB2TXT, &[src.to_str().unwrap()], b"");
    assert!(out.status.success());
    assert_eq!(std::fs::read(dir.join("U.Mod.txt")).unwrap(), b"\xc3\xa4\n"); // UTF-8 'ä', LF

    let out = run(TXT2OB, &[dir.join("U.Mod.txt").to_str().unwrap()], b"");
    assert!(out.status.success());
    assert_eq!(std::fs::read(&src).unwrap(), [0xE4u8, b'\r']);

    let _ = std::fs::remove_dir_all(&dir);
}

/// `txt2ob` insists on a `.txt` input, so it can't overwrite the wrong file.
#[test]
fn txt2ob_requires_a_txt_suffix() {
    let dir = scratch_dir("txt2ob-suffix");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("A.Mod");
    std::fs::write(&f, b"x").unwrap();
    let out = run(TXT2OB, &[f.to_str().unwrap()], b"");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains(".txt"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A missing FILE is a clap usage error (exit 2, points at --help), not a hang.
#[test]
fn ob2txt_requires_a_file_argument() {
    let out = run(OB2TXT, &[], b"");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--help"));
}

// Heavy: compiles all of Project Oberon through the shim, so it's `#[ignore]`d —
// run with `cargo test -p host-tools -- --ignored`. Self-contained: it round-trips
// the committed golden image (extract its sources, including the generated
// `.packonly`, then rebuild) and checks the rebuild boots identically.
#[test]
#[ignore = "compiles all of Oberon via the shim; run with --ignored"]
fn build_po_image_round_trips_the_golden() {
    let Some(img) = golden_image() else {
        eprintln!("golden image not present; skipping round-trip test");
        return;
    };

    let src = scratch_dir("round-trip-src");
    let out = run(
        EXTRACT_SOURCE,
        &[img.to_str().unwrap(), src.to_str().unwrap()],
        b"",
    );
    assert!(
        out.status.success(),
        "extract-source failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let dst = scratch_dir("round-trip-build");
    let dsk = dst.join("Oberon.dsk");
    let status = Command::new(BUILD_PO_IMAGE)
        .arg(&src)
        .arg(&dsk)
        .status()
        .expect("spawn build-po-image");
    assert!(status.success(), "build-po-image failed");

    // The boot hashes come from the running machine (modules load by name, not by
    // disk layout), so the rebuild must boot to the C-derived golden even though
    // its on-disk byte layout may differ from the original image.
    let mut risc = Risc::new();
    risc.set_spi(1, Box::new(Disk::new(Some(&dsk)).expect("open disk")));
    headless::run_frames(&mut risc, 250);
    assert_eq!(headless::framebuffer_hash(&risc), 0xb9bd_bf56_ba51_298d);
    assert_eq!(headless::state_hash(&risc), 0x7531_e881_9ea3_aac1);

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}

/// The committed golden disk image (repo `DiskImage/`), resolved relative to this
/// crate. `extract-source` reads it directly (no boot), so this is cheap and
/// hermetic. Returns `None` (and the test skips) if the image isn't present.
fn golden_image() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../DiskImage/Oberon-2020-08-18.dsk");
    p.exists().then_some(p)
}

#[test]
fn extract_source_yields_a_build_ready_tree() {
    let Some(img) = golden_image() else {
        eprintln!("golden image not present; skipping extract-source test");
        return;
    };
    // `scratch_dir` clears any stale run; `extract-source` creates the directory.
    let dir = scratch_dir("extract-source");
    let out = run(
        EXTRACT_SOURCE,
        &[img.to_str().unwrap(), dir.to_str().unwrap()],
        b"",
    );
    assert!(
        out.status.success(),
        "extract-source failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Compiled artifacts are dropped (38 .rsc + 38 .smb); 60 sources remain, plus
    // the generated .packonly manifest.
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let sources: Vec<&String> = names.iter().filter(|n| n.as_str() != ".packonly").collect();
    assert_eq!(sources.len(), 60, "expected 60 source files");
    assert!(
        !sources.iter().any(|n| matches!(
            Path::new(n).extension().and_then(|e| e.to_str()),
            Some("rsc" | "smb")
        )),
        "compiled artifacts were not skipped"
    );

    // The manifest marks the data files and the reference modules that ship as
    // source with no object, but not a module that actually compiled.
    let pack = host_tools::packonly::parse(
        &std::fs::read_to_string(dir.join(".packonly")).expect(".packonly generated"),
    );
    assert_eq!(pack.len(), 22, "expected 22 pack-only entries");
    assert!(pack.contains("Display.Orig.Mod")); // reference original, never compiled
    assert!(pack.contains("BootLoad.Mod")); // hardware module, not in the build
    assert!(pack.contains("Oberon10.Scn.Fnt")); // a font (data)
    assert!(!pack.contains("Kernel.Mod")); // a compiled module's source

    // A known module came out byte-complete and as readable source.
    let kernel = std::fs::read(dir.join("Kernel.Mod")).expect("Kernel.Mod extracted");
    assert_eq!(kernel.len(), 9986, "unexpected Kernel.Mod size");
    assert!(
        kernel.starts_with(b"MODULE Kernel;"),
        "Kernel.Mod is not the expected source"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `build-po-image` rejects a tree with no `.packonly` (it's required), failing
/// fast — before the heavy toolchain build — with a clear message.
#[test]
fn build_po_image_requires_a_packonly() {
    let dir = scratch_dir("bi-no-packonly");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Foo.Mod"), b"MODULE Foo; END Foo.").unwrap();
    let out = run(
        BUILD_PO_IMAGE,
        &[dir.to_str().unwrap(), dir.join("out.dsk").to_str().unwrap()],
        b"",
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(".packonly"),
        "expected a .packonly error, got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A non-source file left off `.packonly` is a compile candidate, so the build
/// fails clearly and points back at `.packonly` rather than feeding it to ORP.
#[test]
fn build_po_image_rejects_unlisted_non_source() {
    let dir = scratch_dir("bi-unlisted-data");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".packonly"), b"").unwrap(); // nothing pack-only...
    std::fs::write(dir.join("Logo.Fnt"), [0u8, 1, 2, 3]).unwrap(); // ...but this is data
    let out = run(
        BUILD_PO_IMAGE,
        &[dir.to_str().unwrap(), dir.join("out.dsk").to_str().unwrap()],
        b"",
    );
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("Logo.Fnt") && err.contains(".packonly"),
        "got: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two candidates that declare the same module collide (the forgotten-skip-entry
/// backstop), and the build says so instead of silently clobbering one object.
#[test]
fn build_po_image_reports_a_duplicate_module() {
    let dir = scratch_dir("bi-dup-module");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".packonly"), b"").unwrap();
    std::fs::write(dir.join("A.Mod"), b"MODULE Same; END Same.").unwrap();
    std::fs::write(dir.join("B.Mod"), b"MODULE Same; END Same.").unwrap();
    let out = run(
        BUILD_PO_IMAGE,
        &[dir.to_str().unwrap(), dir.join("out.dsk").to_str().unwrap()],
        b"",
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("MODULE Same"),
        "got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ----------------------- Extended Oberon (build-eo-image) -----------------------

/// Copy the committed EO bootstrap seed (`InnerCore` + glue `.rsc`) into `dir`.
fn stage_eo_seed(dir: &Path) -> PathBuf {
    let seed = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/eo/bootstrap");
    std::fs::create_dir_all(dir).unwrap();
    for entry in std::fs::read_dir(&seed).expect("eo/bootstrap seed present") {
        let p = entry.unwrap().path();
        std::fs::copy(&p, dir.join(p.file_name().unwrap())).unwrap();
    }
    dir.to_path_buf()
}

/// The committed EO inner core boots headlessly under the `shim` and runs the
/// whole compile→load→execute path: it compiles a trivial module (`Tiny`, no
/// imports — all the seed has objects for) and then runs the freshly built
/// command. Hermetic and self-contained (the seed is vendored), so it guards the
/// EO bring-up without an external source tree. Heavier than a unit test (it
/// boots a CPU and dynamically loads the EO compiler) but far lighter than a full
/// system build.
#[test]
fn eo_seed_boots_compiles_and_runs() {
    let dir = scratch_dir("eo-seed-smoke");
    stage_eo_seed(&dir);
    std::fs::write(
        dir.join("Tiny.Mod"),
        b"MODULE Tiny;\n  PROCEDURE Go*;\n  BEGIN\n  END Go;\nEND Tiny.\n",
    )
    .unwrap();

    let path = [dir.clone()];
    let compile = risc_core::shim::run(&["ORP.Compile".into(), "Tiny.Mod/s".into()], &dir, &path)
        .expect("shim run (compile)");
    assert_eq!(compile, 0, "compiling Tiny in the EO seed should succeed");
    assert!(
        dir.join("Tiny.rsc").exists(),
        "ORP.Compile produced no Tiny.rsc"
    );

    let go = risc_core::shim::run(&["Tiny.Go".into()], &dir, &path).expect("shim run (Go)");
    assert_eq!(go, 0, "running the freshly compiled Tiny.Go should succeed");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Boot `disk` headless until the desktop settles and return the framebuffer hash.
/// `Disk::new` auto-detects the sector offset (full SD `RISC.img` vs raw `.dsk`).
fn boot_framebuffer_hash(disk: &Path) -> u64 {
    let mut risc = Risc::new();
    risc.set_spi(1, Box::new(Disk::new(Some(disk)).expect("open disk")));
    headless::run_frames(&mut risc, 1000);
    headless::framebuffer_hash(&risc)
}

/// Full `build-eo-image` round-trip: extract an Extended Oberon image's sources,
/// rebuild a disk, and check the rebuild boots **identically** to the original — to
/// the same EO desktop framebuffer. Modules load by name, not by disk layout, so a
/// faithful rebuild reaches the very same screen despite a different byte layout.
///
/// Needs an EO image (the AP 1.1.26 `RISC.img` is ~270 MB, not vendored): point
/// `EO_IMAGE` at one (a full SD `RISC.img` or a `.dsk`) — skipped otherwise. Heavy
/// (compiles the whole EO system through the shim), so it's `#[ignore]`d.
#[test]
#[ignore = "needs EO_IMAGE; rebuilds all of EO via the shim; run with --ignored"]
fn build_eo_image_round_trips_a_bootable_desktop() {
    let Some(img) = std::env::var_os("EO_IMAGE").map(PathBuf::from) else {
        eprintln!("EO_IMAGE not set; skipping build-eo-image round-trip");
        return;
    };

    let src = scratch_dir("eo-rt-src");
    let out = run(
        EXTRACT_SOURCE,
        &[img.to_str().unwrap(), src.to_str().unwrap()],
        b"",
    );
    assert!(
        out.status.success(),
        "extract-source failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let dst = scratch_dir("eo-rt-build");
    let dsk = dst.join("Oberon.dsk");
    let status = Command::new(BUILD_EO_IMAGE)
        .arg(&src)
        .arg(&dsk)
        .status()
        .expect("spawn build-eo-image");
    assert!(status.success(), "build-eo-image failed");

    let built = boot_framebuffer_hash(&dsk);
    let original = boot_framebuffer_hash(&img);
    assert_eq!(
        built, original,
        "rebuilt disk does not boot to the same desktop as the original EO image"
    );

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}
