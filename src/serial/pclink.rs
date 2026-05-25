//! PCLink file transfer over the serial line (port of `pclink.c`).
//!
//! Watches for two job files (`PCLink.REC`, naming a host file to send *to*
//! Oberon, and `PCLink.SND`, naming a host file to receive *from* Oberon),
//! driving the framed byte protocol the Oberon `PCLink` tool speaks. The C
//! resolves these names relative to the working directory; we keep that default
//! but allow a base directory to be set, which makes the protocol testable.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::io::Serial;

const ACK: u32 = 0x10;
const REC: u32 = 0x21;
const SND: u32 = 0x22;

pub struct PcLink {
    dir: PathBuf,
    mode: u32, // 0 (idle), REC, or SND
    file: Option<File>,
    txcount: i32,
    rxcount: i32,
    fnlen: i32,
    flen: i64,
    filename: String,
    buf: [u8; 257],
}

impl Default for PcLink {
    fn default() -> Self {
        Self::in_dir(".")
    }
}

impl PcLink {
    /// Watch `./PCLink.REC` and `./PCLink.SND`, as the C does.
    pub fn new() -> Self {
        Self::default()
    }

    /// Watch job files and resolve transferred filenames under `dir`.
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        PcLink {
            dir: dir.into(),
            mode: 0,
            file: None,
            txcount: 0,
            rxcount: 0,
            fnlen: 0,
            flen: 0,
            filename: String::new(),
            buf: [0; 257],
        }
    }

    fn rec_name(&self) -> PathBuf {
        self.dir.join("PCLink.REC")
    }
    fn snd_name(&self) -> PathBuf {
        self.dir.join("PCLink.SND")
    }
    fn target(&self) -> PathBuf {
        self.dir.join(&self.filename)
    }

    /// Read the target filename from a job file (1..=33 bytes), resetting the
    /// transfer counters; delete the job file if it is unusable. Port of `GetJob`.
    fn get_job(&mut self, job_name: &Path) -> bool {
        let mut res = false;
        if let Ok(meta) = std::fs::metadata(job_name) {
            if meta.len() > 0 && meta.len() <= 33 {
                if let Ok(content) = std::fs::read_to_string(job_name) {
                    if let Some(tok) = content.split_whitespace().next() {
                        self.filename = tok.chars().take(31).collect();
                        res = true;
                        self.txcount = 0;
                        self.rxcount = 0;
                        self.fnlen = self.filename.len() as i32 + 1;
                    }
                }
            }
            if !res {
                let _ = std::fs::remove_file(job_name);
            }
        }
        res
    }
}

impl Serial for PcLink {
    fn read_status(&mut self) -> u32 {
        if self.mode == 0 {
            if self.get_job(&self.rec_name()) {
                // REC: send a host file to Oberon.
                if let Ok(meta) = std::fs::metadata(self.target()) {
                    if meta.len() < 0x0100_0000 {
                        if let Ok(f) = File::open(self.target()) {
                            self.flen = meta.len() as i64;
                            self.mode = REC;
                            self.file = Some(f);
                            println!("PCLink REC Filename: {} size {}", self.filename, self.flen);
                        }
                    }
                }
                if self.mode == 0 {
                    let _ = std::fs::remove_file(self.rec_name());
                }
            } else if self.get_job(&self.snd_name()) {
                // SND: receive a file from Oberon into a host file.
                if let Ok(f) = OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .read(true)
                    .write(true)
                    .open(self.target())
                {
                    self.flen = -1;
                    self.mode = SND;
                    self.file = Some(f);
                    println!("PCLink SND Filename: {}", self.filename);
                }
                if self.mode == 0 {
                    let _ = std::fs::remove_file(self.snd_name());
                }
            }
        }
        2 + if self.mode != 0 { 1 } else { 0 } // bit1: xmit ready; bit0: active
    }

    fn read_data(&mut self) -> u32 {
        let mut ch: u32 = 0;
        if self.mode != 0 {
            if self.rxcount == 0 {
                ch = self.mode;
            } else if self.rxcount < self.fnlen + 1 {
                // Filename bytes followed by its NUL terminator.
                let idx = (self.rxcount - 1) as usize;
                ch = self.filename.as_bytes().get(idx).copied().unwrap_or(0) as u32;
            } else if self.mode == SND {
                ch = ACK;
                if self.flen == 0 {
                    self.mode = 0;
                    let _ = std::fs::remove_file(self.snd_name());
                }
            } else {
                // REC payload, framed as 255-byte blocks each prefixed by a
                // length byte; a length < 255 (here 0) ends the transfer.
                let pos = (self.rxcount - self.fnlen - 1) % 256;
                if pos == 0 || self.flen == 0 {
                    if self.flen > 255 {
                        ch = 255;
                    } else {
                        ch = self.flen as u32;
                        if self.flen == 0 {
                            self.mode = 0;
                            let _ = std::fs::remove_file(self.rec_name());
                        }
                    }
                } else {
                    let mut b = [0u8; 1];
                    if let Some(f) = self.file.as_mut() {
                        let _ = f.read(&mut b);
                    }
                    ch = b[0] as u32;
                    self.flen -= 1;
                }
            }
        }
        self.rxcount += 1;
        ch
    }

    fn write_data(&mut self, value: u32) {
        if self.mode != 0 {
            if self.txcount == 0 {
                // The first byte must be ACK; anything else aborts the job.
                if value != ACK {
                    self.file = None;
                    if self.mode == SND {
                        let _ = std::fs::remove_file(self.target());
                        let _ = std::fs::remove_file(self.snd_name());
                    } else {
                        let _ = std::fs::remove_file(self.rec_name());
                    }
                    self.mode = 0;
                }
            } else if self.mode == SND {
                let pos = ((self.txcount - 1) % 256) as usize;
                self.buf[pos] = value as u8;
                let lim = self.buf[0] as usize;
                if pos == lim {
                    if let Some(f) = self.file.as_mut() {
                        let _ = f.write_all(&self.buf[1..1 + lim]);
                    }
                    if lim < 255 {
                        self.flen = 0;
                        self.file = None;
                    }
                }
            }
        }
        self.txcount += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct Scratch {
        dir: PathBuf,
    }
    impl Scratch {
        fn new() -> Scratch {
            let mut dir = std::env::temp_dir();
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            dir.push(format!("oberon_pclink_{}_{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch { dir }
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn idle_status_is_xmit_ready() {
        let s = Scratch::new();
        let mut pc = PcLink::in_dir(&s.dir);
        assert_eq!(pc.read_status(), 2); // tx ready, not active
    }

    #[test]
    fn rec_sends_host_file_to_oberon() {
        let s = Scratch::new();
        std::fs::write(s.dir.join("payload.txt"), b"Hello, Oberon!").unwrap();
        std::fs::write(s.dir.join("PCLink.REC"), "payload.txt").unwrap();

        let mut pc = PcLink::in_dir(&s.dir);
        assert_eq!(pc.read_status(), 3); // REC active

        assert_eq!(pc.read_data(), REC); // mode byte
        pc.write_data(ACK); // handshake

        // Filename + NUL terminator.
        let mut name = b"payload.txt".to_vec();
        name.push(0);
        for &b in &name {
            assert_eq!(pc.read_data() as u8, b);
        }

        // Block: length (14), the 14 payload bytes, then a 0-length terminator.
        assert_eq!(pc.read_data(), 14);
        let mut got = Vec::new();
        for _ in 0..14 {
            got.push(pc.read_data() as u8);
        }
        assert_eq!(&got, b"Hello, Oberon!");
        assert_eq!(pc.read_data(), 0); // end of file
        assert_eq!(pc.read_status(), 2); // transfer done, idle again
    }

    #[test]
    fn snd_receives_oberon_file_to_host() {
        let s = Scratch::new();
        std::fs::write(s.dir.join("PCLink.SND"), "out.txt").unwrap();

        let mut pc = PcLink::in_dir(&s.dir);
        assert_eq!(pc.read_status(), 3); // SND active

        assert_eq!(pc.read_data(), SND); // mode byte
        pc.write_data(ACK); // handshake

        // Advance past the echoed filename + NUL ("out.txt" + NUL = 8).
        for _ in 0..8 {
            pc.read_data();
        }

        // Oberon sends one block of "Hi": length byte then the two bytes.
        pc.write_data(2);
        pc.write_data(b'H' as u32);
        pc.write_data(b'i' as u32);

        assert_eq!(pc.read_data(), ACK); // completion; mode clears (flen == 0)
        assert_eq!(pc.read_status(), 2);
        assert_eq!(std::fs::read(s.dir.join("out.txt")).unwrap(), b"Hi");
    }
}
