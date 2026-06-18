//! Headless "shim" runtime: drive the [`Risc`] CPU from an inner-core image,
//! mapping Oberon's `Kernel`/`Files`/`FileDir` operations onto the host
//! filesystem. A Rust port of `project-norebo`'s `Runtime/norebo.c`.
//!
//! Unlike `norebo.c`, which trusts the guest, the syscall ABI bounds-checks
//! everything guest-supplied: out-of-RAM reads yield zeros and writes are
//! dropped, transfer lengths clamp to RAM, and a file is capped at 1 GiB — so
//! a corrupt inner core degrades into clean errors instead of host faults.
//!
//! This is the CPU's *second* execution mode, the counterpart to the FPGA device
//! map in [`crate::io`]: rather than disk/display/keyboard devices, the whole MMIO
//! region routes to this host, and the machine boots an `InnerCore` image instead
//! of the boot ROM. The CPU hands the host every MMIO access; the byte offsets it
//! answers (`address - 0xFFFFFFC0`, i.e. `norebo.c`'s `adr + 64`) are: `0` ms clock,
//! `8` stdin/stdout, `12` status, `48`/`52`/`56` syscall args 2/1/0, `60` syscall
//! trigger / result.
//!
//! [`run`] runs one Oberon command (e.g. `ORP.Compile Foo.Mod/s`) to completion
//! and returns its exit code; the host-side `build-*-image` pipeline drives it
//! repeatedly to compile a whole Oberon system and assemble a disk image.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::name_char_ok;
use crate::risc::Risc;

/// Fixed RAM size (`norebo.c` `MemBytes`).
const MEM_BYTES: u32 = 8 * 1024 * 1024;
/// Stack origin (`norebo.c` `StackOrg`).
const STACK_ORG: u32 = 0x0008_0000;
/// Maximum simultaneously open files (`norebo.c` `MaxFiles`).
const MAX_FILES: usize = 500;
/// Cap on a shim file's size. Far above any real artifact (the largest, a full
/// disk image, is tens of MB), but low enough that a corrupt guest seek/length
/// can't zero-fill the host's memory.
const MAX_FILE_BYTES: usize = 1 << 30;
/// Oberon file-name length (`norebo.c` `NameLength`).
const NAME_LEN: usize = 32;
/// A fixed Oberon-encoded date (2024-05-27 12:00:00). File dates are recorded in
/// the directory but do not affect a freshly-booted image's framebuffer.
const OBERON_DATE: u32 = (24 << 26) | (5 << 22) | (27 << 17) | (12 << 12);

/// A byte-addressable view of the guest's RAM, wrapped per syscall from the
/// [`Risc`](crate::risc::Risc)'s word array so the handlers can read their
/// arguments and read/write file data directly in emulated memory (`norebo.c`
/// operates on a global `mem[]`; this is the borrow-safe equivalent — the `Risc`
/// lends its `ram` only for the duration of one `store`).
struct ShimMem<'a> {
    ram: &'a mut [u32],
}

impl<'a> ShimMem<'a> {
    fn new(ram: &'a mut [u32]) -> Self {
        Self { ram }
    }

    /// Read the byte at `adr`. Out-of-range reads yield 0 — a bad guest pointer
    /// must not take the host down (`norebo.c` would just fault).
    fn read_byte(&self, adr: u32) -> u8 {
        self.ram
            .get((adr / 4) as usize)
            .map_or(0, |w| (w >> ((adr % 4) * 8)) as u8)
    }

    /// Write the byte at `adr` (read-modify-write of its containing word).
    /// Out-of-range writes are dropped.
    fn write_byte(&mut self, adr: u32, value: u8) {
        let shift = (adr % 4) * 8;
        if let Some(w) = self.ram.get_mut((adr / 4) as usize) {
            *w = (*w & !(0xFFu32 << shift)) | (u32::from(value) << shift);
        }
    }

    /// Guest RAM size in bytes — the hard bound on any single transfer.
    fn len_bytes(&self) -> usize {
        self.ram.len() * 4
    }

    /// Copy `buf.len()` bytes from memory starting at `adr` into `buf`.
    fn read_bytes(&self, adr: u32, buf: &mut [u8]) {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = self.read_byte(adr + i as u32);
        }
    }

    /// Copy `buf` into memory starting at `adr`.
    fn write_bytes(&mut self, adr: u32, buf: &[u8]) {
        for (i, &b) in buf.iter().enumerate() {
            self.write_byte(adr + i as u32, b);
        }
    }
}

/// Run one Oberon command headlessly. `args` is the command and its parameters
/// (e.g. `["ORP.Compile", "Foo.Mod/s"]`); files are resolved relative to `cwd`,
/// with `path` searched read-only for inputs not found there. The inner core is
/// loaded from `cwd`/`path` just like the C runtime. Returns the guest exit code.
///
/// # Errors
/// Fails if the `InnerCore` image cannot be found/read or is malformed.
pub fn run(args: &[String], cwd: &Path, path: &[PathBuf]) -> io::Result<i32> {
    let image = find_file(cwd, path, "InnerCore")?;
    let host = Host::new(cwd.to_path_buf(), path.to_vec(), args.to_vec());

    let mut risc = Risc::new();
    risc.configure_shim(MEM_BYTES);
    risc.set_shim(Box::new(host));
    risc.boot_inner_core(&image, STACK_ORG)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    // `risc` (and the host it owns) drop at the end of this scope, flushing any
    // files the guest left open — so persistence happens before we return.
    Ok(risc.shim_run())
}

/// Look up `name` in `cwd`, then in each `path` directory; return its bytes.
fn find_file(cwd: &Path, path: &[PathBuf], name: &str) -> io::Result<Vec<u8>> {
    if let Ok(bytes) = fs::read(cwd.join(name)) {
        return Ok(bytes);
    }
    for dir in path {
        if let Ok(bytes) = fs::read(dir.join(name)) {
            return Ok(bytes);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("can't find '{name}' in cwd or search path"),
    ))
}

/// One open file. New (unregistered) files are anonymous in-memory buffers;
/// `Register`/`Old` give them a `persist` path that is rewritten on close/drop
/// if dirty (read-only search-path opens keep `persist == None`).
struct OpenFile {
    data: Vec<u8>,
    pos: u64,
    name: String,
    persist: Option<PathBuf>,
    registered: bool,
    dirty: bool,
}

impl OpenFile {
    fn flush(&mut self) {
        if self.dirty {
            if let Some(p) = &self.persist {
                if let Err(e) = fs::write(p, &self.data) {
                    eprintln!("shim: can't write '{}': {e}", p.display());
                }
            }
            self.dirty = false;
        }
    }
}

/// The shim host: command-line args, the open-file table, and the syscall ABI.
pub(crate) struct Host {
    cwd: PathBuf,
    path: Vec<PathBuf>,
    args: Vec<String>,
    sysarg: [u32; 3],
    sysres: u32,
    files: Vec<Option<OpenFile>>,
    enumerate: std::vec::IntoIter<String>,
    exit: Option<i32>,
    start: Instant,
    out: io::BufWriter<io::Stdout>,
}

impl Host {
    fn new(cwd: PathBuf, path: Vec<PathBuf>, args: Vec<String>) -> Self {
        Host {
            cwd,
            path,
            args,
            sysarg: [0; 3],
            sysres: 0,
            files: (0..MAX_FILES).map(|_| None).collect(),
            enumerate: Vec::new().into_iter(),
            exit: None,
            start: Instant::now(),
            out: io::BufWriter::new(io::stdout()),
        }
    }

    fn file_mut(&mut self, h: u32) -> Option<&mut OpenFile> {
        self.files.get_mut(h as usize).and_then(Option::as_mut)
    }

    fn allocate(&mut self, file: OpenFile) -> u32 {
        for (i, slot) in self.files.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(file);
                return i as u32;
            }
        }
        self.exit = Some(1);
        eprintln!("shim: too many open files");
        u32::MAX
    }

    /// Dispatch syscall `n` with the latched arguments. Port of `sysreq_exec`.
    fn sysreq(&mut self, n: u32, mem: &mut ShimMem) -> u32 {
        let [a0, a1, a2] = self.sysarg;
        match n {
            1 => {
                // Norebo.Halt
                self.exit = Some(a0 as i32);
                0
            }
            2 => self.args.len() as u32, // Norebo.Argc
            3 => self.argv(a0, a1, a2, mem),
            4 => self.trap(a0, a1, a2, mem),
            11 => self.files_new(a0, mem),
            12 => self.files_old(a0, mem),
            13 => self.files_register(a0),
            14 => self.files_close(a0),
            15 => self.files_seek(a0, a1, a2),
            16 => self.file_mut(a0).map_or(u32::MAX, |f| f.pos as u32), // Files.Tell
            17 => self.files_read(a0, a1, a2, mem),
            18 => self.files_write(a0, a1, a2, mem),
            19 => self.file_mut(a0).map_or(u32::MAX, |f| f.data.len() as u32), // Files.Length
            20 => OBERON_DATE, // Files.Date (fixed; does not affect a booted image)
            21 => self.files_delete(a0, mem),
            22 => 0, // Files.Purge: no-op (unused by the toolchain)
            23 => self.files_rename(a0, a1, mem),
            31 => self.enumerate_begin(),
            32 => self.enumerate_next(a0, mem),
            33 => {
                self.enumerate = Vec::new().into_iter();
                0
            }
            _ => {
                eprintln!("shim: unimplemented syscall {n}");
                self.exit = Some(1);
                0
            }
        }
    }

    fn argv(&mut self, idx: u32, adr: u32, siz: u32, mem: &mut ShimMem) -> u32 {
        let Some(arg) = self.args.get(idx as usize) else {
            return u32::MAX;
        };
        let bytes = arg.as_bytes();
        if siz > 0 {
            let n = bytes.len().min(siz as usize - 1);
            mem.write_bytes(adr, &bytes[..n]);
            for j in n..siz as usize {
                mem.write_byte(adr + j as u32, 0);
            }
        }
        bytes.len() as u32
    }

    fn trap(&mut self, trap: u32, name_adr: u32, pos: u32, mem: &mut ShimMem) -> u32 {
        let msg = match trap {
            1 => "array index out of range",
            2 => "type guard failure",
            3 => "array or string copy overflow",
            4 => "access via NIL pointer",
            5 => "illegal procedure call",
            6 => "integer division by zero",
            7 => "assertion violated",
            _ => "unknown trap",
        };
        let name = read_name(mem, name_adr).unwrap_or_else(|| "(unknown)".to_string());
        eprintln!("shim: {msg} at {name} pos {pos}");
        self.exit = Some(100 + trap as i32);
        0
    }

    fn files_new(&mut self, adr: u32, mem: &mut ShimMem) -> u32 {
        let Some(name) = read_name(mem, adr) else {
            return u32::MAX;
        };
        self.allocate(OpenFile {
            data: Vec::new(),
            pos: 0,
            name,
            persist: None,
            registered: false,
            dirty: false,
        })
    }

    fn files_old(&mut self, adr: u32, mem: &mut ShimMem) -> u32 {
        let Some(name) = read_name(mem, adr) else {
            return u32::MAX;
        };
        // First the working directory (read-write), then the search path (read-only).
        let cwd_path = self.cwd.join(&name);
        if let Ok(data) = fs::read(&cwd_path) {
            return self.allocate(OpenFile {
                data,
                pos: 0,
                name,
                persist: Some(cwd_path),
                registered: true,
                dirty: false,
            });
        }
        for dir in &self.path {
            if let Ok(data) = fs::read(dir.join(&name)) {
                return self.allocate(OpenFile {
                    data,
                    pos: 0,
                    name,
                    persist: None, // read-only
                    registered: true,
                    dirty: false,
                });
            }
        }
        u32::MAX
    }

    fn files_register(&mut self, h: u32) -> u32 {
        let cwd = self.cwd.clone();
        if let Some(f) = self.file_mut(h) {
            if !f.registered && !f.name.is_empty() {
                let p = cwd.join(&f.name);
                if let Err(e) = fs::write(&p, &f.data) {
                    eprintln!("shim: can't create '{}': {e}", p.display());
                    return u32::MAX;
                }
                f.persist = Some(p);
                f.registered = true;
                f.dirty = false;
            }
        }
        0
    }

    fn files_close(&mut self, h: u32) -> u32 {
        if let Some(slot) = self.files.get_mut(h as usize) {
            if let Some(mut f) = slot.take() {
                f.flush();
            }
        }
        0
    }

    fn files_seek(&mut self, h: u32, pos: u32, whence: u32) -> u32 {
        if let Some(f) = self.file_mut(h) {
            let len = f.data.len() as i64;
            let cur = f.pos as i64;
            let base = match whence {
                1 => cur,
                2 => len,
                _ => 0,
            };
            f.pos = (base + i64::from(pos as i32)).max(0) as u64;
        }
        0
    }

    fn files_read(&mut self, h: u32, adr: u32, siz: u32, mem: &mut ShimMem) -> u32 {
        // The destination is guest RAM, so a transfer can't meaningfully exceed
        // it; clamping keeps a corrupt length from forcing a giant allocation.
        let siz = (siz as usize).min(mem.len_bytes());
        let Some(f) = self.file_mut(h) else {
            return 0;
        };
        let start = f.pos as usize;
        let avail = f.data.len().saturating_sub(start);
        let n = siz.min(avail);
        let mut buf = vec![0u8; siz]; // tail is zero-filled, as in `norebo.c`
        buf[..n].copy_from_slice(&f.data[start..start + n]);
        f.pos += n as u64;
        mem.write_bytes(adr, &buf);
        n as u32
    }

    fn files_write(&mut self, h: u32, adr: u32, siz: u32, mem: &mut ShimMem) -> u32 {
        let siz = (siz as usize).min(mem.len_bytes()); // the source is guest RAM
        let Some(f) = self.file_mut(h) else {
            return 0;
        };
        let start = f.pos as usize;
        let end = start + siz;
        if end > MAX_FILE_BYTES {
            eprintln!(
                "shim: write to '{}' would exceed the {} MiB file cap",
                f.name,
                MAX_FILE_BYTES >> 20
            );
            return 0;
        }
        if f.data.len() < end {
            f.data.resize(end, 0);
        }
        mem.read_bytes(adr, &mut f.data[start..end]);
        f.pos = end as u64;
        f.dirty = true;
        siz as u32
    }

    fn files_delete(&mut self, adr: u32, mem: &mut ShimMem) -> u32 {
        match read_name(mem, adr) {
            Some(name) if !name.is_empty() && fs::remove_file(self.cwd.join(&name)).is_ok() => 0,
            _ => u32::MAX,
        }
    }

    fn files_rename(&mut self, old_adr: u32, new_adr: u32, mem: &mut ShimMem) -> u32 {
        let (Some(old), Some(new)) = (read_name(mem, old_adr), read_name(mem, new_adr)) else {
            return u32::MAX;
        };
        if old.is_empty() || new.is_empty() {
            return u32::MAX;
        }
        if fs::rename(self.cwd.join(old), self.cwd.join(new)).is_ok() {
            0
        } else {
            u32::MAX
        }
    }

    fn enumerate_begin(&mut self) -> u32 {
        let mut names = Vec::new();
        if let Ok(rd) = fs::read_dir(&self.cwd) {
            for entry in rd.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if valid_name(name.as_bytes()) {
                        names.push(name.to_string());
                    }
                }
            }
        }
        self.enumerate = names.into_iter();
        0
    }

    fn enumerate_next(&mut self, adr: u32, mem: &mut ShimMem) -> u32 {
        if let Some(name) = self.enumerate.next() {
            let mut buf = [0u8; NAME_LEN];
            let bytes = name.as_bytes();
            let n = bytes.len().min(NAME_LEN - 1);
            buf[..n].copy_from_slice(&bytes[..n]);
            mem.write_bytes(adr, &buf);
            0
        } else {
            mem.write_byte(adr, 0);
            u32::MAX
        }
    }
}

impl Host {
    pub(crate) fn load(&mut self, offset: u32) -> u32 {
        match offset {
            0 => self.start.elapsed().as_millis() as u32, // millisecond clock
            8 => read_stdin_byte(),                       // getchar
            12 => 3,                                      // status (carried from Oberon)
            48 => self.sysarg[2],
            52 => self.sysarg[1],
            56 => self.sysarg[0],
            60 => self.sysres,
            _ => 0,
        }
    }

    pub(crate) fn store(&mut self, offset: u32, value: u32, ram: &mut [u32]) {
        match offset {
            8 => {
                // putchar
                let _ = self.out.write_all(&[value as u8]);
            }
            48 => self.sysarg[2] = value,
            52 => self.sysarg[1] = value,
            56 => self.sysarg[0] = value,
            // Only the syscall trigger reaches into guest memory, so wrap `ram` here.
            60 => self.sysres = self.sysreq(value, &mut ShimMem::new(ram)),
            // offset 4 (LEDs) and anything else: ignored.
            _ => {}
        }
    }

    pub(crate) fn exit_code(&self) -> Option<i32> {
        self.exit
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // Persist anything the guest left open, then flush stdout.
        for f in self.files.iter_mut().flatten() {
            f.flush();
        }
        let _ = self.out.flush();
    }
}

/// Read a 32-byte Oberon file name at `adr`, validating it. `Some("")` is a
/// valid (empty) name; `None` means an illegal character or no terminator.
fn read_name(mem: &ShimMem, adr: u32) -> Option<String> {
    let mut buf = [0u8; NAME_LEN];
    mem.read_bytes(adr, &mut buf);
    let mut s = String::new();
    for (i, &ch) in buf.iter().enumerate() {
        if ch == 0 {
            return Some(s);
        }
        if !name_char_ok(i, ch) {
            return None;
        }
        s.push(char::from(ch));
    }
    None
}

/// Whether `bytes` (a NUL-terminated name, or a directory entry) is a legal
/// Oberon file name, matching `norebo.c`'s `files_check_name`.
fn valid_name(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() >= NAME_LEN {
        return false;
    }
    for (i, &ch) in bytes.iter().enumerate() {
        if !name_char_ok(i, ch) {
            return false;
        }
    }
    true
}

/// One byte from stdin, or `0xFFFF_FFFF` at EOF (`norebo.c`'s `getchar` convention).
fn read_stdin_byte() -> u32 {
    let mut b = [0u8; 1];
    match io::stdin().lock().read(&mut b) {
        Ok(1) => u32::from(b[0]),
        _ => u32::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ShimMem: byte addressing over the guest's word array ---

    #[test]
    fn shimmem_packs_bytes_little_endian() {
        let mut ram = vec![0u32; 1];
        {
            let mut mem = ShimMem::new(&mut ram);
            mem.write_byte(0, 0x00);
            mem.write_byte(1, 0x11);
            mem.write_byte(2, 0x22);
            mem.write_byte(3, 0x33);
        }
        assert_eq!(ram[0], 0x3322_1100); // little-endian within the word
    }

    #[test]
    fn shimmem_write_byte_preserves_neighbors() {
        let mut ram = vec![0xFFFF_FFFFu32];
        {
            let mut mem = ShimMem::new(&mut ram);
            mem.write_byte(2, 0x00); // clear only the third byte
            assert_eq!(mem.read_byte(2), 0x00);
            assert_eq!(mem.read_byte(3), 0xFF);
        }
        assert_eq!(ram[0], 0xFF00_FFFF);
    }

    #[test]
    fn shimmem_bytes_round_trip_across_a_word_boundary() {
        let mut ram = vec![0u32; 4];
        let mut mem = ShimMem::new(&mut ram);
        let src = [1u8, 2, 3, 4, 5, 6]; // 6 bytes from adr 2 spans two words
        mem.write_bytes(2, &src);
        let mut got = [0u8; 6];
        mem.read_bytes(2, &mut got);
        assert_eq!(got, src);
    }

    // --- name validation (read_name reads guest memory; valid_name is pure) ---

    fn name_at_zero(bytes: &[u8]) -> Option<String> {
        let mut ram = vec![0u32; NAME_LEN]; // ample room
        let mut mem = ShimMem::new(&mut ram);
        mem.write_bytes(0, bytes);
        read_name(&mem, 0)
    }

    #[test]
    fn read_name_stops_at_the_nul() {
        assert_eq!(
            name_at_zero(b"Kernel.Mod\0junk").as_deref(),
            Some("Kernel.Mod")
        );
    }

    #[test]
    fn read_name_empty_is_ok_but_unterminated_is_rejected() {
        assert_eq!(name_at_zero(b"\0").as_deref(), Some("")); // leading NUL
        assert_eq!(name_at_zero(&[b'A'; NAME_LEN]), None); // no terminator in 32 bytes
    }

    #[test]
    fn read_name_rejects_illegal_chars() {
        assert_eq!(name_at_zero(b"a/b\0"), None); // path separator
        assert_eq!(name_at_zero(b"9bad\0"), None); // leading digit
    }

    #[test]
    fn valid_name_enforces_length_and_charset() {
        assert!(valid_name(b"Oberon10.Scn.Fnt"));
        assert!(!valid_name(b"")); // empty
        assert!(!valid_name(&[b'A'; NAME_LEN])); // >= NAME_LEN
        assert!(!valid_name(b"bad name")); // space is illegal
    }

    // --- Host file ABI, exercised directly (no CPU, no disk) ---

    fn host() -> Host {
        // A cwd that does not exist: the in-memory tests never persist, and the
        // `files_old` case wants the host-side read to simply fail.
        Host::new(std::path::PathBuf::from("/nonexistent-shim-test"), vec![], vec![])
    }

    #[test]
    fn files_new_write_seek_read_round_trips_in_memory() {
        let mut ram = vec![0u32; 64];
        let mut mem = ShimMem::new(&mut ram);
        mem.write_bytes(0, b"Scratch\0");
        let mut h = host();
        let fd = h.files_new(0, &mut mem);
        assert_ne!(fd, u32::MAX);

        let payload = b"hello oberon";
        mem.write_bytes(64, payload);
        assert_eq!(
            h.files_write(fd, 64, payload.len() as u32, &mut mem),
            payload.len() as u32
        );

        h.files_seek(fd, 0, 0); // whence 0 = SET
        assert_eq!(
            h.files_read(fd, 128, payload.len() as u32, &mut mem),
            payload.len() as u32
        );
        let mut got = vec![0u8; payload.len()];
        mem.read_bytes(128, &mut got);
        assert_eq!(got, payload);
    }

    #[test]
    fn files_read_past_eof_zero_fills() {
        let mut ram = vec![0u32; 64];
        let mut mem = ShimMem::new(&mut ram);
        mem.write_bytes(0, b"Scratch\0");
        let mut h = host();
        let fd = h.files_new(0, &mut mem);
        mem.write_bytes(64, &[0xAA, 0xBB]);
        h.files_write(fd, 64, 2, &mut mem);
        h.files_seek(fd, 0, 0);

        mem.write_bytes(128, &[0xFF; 4]); // dirty the destination first
        assert_eq!(h.files_read(fd, 128, 4, &mut mem), 2); // only 2 available
        let mut got = [0u8; 4];
        mem.read_bytes(128, &mut got);
        assert_eq!(got, [0xAA, 0xBB, 0x00, 0x00]); // tail zero-filled
    }

    #[test]
    fn files_new_rejects_an_illegal_name() {
        let mut ram = vec![0u32; 16];
        let mut mem = ShimMem::new(&mut ram);
        mem.write_bytes(0, b"a/b\0");
        assert_eq!(host().files_new(0, &mut mem), u32::MAX);
    }

    #[test]
    fn files_old_on_a_missing_file_returns_max() {
        let mut ram = vec![0u32; 16];
        let mut mem = ShimMem::new(&mut ram);
        mem.write_bytes(0, b"Nope.Mod\0");
        assert_eq!(host().files_old(0, &mut mem), u32::MAX);
    }

    #[test]
    fn syscalls_survive_wild_guest_pointers_and_lengths() {
        // A bad pointer or length must not panic the host or balloon its
        // memory: out-of-range reads yield zeros, writes vanish, transfer
        // lengths clamp to guest RAM.
        let mut ram = vec![0u32; 16]; // 64 bytes of guest RAM
        let mut mem = ShimMem::new(&mut ram);
        assert_eq!(mem.read_byte(1 << 20), 0);
        mem.write_byte(1 << 20, 0xAB); // dropped

        let mut h = host();
        mem.write_bytes(0, b"Scratch\0");
        let fd = h.files_new(0, &mut mem);
        assert_ne!(fd, u32::MAX);
        // A huge read length clamps to RAM; the file is empty, so 0 bytes read.
        assert_eq!(h.files_read(fd, 0, u32::MAX, &mut mem), 0);
        // A name pointer outside RAM reads as all-NUL -> the empty (valid) name.
        assert_eq!(read_name(&mem, 1 << 20).as_deref(), Some(""));
    }

    #[test]
    fn files_write_past_the_size_cap_is_refused() {
        let mut ram = vec![0u32; 64];
        let mut mem = ShimMem::new(&mut ram);
        mem.write_bytes(0, b"Scratch\0");
        let mut h = host();
        let fd = h.files_new(0, &mut mem);
        // Seek to ~2 GiB and write: refused outright, not a 2 GiB zero-fill.
        h.files_seek(fd, i32::MAX as u32, 0); // whence 0 = SET
        assert_eq!(h.files_write(fd, 0, 4, &mut mem), 0);
        assert_eq!(h.file_mut(fd).unwrap().data.len(), 0, "file must not grow");
    }
}
