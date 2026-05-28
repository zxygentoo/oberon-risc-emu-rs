//! Headless "shim" runtime: drive the [`risc_core`] CPU from an inner-core
//! image, mapping Oberon's `Kernel`/`Files`/`FileDir` operations onto the host
//! filesystem. A Rust port of `project-norebo`'s `Runtime/norebo.c`.
//!
//! [`run`] runs one Oberon command (e.g. `ORP.Compile Foo.Mod/s`) to
//! completion and returns its exit code; the `build-image` binary drives it
//! repeatedly to compile Project Oberon and assemble a disk image.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use risc_core::io::{ShimHost, ShimMem};
use risc_core::risc::Risc;

/// Fixed RAM size (`norebo.c` `MemBytes`).
const MEM_BYTES: u32 = 8 * 1024 * 1024;
/// Stack origin (`norebo.c` `StackOrg`).
const STACK_ORG: u32 = 0x0008_0000;
/// Maximum simultaneously open files (`norebo.c` `MaxFiles`).
const MAX_FILES: usize = 500;
/// Oberon file-name length (`norebo.c` `NameLength`).
const NAME_LEN: usize = 32;

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
struct Host {
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
        let Some(f) = self.file_mut(h) else {
            return 0;
        };
        let start = f.pos as usize;
        let avail = f.data.len().saturating_sub(start);
        let n = (siz as usize).min(avail);
        let mut buf = vec![0u8; siz as usize]; // tail is zero-filled, as in `norebo.c`
        buf[..n].copy_from_slice(&f.data[start..start + n]);
        f.pos += n as u64;
        mem.write_bytes(adr, &buf);
        n as u32
    }

    fn files_write(&mut self, h: u32, adr: u32, siz: u32, mem: &mut ShimMem) -> u32 {
        let Some(f) = self.file_mut(h) else {
            return 0;
        };
        let start = f.pos as usize;
        let end = start + siz as usize;
        if f.data.len() < end {
            f.data.resize(end, 0);
        }
        mem.read_bytes(adr, &mut f.data[start..end]);
        f.pos = end as u64;
        f.dirty = true;
        siz
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

impl ShimHost for Host {
    fn load(&mut self, offset: u32, _mem: &mut ShimMem) -> u32 {
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

    fn store(&mut self, offset: u32, value: u32, mem: &mut ShimMem) {
        match offset {
            8 => {
                // putchar
                let _ = self.out.write_all(&[value as u8]);
            }
            48 => self.sysarg[2] = value,
            52 => self.sysarg[1] = value,
            56 => self.sysarg[0] = value,
            60 => self.sysres = self.sysreq(value, mem),
            // offset 4 (LEDs) and anything else: ignored.
            _ => {}
        }
    }

    fn exit_code(&self) -> Option<i32> {
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
        let ok = ch.is_ascii_alphabetic() || (i > 0 && (ch == b'.' || ch.is_ascii_digit()));
        if !ok {
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
        let ok = ch.is_ascii_alphabetic() || (i > 0 && (ch == b'.' || ch.is_ascii_digit()));
        if !ok {
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

/// A fixed Oberon-encoded date (2024-05-27 12:00:00). File dates are recorded in
/// the directory but do not affect a freshly-booted image's framebuffer.
const OBERON_DATE: u32 = (24 << 26) | (5 << 22) | (27 << 17) | (12 << 12);
