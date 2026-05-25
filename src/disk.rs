//! The SPI SD-card state machine (port of `disk.c`).
//!
//! Models just enough of the SD command protocol for Oberon: single-block read
//! (CMD17 / 81) and write (CMD24 / 88), driven byte-by-byte through the [`Spi`]
//! interface. A `.dsk` image is backed by a host file opened read+write; a
//! filesystem-only image (first word `0x9B1EA38D`) is detected and its sector
//! numbers are rebased by the fixed `0x80002` offset.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::io::Spi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiskState {
    Command,
    Read,
    Write,
    Writing,
}

/// An SD card attached to the SPI bus, backed by a `.dsk` host file.
pub struct Disk {
    state: DiskState,
    file: Option<File>,
    offset: u32,

    rx_buf: [u32; 128],
    rx_idx: usize,

    tx_buf: [u32; 128 + 2],
    tx_cnt: i32,
    tx_idx: i32,
}

impl Disk {
    /// Open a disk image (or build a diskless card with `None`, for
    /// `--boot-from-serial`). Port of `disk_new`.
    pub fn new(filename: Option<&Path>) -> std::io::Result<Self> {
        let mut disk = Disk {
            state: DiskState::Command,
            file: None,
            offset: 0,
            rx_buf: [0; 128],
            rx_idx: 0,
            tx_buf: [0; 130],
            tx_cnt: 0,
            tx_idx: 0,
        };

        if let Some(path) = filename {
            let mut file = OpenOptions::new().read(true).write(true).open(path)?;
            // Detect a filesystem-only image, which starts directly at sector 1
            // (DiskAdr 29): read sector 0 and check the magic word.
            read_sector(Some(&mut file), &mut disk.tx_buf[0..128]);
            disk.offset = if disk.tx_buf[0] == 0x9B1E_A38D {
                0x8_0002
            } else {
                0
            };
            disk.file = Some(file);
        }

        Ok(disk)
    }

    fn run_command(&mut self) {
        let cmd = self.rx_buf[0];
        let arg = (self.rx_buf[1] << 24)
            | (self.rx_buf[2] << 16)
            | (self.rx_buf[3] << 8)
            | self.rx_buf[4];

        match cmd {
            81 => {
                self.state = DiskState::Read;
                self.tx_buf[0] = 0;
                self.tx_buf[1] = 254;
                let secnum = arg.wrapping_sub(self.offset);
                if let Some(f) = self.file.as_mut() {
                    seek_sector(f, secnum);
                }
                read_sector(self.file.as_mut(), &mut self.tx_buf[2..130]);
                self.tx_cnt = 2 + 128;
            }
            88 => {
                self.state = DiskState::Write;
                let secnum = arg.wrapping_sub(self.offset);
                if let Some(f) = self.file.as_mut() {
                    seek_sector(f, secnum);
                }
                self.tx_buf[0] = 0;
                self.tx_cnt = 1;
            }
            _ => {
                self.tx_buf[0] = 0;
                self.tx_cnt = 1;
            }
        }
        self.tx_idx = -1;
    }
}

impl Spi for Disk {
    fn read_data(&mut self) -> u32 {
        if self.tx_idx >= 0 && self.tx_idx < self.tx_cnt {
            self.tx_buf[self.tx_idx as usize]
        } else {
            255
        }
    }

    fn write_data(&mut self, value: u32) {
        self.tx_idx += 1;
        match self.state {
            DiskState::Command => {
                if (value as u8) != 0xFF || self.rx_idx != 0 {
                    self.rx_buf[self.rx_idx] = value;
                    self.rx_idx += 1;
                    if self.rx_idx == 6 {
                        self.run_command();
                        self.rx_idx = 0;
                    }
                }
            }
            DiskState::Read => {
                if self.tx_idx == self.tx_cnt {
                    self.state = DiskState::Command;
                    self.tx_cnt = 0;
                    self.tx_idx = 0;
                }
            }
            DiskState::Write => {
                if value == 254 {
                    self.state = DiskState::Writing;
                }
            }
            DiskState::Writing => {
                if self.rx_idx < 128 {
                    self.rx_buf[self.rx_idx] = value;
                }
                self.rx_idx += 1;
                if self.rx_idx == 128 {
                    write_sector(self.file.as_mut(), &self.rx_buf);
                }
                if self.rx_idx == 130 {
                    self.tx_buf[0] = 5;
                    self.tx_cnt = 1;
                    self.tx_idx = -1;
                    self.rx_idx = 0;
                    self.state = DiskState::Command;
                }
            }
        }
    }
}

fn seek_sector(file: &mut File, secnum: u32) {
    // The C computes `secnum * 512` in 32-bit unsigned arithmetic.
    let _ = file.seek(SeekFrom::Start(secnum.wrapping_mul(512) as u64));
}

fn read_sector(file: Option<&mut File>, buf: &mut [u32]) {
    let mut bytes = [0u8; 512];
    if let Some(f) = file {
        // Read up to 512 bytes (short reads at EOF leave the rest zero, as the
        // C's zero-initialised buffer + fread does).
        let mut filled = 0;
        while filled < 512 {
            match f.read(&mut bytes[filled..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => filled += n,
            }
        }
    }
    for (i, w) in buf.iter_mut().enumerate().take(128) {
        *w = u32::from_le_bytes([
            bytes[i * 4],
            bytes[i * 4 + 1],
            bytes[i * 4 + 2],
            bytes[i * 4 + 3],
        ]);
    }
}

fn write_sector(file: Option<&mut File>, buf: &[u32]) {
    if let Some(f) = file {
        let mut bytes = [0u8; 512];
        for (i, w) in buf.iter().enumerate().take(128) {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        let _ = f.write_all(&bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A throwaway image file, removed on drop.
    struct TempImage {
        path: PathBuf,
    }
    impl TempImage {
        fn new(bytes: &[u8]) -> TempImage {
            let mut path = std::env::temp_dir();
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            path.push(format!("oberon_disk_test_{}_{n}.img", std::process::id()));
            std::fs::write(&path, bytes).unwrap();
            TempImage { path }
        }
    }
    impl Drop for TempImage {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn sector_bytes(words: &[u32]) -> Vec<u8> {
        let mut v = vec![0u8; 512];
        for (i, w) in words.iter().enumerate() {
            v[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        v
    }

    fn send_command(disk: &mut Disk, cmd: u32, arg: u32) {
        disk.write_data(cmd);
        disk.write_data((arg >> 24) & 0xFF);
        disk.write_data((arg >> 16) & 0xFF);
        disk.write_data((arg >> 8) & 0xFF);
        disk.write_data(arg & 0xFF);
        disk.write_data(0xFF); // CRC byte (ignored)
    }

    #[test]
    fn detects_filesystem_only_image_magic() {
        let mut img = vec![0u8; 1024];
        img[0..4].copy_from_slice(&0x9B1E_A38Du32.to_le_bytes());
        let t = TempImage::new(&img);
        let disk = Disk::new(Some(&t.path)).unwrap();
        assert_eq!(disk.offset, 0x8_0002);

        let plain = TempImage::new(&vec![0u8; 1024]);
        let disk2 = Disk::new(Some(&plain.path)).unwrap();
        assert_eq!(disk2.offset, 0);
    }

    #[test]
    fn read_command_returns_response_then_sector() {
        let mut img = vec![0u8; 512 * 4];
        let s1: Vec<u32> = (0..128).map(|i| 0x1000_0000 + i as u32).collect();
        img[512..1024].copy_from_slice(&sector_bytes(&s1));
        let t = TempImage::new(&img);

        let mut disk = Disk::new(Some(&t.path)).unwrap();
        assert_eq!(disk.offset, 0);

        send_command(&mut disk, 81, 1); // read sector 1
        let resp: Vec<u32> = (0..130)
            .map(|_| {
                disk.write_data(0xFF);
                disk.read_data()
            })
            .collect();

        assert_eq!(resp[0], 0, "R1 response");
        assert_eq!(resp[1], 254, "data token");
        assert_eq!(&resp[2..130], &s1[..], "sector payload");
        // Further reads after the payload yield idle 0xFF (255).
        disk.write_data(0xFF);
        assert_eq!(disk.read_data(), 255);
    }

    #[test]
    fn write_command_persists_sector() {
        let img = vec![0u8; 512 * 4];
        let t = TempImage::new(&img);
        let s2: Vec<u32> = (0..128).map(|i| 0xABCD_0000 + i as u32).collect();

        {
            let mut disk = Disk::new(Some(&t.path)).unwrap();
            send_command(&mut disk, 88, 2); // write sector 2
            disk.write_data(0xFF); // clock out R1
            assert_eq!(disk.read_data(), 0);
            disk.write_data(254); // data token -> Writing
            for &w in &s2 {
                disk.write_data(w);
            }
            disk.write_data(0xFF); // two trailing CRC bytes -> finalize
            disk.write_data(0xFF);
            // One more clock cycle to shift out the data-accepted token (5).
            disk.write_data(0xFF);
            assert_eq!(disk.read_data(), 5);
        }

        let written = std::fs::read(&t.path).unwrap();
        assert_eq!(&written[1024..1536], &sector_bytes(&s2)[..]);
    }

    #[test]
    fn diskless_reads_idle() {
        let mut disk = Disk::new(None).unwrap();
        disk.write_data(0xFF); // leading idle byte at rx_idx 0 is ignored
        assert_eq!(disk.read_data(), 255);
    }

    #[test]
    fn unknown_command_returns_single_status_byte() {
        let t = TempImage::new(&vec![0u8; 512]);
        let mut disk = Disk::new(Some(&t.path)).unwrap();
        send_command(&mut disk, 0, 0); // CMD0 (GO_IDLE) is not modelled
        disk.write_data(0xFF);
        assert_eq!(disk.read_data(), 0);
    }
}
