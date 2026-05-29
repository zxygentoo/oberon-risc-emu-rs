//! End-to-end tests for the `ob2unix`, `asciidecoder`, `build-image`, and
//! `extract-source` binaries.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use risc_core::disk::Disk;
use risc_core::headless;
use risc_core::risc::Risc;

const OB2UNIX: &str = env!("CARGO_BIN_EXE_ob2unix");
const ASCIIDECODER: &str = env!("CARGO_BIN_EXE_asciidecoder");
const EXTRACT_SOURCE: &str = env!("CARGO_BIN_EXE_extract-source");
const BUILD_IMAGE: &str = env!("CARGO_BIN_EXE_build-image");

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

// Build an Oberon Text: magic 0xF0 0x01, then a little-endian header length,
// then the body. `header_len` counts the whole header, including these 6 bytes.
fn oberon_text(header_len: u32, body: &[u8]) -> Vec<u8> {
    let mut v = vec![
        0xF0,
        0x01,
        header_len as u8,
        (header_len >> 8) as u8,
        (header_len >> 16) as u8,
        (header_len >> 24) as u8,
    ];
    v.resize(header_len as usize, 0);
    v.extend_from_slice(body);
    v
}

/// Write `bytes` to a uniquely named file under the test target dir and return
/// its path. `ob2unix` takes the text to convert as a FILE argument.
fn input_file(name: &str, bytes: &[u8]) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, bytes).expect("failed to write input file");
    path
}

#[test]
fn ob2unix_converts_oberon_text() {
    let f = input_file("ob2unix_convert.bin", &oberon_text(8, b"A\rB\rC"));
    let out = run(OB2UNIX, &[f.to_str().unwrap()], b"");
    assert!(out.status.success());
    assert_eq!(out.stdout, b"A\nB\nC");
}

#[test]
fn ob2unix_converts_body_larger_than_buffer() {
    // 3000 > the 1024-byte internal buffer, so the read/convert loop iterates.
    let f = input_file("ob2unix_large.bin", &oberon_text(6, &vec![b'\r'; 3000]));
    let out = run(OB2UNIX, &[f.to_str().unwrap()], b"");
    assert!(out.status.success());
    assert_eq!(out.stdout, vec![b'\n'; 3000]);
}

#[test]
fn ob2unix_passes_through_non_oberon_file() {
    // No Oberon magic: copied verbatim, CR kept.
    let f = input_file("ob2unix_plain.bin", b"plain\r text");
    let out = run(OB2UNIX, &[f.to_str().unwrap()], b"");
    assert!(out.status.success());
    assert_eq!(out.stdout, b"plain\r text");
}

#[test]
fn ob2unix_handles_empty_file() {
    let f = input_file("ob2unix_empty.bin", b"");
    let out = run(OB2UNIX, &[f.to_str().unwrap()], b"");
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

#[test]
fn ob2unix_help_goes_to_stdout() {
    let out = run(OB2UNIX, &["--help"], b"");
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Usage:"));
}

#[test]
fn ob2unix_requires_a_file_argument() {
    // No FILE: clap fails fast (exit 2) and points at --help, rather than hanging.
    let out = run(OB2UNIX, &[], b"");
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("required"));
    assert!(stderr.contains("--help"));
}

#[test]
fn ob2unix_errors_on_missing_file() {
    let out = run(OB2UNIX, &["does-not-exist.Mod"], b"");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("can't open 'does-not-exist.Mod'"));
}

#[test]
fn ob2unix_rejects_extra_argument() {
    let out = run(OB2UNIX, &["a.Mod", "b.Mod"], b"");
    assert_eq!(out.status.code(), Some(2)); // clap usage error
    assert!(String::from_utf8_lossy(&out.stderr).contains("unexpected argument"));
}

// "8U6%" is the AsciiCoder 6-bit encoding of the bytes "Hi".
const HI_ARCHIVE: &[u8] = b"AsciiCoder.DecodeFiles\nhi.txt ~ 8U6%";

#[test]
fn asciidecoder_extracts_into_directory() {
    let dir = scratch_dir("extract");
    let out = run(ASCIIDECODER, &["-C", dir.to_str().unwrap()], HI_ARCHIVE);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read(dir.join("hi.txt")).unwrap(), b"Hi");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn asciidecoder_verbose_lists_filenames() {
    let dir = scratch_dir("verbose");
    let out = run(
        ASCIIDECODER,
        &["-v", "-C", dir.to_str().unwrap()],
        HI_ARCHIVE,
    );
    assert!(out.status.success());
    assert_eq!(out.stdout, b"hi.txt\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn asciidecoder_errors_without_marker() {
    let out = run(ASCIIDECODER, &[], b"no archive marker here");
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no AsciiCoder.DecodeFiles archive found")
    );
}

#[test]
fn asciidecoder_help_goes_to_stdout() {
    let out = run(ASCIIDECODER, &["--help"], b"");
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Usage:"));
}

#[test]
fn asciidecoder_rejects_unknown_flag() {
    let out = run(ASCIIDECODER, &["-x"], b"");
    assert_eq!(out.status.code(), Some(2)); // clap usage error
    assert!(String::from_utf8_lossy(&out.stderr).contains("unexpected argument"));
}

// Heavy: compiles all of Project Oberon through the shim, so it's `#[ignore]`d —
// run with `cargo test -p host-tools -- --ignored`. Self-contained: it round-trips
// the committed golden image (extract its sources, including the generated
// `.packonly`, then rebuild) and checks the rebuild boots identically.
#[test]
#[ignore = "compiles all of Oberon via the shim; run with --ignored"]
fn build_image_round_trips_the_golden() {
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
    let status = Command::new(BUILD_IMAGE)
        .arg(&src)
        .arg(&dsk)
        .status()
        .expect("spawn build-image");
    assert!(status.success(), "build-image failed");

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

/// `build-image` rejects a tree with no `.packonly` (it's required), failing fast
/// — before the heavy toolchain build — with a clear message.
#[test]
fn build_image_requires_a_packonly() {
    let dir = scratch_dir("bi-no-packonly");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Foo.Mod"), b"MODULE Foo; END Foo.").unwrap();
    let out = run(
        BUILD_IMAGE,
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
fn build_image_rejects_unlisted_non_source() {
    let dir = scratch_dir("bi-unlisted-data");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".packonly"), b"").unwrap(); // nothing pack-only...
    std::fs::write(dir.join("Logo.Fnt"), [0u8, 1, 2, 3]).unwrap(); // ...but this is data
    let out = run(
        BUILD_IMAGE,
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
fn build_image_reports_a_duplicate_module() {
    let dir = scratch_dir("bi-dup-module");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".packonly"), b"").unwrap();
    std::fs::write(dir.join("A.Mod"), b"MODULE Same; END Same.").unwrap();
    std::fs::write(dir.join("B.Mod"), b"MODULE Same; END Same.").unwrap();
    let out = run(
        BUILD_IMAGE,
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
