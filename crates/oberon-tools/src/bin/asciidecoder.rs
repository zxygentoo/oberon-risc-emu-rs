// Decodes 'AsciiCoder.DecodeFiles' archives

use std::env;
use std::fs;
use std::io::{stdin, BufRead, BufReader, ErrorKind, Read, Write};
use std::process::exit;

trait ByteIterator: Iterator<Item = u8> {}
impl<T: Iterator<Item = u8>> ByteIterator for T {}

fn skip_header<T: ByteIterator>(bytes: &mut T) -> bool {
    let command = b"AsciiCoder.DecodeFiles";
    let mut idx = 0;
    for ch in bytes.by_ref() {
        if ch != command[idx] {
            idx = 0;
        }
        if ch == command[idx] {
            idx += 1;
            if idx == command.len() {
                return true;
            }
        }
    }
    false
}

fn decode<T: ByteIterator>(bytes: &mut T) -> Option<Vec<u8>> {
    const BASE: u8 = 48;
    let mut vec = Vec::new();
    let mut bits = 0;
    let mut buf: u32 = 0;
    for ch in bytes.filter(|&a| a > 32) {
        if (BASE..BASE + 64).contains(&ch) {
            buf |= ((ch - BASE) as u32) << bits;
            bits += 6;
            if bits >= 8 {
                vec.push((buf & 0xFF) as u8);
                buf >>= 8;
                bits -= 8;
            }
        } else {
            return match ch {
                b'#' if bits == 0 => Some(vec),
                b'%' if bits == 2 => Some(vec),
                b'$' if bits == 4 => Some(vec),
                _ => None,
            };
        }
    }
    None
}

fn read_number<T: ByteIterator>(bytes: &mut T) -> Option<i32> {
    let mut n: i32 = 0;
    let mut bits = 0;
    for b in bytes.by_ref() {
        if b >= 0x80 {
            n |= ((b - 0x80) as i32) << bits;
            bits += 7;
            if bits >= 32 {
                return None;
            }
        } else {
            n |= (((b as i32) ^ 0x40) - 0x40) << bits;
            return Some(n);
        }
    }
    None
}

fn decompress<T: ByteIterator>(bytes: &mut T) -> Option<Vec<u8>> {
    const N: usize = 16384;

    let size = match read_number(bytes) {
        Some(n) if n >= 0 => n,
        _ => return None,
    };

    let mut table = [0u8; N];
    let mut vec = Vec::new();
    let mut hash: usize = 0;
    let mut buf: u32 = 0;
    let mut bits = 0;

    for _ in 0..size {
        if bits == 0 {
            buf = u32::from(bytes.next()?);
            bits = 8;
        }

        let misprediction = (buf & 1) != 0;
        buf >>= 1;
        bits -= 1;

        let data = if misprediction {
            let b = bytes.next()?;
            buf |= (b as u32) << bits;
            let d = (buf & 0xFF) as u8;
            buf >>= 8;
            table[hash] = d;
            d
        } else {
            table[hash]
        };
        vec.push(data);
        hash = (16 * hash + data as usize) % N;
    }
    Some(vec)
}

fn read_name<T: ByteIterator>(bytes: &mut T) -> Option<String> {
    let vec: Vec<u8> = bytes
        .skip_while(|&b| b <= 32)
        .take_while(|&b| b > 32)
        .collect();
    let name = String::from_utf8(vec).unwrap();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

macro_rules! printerr {
    ($prog_name:expr, $err:expr, $fmt:expr) => {
        eprintln!(concat!("{}: ", $fmt, ": {}"), $prog_name, $err)
    };
    ($prog_name:expr, $err:expr, $fmt:expr, $($arg:tt)*) => {
        eprintln!(concat!("{}: ", $fmt, ": {}"), $prog_name, $($arg)*, $err)
    };
}

macro_rules! printerrx {
    ($prog_name:expr, $fmt:expr) => {
        eprintln!(concat!("{}: ", $fmt), $prog_name)
    };
    ($prog_name:expr, $fmt:expr, $($arg:tt)*) => {
        eprintln!(concat!("{}: ", $fmt), $prog_name, $($arg)*)
    };
}

const HELP: &str = "\
asciidecoder - extract files from an Oberon AsciiCoder archive

Extracts the files from an 'AsciiCoder.DecodeFiles' archive - the plain-text
file encoding produced by Oberon's AsciiCoder - just as the Oberon command
AsciiCoder.DecodeFiles would. The archive is read from FILE, or from standard
input if no FILE is given.

Usage:
  asciidecoder [-v] [-C DIR] [FILE]

Flags:
  -v, --verbose        Print the name of each extracted file.
  -C, --directory DIR  Extract into DIR, creating it if it does not exist.
  -h, --help           Show this help and exit.";

fn main() {
    let mut verbose = false;
    let mut directory = None;
    let mut input_file = None;

    let mut args = env::args();
    let progname = args.next().unwrap().rsplit('/').next().unwrap().to_owned();
    while let Some(opt) = args.next() {
        match opt.as_str() {
            "-v" | "--verbose" => {
                verbose = true;
            }
            "-C" | "--directory" if directory.is_none() => {
                directory = args.next();
            }
            "-h" | "--help" => {
                println!("{HELP}");
                return;
            }
            s if !s.starts_with('-') && input_file.is_none() => {
                input_file = Some(opt.clone());
            }
            _ => {
                eprintln!("{progname}: unrecognized argument '{opt}'");
                eprintln!("Usage: {progname} [-v] [-C DIR] [FILE]  (try --help)");
                exit(1)
            }
        }
    }

    let input: Box<dyn BufRead> = match input_file {
        None => Box::new(BufReader::new(stdin())),
        Some(filename) => match fs::File::open(&filename) {
            Ok(f) => Box::new(BufReader::new(f)),
            Err(e) => {
                printerr!(progname, e, "can't open '{}'", filename);
                exit(1)
            }
        },
    };
    let mut input: Box<dyn Iterator<Item = u8>> = Box::new(input.bytes().map(Result::unwrap));

    if let Some(directory) = directory {
        if let Err(e) = env::set_current_dir(&directory) {
            if e.kind() != ErrorKind::NotFound {
                printerr!(progname, e, "can't change to directory '{}'", directory);
                exit(1);
            }
            if let Err(e) = fs::create_dir_all(&directory) {
                printerr!(progname, e, "can't create directory '{}'", directory);
                exit(1);
            }
            if let Err(e) = env::set_current_dir(&directory) {
                printerr!(progname, e, "can't change to directory '{}'", directory);
                exit(1);
            }
        }
    }

    let mut compressed = false;
    let mut names = Vec::new();
    if !skip_header(&mut input) {
        printerrx!(progname, "no AsciiCoder.DecodeFiles archive found");
        exit(1);
    }
    while let Some(name) = read_name(&mut input) {
        match name.as_str() {
            "~" => break,
            "%" => compressed = true,
            _ => names.push(name),
        }
    }

    for name in &names {
        if verbose {
            println!("{name}");
        }

        let Some(mut data) = decode(&mut input) else {
            printerrx!(progname, "can't decode '{}' (input file truncated?)", name);
            exit(1)
        };
        if compressed {
            data = if let Some(vec) = decompress(&mut data.into_iter()) {
                vec
            } else {
                printerrx!(progname, "can't decompress '{}'", name);
                exit(1)
            };
        }

        let mut file = match fs::File::create(name) {
            Ok(f) => f,
            Err(e) => {
                printerr!(progname, e, "can't create file '{}'", name);
                continue;
            }
        };

        if let Err(e) = file.write_all(&data) {
            printerr!(progname, e, "can't write file '{}'", name);
            fs::remove_file(name).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, decompress, read_name, read_number, skip_header};

    fn bytes(s: &[u8]) -> impl Iterator<Item = u8> + '_ {
        s.iter().copied()
    }

    #[test]
    fn decode_unpacks_six_bit_payload() {
        // "8U6%" is the AsciiCoder 6-bit packing of "Hi" ('%' = 2 leftover bits).
        assert_eq!(decode(&mut bytes(b"8U6%")), Some(b"Hi".to_vec()));
    }

    #[test]
    fn decode_rejects_wrong_terminator() {
        // '#' is only valid with 0 leftover bits, but 2 remain here.
        assert_eq!(decode(&mut bytes(b"8U6#")), None);
    }

    #[test]
    fn read_number_decodes_signed_varint() {
        assert_eq!(read_number(&mut bytes(&[0x00])), Some(0));
        assert_eq!(read_number(&mut bytes(&[0x01])), Some(1));
        assert_eq!(read_number(&mut bytes(&[0x7F])), Some(-1));
        assert_eq!(read_number(&mut bytes(&[0x80, 0x01])), Some(128));
    }

    #[test]
    fn read_name_splits_on_whitespace() {
        let mut it = bytes(b"  hi.txt  next ");
        assert_eq!(read_name(&mut it).as_deref(), Some("hi.txt"));
        assert_eq!(read_name(&mut it).as_deref(), Some("next"));
        assert_eq!(read_name(&mut it), None);
    }

    #[test]
    fn skip_header_finds_the_marker() {
        assert!(skip_header(&mut bytes(b"junk AsciiCoder.DecodeFiles rest")));
        assert!(!skip_header(&mut bytes(b"no marker present")));
    }

    #[test]
    fn decompress_handles_literal_and_predicted_bytes() {
        // size=1 (0x01); control 0xB1 has its LSB set (a misprediction), so the
        // next source byte 0x00 combines with the carried bits to yield 'X'.
        assert_eq!(
            decompress(&mut bytes(&[0x01, 0xB1, 0x00])),
            Some(vec![b'X'])
        );
        // size=1; control 0x00 has its LSB clear (a prediction); table[0] is 0.
        assert_eq!(decompress(&mut bytes(&[0x01, 0x00])), Some(vec![0]));
    }
}
