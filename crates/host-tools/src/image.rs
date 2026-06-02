//! A read-only reader for the Project Oberon on-disk filesystem — the format
//! implemented by `FileDir.Mod`/`Files.Mod` and stored in a `.dsk` image. It
//! parses the directory B-tree and reconstructs file contents straight from the
//! image bytes; no emulator, no boot.
//!
//! Layout (see `assets/common/VFileDir.Mod`): the image is a flat array of
//! 1024-byte sectors, with sector `s` at byte offset `(s - 1) * 1024` and on-disk
//! pointers stored as `DiskAdr = s * 29`. The directory is a B-tree rooted at
//! `DiskAdr` 29 (sector 1); each file's first sector is a header holding its name,
//! length, and the table of data-sector addresses. Files up to 64 pages live in
//! the header's `sec` table; larger ones spill into index sectors via `ext`.
//!
//! A raw `.dsk` holds that sector array from byte 0; a full SD-card image (the
//! `RISC.img` Extended Oberon and the FPGA install ship) places it behind a fixed
//! prefix. The filesystem start is detected from where the root `DirMark` lands.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use risc_core::name_char_ok;

// Constants from `FileDir.Mod`.
const SECTOR_SIZE: usize = 1024;
const FN_LENGTH: usize = 32;
const SEC_TAB_SIZE: usize = 64;
const EX_TAB_SIZE: usize = 12;
const INDEX_SIZE: usize = SECTOR_SIZE / 4; // 256 disk addresses per index sector
const HEADER_SIZE: usize = 352;
const DIR_ROOT_ADR: u32 = 29; // sector 1
const DIR_PG_SIZE: usize = 24;
const DIR_MARK: u32 = 0x9B1E_A38D;
const HEADER_MARK: u32 = 0x9BA7_1D86;

// Byte offset of the filesystem in a full SD-card image: 0x80002 blocks of 512
// bytes = 256 MiB + 1 KiB — the same base the emulator's `disk.rs` rebases by. A
// raw `.dsk` instead starts at 0; `from_bytes` probes both.
const SD_FS_OFFSET: usize = 0x1000_0400;

// Field byte offsets within a file header sector. (The name lives at offset 4,
// but the reader takes names from the directory, so it isn't needed here.)
const OFF_ALENG: usize = 36;
const OFF_BLENG: usize = 40;
const OFF_EXT: usize = 48; // ext[12]
const OFF_SEC: usize = 96; // sec[64]

// Field byte offsets within a directory page.
const OFF_DIR_M: usize = 4;
const OFF_DIR_P0: usize = 8;
const OFF_DIR_E: usize = 64; // e[24]
const DIR_ENTRY_SIZE: usize = FN_LENGTH + 4 + 4; // name + adr + p = 40

/// A file found in the directory: its name and the disk address of its header.
pub struct Entry {
    pub name: String,
    pub header: u32,
}

/// An Oberon filesystem image held in memory, read only.
pub struct Image {
    data: Vec<u8>,
    /// Byte offset of the filesystem within `data`: 0 for a raw `.dsk`, or
    /// `SD_FS_OFFSET` for a full SD-card image.
    base: usize,
}

impl Image {
    /// Load and validate a `.dsk` image.
    ///
    /// # Errors
    /// Fails if the file can't be read or doesn't begin with a directory root
    /// page (i.e. isn't a filesystem image).
    pub fn open(path: &Path) -> io::Result<Image> {
        Self::from_bytes(fs::read(path)?)
    }

    fn from_bytes(data: Vec<u8>) -> io::Result<Image> {
        // The directory root (DiskAdr 29 = sector 1) sits at the very start of the
        // filesystem region. Locate that region by where its DirMark lands: byte 0
        // for a raw `.dsk`, or behind the prefix for a full SD-card image.
        let base = [0, SD_FS_OFFSET]
            .into_iter()
            .find(|&b| has_dir_mark(&data, b))
            .ok_or_else(|| {
                bad("not an Oberon filesystem image (no directory mark at sector 1)")
            })?;
        Ok(Image { data, base })
    }

    /// Every file in the directory, in name order.
    ///
    /// # Errors
    /// Fails if a directory page is malformed (bad mark or out of range).
    pub fn entries(&self) -> io::Result<Vec<Entry>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        self.walk(DIR_ROOT_ADR, &mut out, &mut seen)?;
        Ok(out)
    }

    /// Reconstruct the full byte contents of the file whose header is at `header`.
    ///
    /// # Errors
    /// Fails if the header is malformed or a data sector falls outside the image.
    pub fn read_file(&self, header: u32) -> io::Result<Vec<u8>> {
        let (aleng, bleng, sec, ext) = self.header(header)?;
        // Don't pre-size from the header: a corrupt `aleng` could claim a huge
        // length, and the loop below bails on the first out-of-range sector anyway.
        let mut data = Vec::new();
        for page in 0..=aleng {
            let buf = self.sector(self.page_sector(page, &sec, &ext)?)?;
            // Page 0 is the header sector, so its data starts past the header; the
            // last page is only filled to `bleng`.
            let start = if page == 0 { HEADER_SIZE } else { 0 };
            let end = if page == aleng { bleng } else { SECTOR_SIZE };
            if start > end {
                return Err(bad("file header length is inconsistent"));
            }
            data.extend_from_slice(&buf[start..end]);
        }
        Ok(data)
    }

    /// Parse a header sector into `(aleng, bleng, sec, ext)`, validating it.
    fn header(&self, header: u32) -> io::Result<(usize, usize, Vec<u32>, Vec<u32>)> {
        let h = self.sector(header)?;
        if rd_u32(&h, 0) != HEADER_MARK {
            return Err(bad("file header has the wrong mark"));
        }
        let aleng = rd_i32(&h, OFF_ALENG);
        let bleng = rd_i32(&h, OFF_BLENG);
        if aleng < 0 || bleng < 0 || bleng as usize > SECTOR_SIZE {
            return Err(bad("file header has an invalid length"));
        }
        let sec = read_adr_table(&h, OFF_SEC, SEC_TAB_SIZE);
        let ext = read_adr_table(&h, OFF_EXT, EX_TAB_SIZE);
        Ok((aleng as usize, bleng as usize, sec, ext))
    }

    /// Map a 0-based page index to its data sector's disk address.
    fn page_sector(&self, page: usize, sec: &[u32], ext: &[u32]) -> io::Result<u32> {
        if page < SEC_TAB_SIZE {
            return Ok(sec[page]);
        }
        let i = (page - SEC_TAB_SIZE) / INDEX_SIZE;
        let j = (page - SEC_TAB_SIZE) % INDEX_SIZE;
        if i >= EX_TAB_SIZE {
            return Err(bad("file is too large (extension table overflow)"));
        }
        Ok(rd_u32(&self.sector(ext[i])?, j * 4))
    }

    /// Read the 1024-byte sector at disk address `adr` (returns a copy).
    fn sector(&self, adr: u32) -> io::Result<[u8; SECTOR_SIZE]> {
        let s = (adr / 29) as usize;
        if s == 0 {
            return Err(bad("invalid disk address 0"));
        }
        let off = self.base + (s - 1) * SECTOR_SIZE;
        let slice = self
            .data
            .get(off..off + SECTOR_SIZE)
            .ok_or_else(|| bad("disk address points past the end of the image"))?;
        let mut buf = [0u8; SECTOR_SIZE];
        buf.copy_from_slice(slice);
        Ok(buf)
    }

    /// In-order B-tree walk (mirrors `VFileDir.enumerate`) collecting every entry.
    /// `seen` guards against a cyclic/corrupt directory.
    fn walk(&self, adr: u32, out: &mut Vec<Entry>, seen: &mut HashSet<u32>) -> io::Result<()> {
        if adr == 0 || !seen.insert(adr / 29) {
            return Ok(());
        }
        let page = self.sector(adr)?;
        if rd_u32(&page, 0) != DIR_MARK {
            return Err(bad("directory page has the wrong mark"));
        }
        let m = (rd_i32(&page, OFF_DIR_M).max(0) as usize).min(DIR_PG_SIZE);
        self.walk(rd_u32(&page, OFF_DIR_P0), out, seen)?;
        for i in 0..m {
            let base = OFF_DIR_E + i * DIR_ENTRY_SIZE;
            if let Some(name) = read_name(&page[base..base + FN_LENGTH]) {
                out.push(Entry {
                    name,
                    header: rd_u32(&page, base + FN_LENGTH),
                });
            }
            self.walk(rd_u32(&page, base + FN_LENGTH + 4), out, seen)?;
        }
        Ok(())
    }
}

fn read_adr_table(buf: &[u8], off: usize, n: usize) -> Vec<u32> {
    (0..n).map(|k| rd_u32(buf, off + k * 4)).collect()
}

fn rd_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn rd_i32(buf: &[u8], off: usize) -> i32 {
    rd_u32(buf, off) as i32
}

/// Whether the four bytes at `off` are the directory `DirMark` (bounds-checked,
/// so an out-of-range `off` is simply `false` rather than a panic).
fn has_dir_mark(data: &[u8], off: usize) -> bool {
    matches!(
        data.get(off..off + 4),
        Some(b) if u32::from_le_bytes([b[0], b[1], b[2], b[3]]) == DIR_MARK
    )
}

/// Decode a 32-byte name field, enforcing the Oberon character set (a letter,
/// then letters/digits/`.`). Returns `None` for an empty or malformed name —
/// which also stops a hostile image from yielding a surprising host path.
fn read_name(field: &[u8]) -> Option<String> {
    let mut s = String::new();
    for (i, &ch) in field.iter().enumerate() {
        if ch == 0 {
            break;
        }
        if !name_char_ok(i, ch) {
            return None;
        }
        s.push(char::from(ch));
    }
    (!s.is_empty()).then_some(s)
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The header's name field; only needed to build realistic test headers.
    const OFF_NAME: usize = 4;
    const NSEC: usize = 16;

    fn blank() -> Vec<u8> {
        vec![0u8; NSEC * SECTOR_SIZE]
    }

    fn at(sector: u32, off: usize) -> usize {
        (sector as usize - 1) * SECTOR_SIZE + off
    }
    fn put_u32(img: &mut [u8], sector: u32, off: usize, val: u32) {
        img[at(sector, off)..at(sector, off) + 4].copy_from_slice(&val.to_le_bytes());
    }
    fn put_bytes(img: &mut [u8], sector: u32, off: usize, bytes: &[u8]) {
        img[at(sector, off)..at(sector, off) + bytes.len()].copy_from_slice(bytes);
    }
    fn put_name(img: &mut [u8], sector: u32, off: usize, name: &str) {
        put_bytes(img, sector, off, name.as_bytes()); // rest stays NUL
    }
    fn adr(sector: u32) -> u32 {
        sector * 29
    }

    /// Write a directory entry `i` into the page at `sector`.
    fn put_entry(img: &mut [u8], sector: u32, i: usize, name: &str, header: u32, child: u32) {
        let base = OFF_DIR_E + i * DIR_ENTRY_SIZE;
        put_name(img, sector, base, name);
        put_u32(img, sector, base + FN_LENGTH, header);
        put_u32(img, sector, base + FN_LENGTH + 4, child);
    }

    /// Write a leaf directory page at `sector` from `(name, header)` entries.
    fn put_dir_page(img: &mut [u8], sector: u32, p0: u32, entries: &[(&str, u32)]) {
        put_u32(img, sector, 0, DIR_MARK);
        put_u32(img, sector, OFF_DIR_M, entries.len() as u32);
        put_u32(img, sector, OFF_DIR_P0, p0);
        for (i, (name, header)) in entries.iter().enumerate() {
            put_entry(img, sector, i, name, *header, 0);
        }
    }

    /// Write a single-sector file header at `sector`, with `content` inline.
    fn put_small_file(img: &mut [u8], sector: u32, name: &str, content: &[u8]) {
        assert!(content.len() <= SECTOR_SIZE - HEADER_SIZE);
        put_u32(img, sector, 0, HEADER_MARK);
        put_name(img, sector, OFF_NAME, name);
        put_u32(img, sector, OFF_ALENG, 0);
        put_u32(img, sector, OFF_BLENG, (HEADER_SIZE + content.len()) as u32);
        put_u32(img, sector, OFF_SEC, adr(sector)); // sec[0] = this sector
        put_bytes(img, sector, HEADER_SIZE, content);
    }

    #[test]
    fn reads_a_single_small_file() {
        let mut img = blank();
        put_dir_page(&mut img, 1, 0, &[("Hello.Mod", adr(2))]);
        put_small_file(&mut img, 2, "Hello.Mod", b"Hello, Oberon!");

        let image = Image::from_bytes(img).unwrap();
        let entries = image.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Hello.Mod");
        assert_eq!(
            image.read_file(entries[0].header).unwrap(),
            b"Hello, Oberon!"
        );
    }

    #[test]
    fn reconstructs_a_multi_sector_file() {
        let mut img = blank();
        // 1000 content bytes: 672 inline in the header sector, 328 in sector 3.
        let content: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        put_dir_page(&mut img, 1, 0, &[("Big.Mod", adr(2))]);
        put_u32(&mut img, 2, 0, HEADER_MARK);
        put_name(&mut img, 2, OFF_NAME, "Big.Mod");
        put_u32(&mut img, 2, OFF_ALENG, 1);
        put_u32(
            &mut img,
            2,
            OFF_BLENG,
            (1000 + HEADER_SIZE - SECTOR_SIZE) as u32,
        ); // 328
        put_u32(&mut img, 2, OFF_SEC, adr(2)); // sec[0]
        put_u32(&mut img, 2, OFF_SEC + 4, adr(3)); // sec[1]
        put_bytes(&mut img, 2, HEADER_SIZE, &content[..672]);
        put_bytes(&mut img, 3, 0, &content[672..]);

        let image = Image::from_bytes(img).unwrap();
        let e = &image.entries().unwrap()[0];
        assert_eq!(e.name, "Big.Mod");
        assert_eq!(image.read_file(e.header).unwrap(), content);
    }

    #[test]
    fn walks_the_btree_in_name_order() {
        let mut img = blank();
        // Root holds "M", with p0 -> leaf {"A"} and e[0].p -> leaf {"Z"}.
        put_u32(&mut img, 1, 0, DIR_MARK);
        put_u32(&mut img, 1, OFF_DIR_M, 1);
        put_u32(&mut img, 1, OFF_DIR_P0, adr(4));
        put_entry(&mut img, 1, 0, "M", adr(3), adr(5));
        put_dir_page(&mut img, 4, 0, &[("A", adr(6))]);
        put_dir_page(&mut img, 5, 0, &[("Z", adr(7))]);
        put_small_file(&mut img, 3, "M", b"m");
        put_small_file(&mut img, 6, "A", b"a");
        put_small_file(&mut img, 7, "Z", b"z");

        let image = Image::from_bytes(img).unwrap();
        let names: Vec<_> = image
            .entries()
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["A", "M", "Z"]);
    }

    #[test]
    fn rejects_a_non_filesystem_image() {
        let err = Image::from_bytes(vec![0u8; SECTOR_SIZE]).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn has_dir_mark_finds_the_mark_and_tolerates_out_of_range() {
        let mut d = vec![0u8; 16];
        d[4..8].copy_from_slice(&DIR_MARK.to_le_bytes());
        assert!(has_dir_mark(&d, 4));
        assert!(!has_dir_mark(&d, 0)); // zeros
        assert!(!has_dir_mark(&d, 14)); // past the end -> false, no panic
    }

    #[test]
    fn read_name_enforces_the_oberon_charset() {
        assert_eq!(read_name(b"Kernel.Mod\0\0").as_deref(), Some("Kernel.Mod"));
        assert_eq!(read_name(b"\0").as_deref(), None); // empty
        assert_eq!(read_name(b"9bad\0").as_deref(), None); // must start with a letter
        assert_eq!(read_name(b"a/b\0").as_deref(), None); // no path separators
    }
}
