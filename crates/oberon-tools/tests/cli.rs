//! End-to-end tests for the `ob2unix`, `asciidecoder`, and `build-image` binaries.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use risc_core::disk::Disk;
use risc_core::headless;
use risc_core::risc::Risc;

const OB2UNIX: &str = env!("CARGO_BIN_EXE_ob2unix");
const ASCIIDECODER: &str = env!("CARGO_BIN_EXE_asciidecoder");

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

// Gated on OBERON_SOURCES (a fetched Project Oberon source tree); the PO2013
// sources aren't vendored, so default CI skips this. Point it at e.g.
// project-norebo's `upstream` directory.
#[test]
fn build_image_reproduces_the_boot_golden() {
    let Ok(sources) = std::env::var("OBERON_SOURCES") else {
        eprintln!("OBERON_SOURCES not set; skipping build-image golden test");
        return;
    };

    let dir = scratch_dir("build-image");
    let dsk = dir.join("Oberon.dsk");
    let status = Command::new(env!("CARGO_BIN_EXE_build-image"))
        .arg(&sources)
        .arg(&dsk)
        .status()
        .expect("spawn build-image");
    assert!(status.success(), "build-image failed");
    assert_eq!(
        std::fs::metadata(&dsk).expect("image produced").len(),
        990_208,
        "unexpected image size",
    );

    // The freshly built image must boot bit-identically to the C-derived golden
    // (frame 250), the same hashes risc-core's boot_matches_c_reference checks.
    let mut risc = Risc::new();
    risc.set_spi(1, Box::new(Disk::new(Some(&dsk)).expect("open disk")));
    headless::run_frames(&mut risc, 250);
    assert_eq!(headless::framebuffer_hash(&risc), 0xb9bd_bf56_ba51_298d);
    assert_eq!(headless::state_hash(&risc), 0x7531_e881_9ea3_aac1);

    let _ = std::fs::remove_dir_all(&dir);
}
