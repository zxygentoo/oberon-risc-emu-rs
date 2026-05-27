//! Extract the files from an Oberon `AsciiCoder` archive (`AsciiCoder.DecodeFiles`).

use std::fs;
use std::io::{stdin, BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::exit;

use clap::Parser;

/// Shorthand for a byte-yielding iterator — the source the decoders pull from.
trait ByteIterator: Iterator<Item = u8> {}
impl<T: Iterator<Item = u8>> ByteIterator for T {}

/// Advance `bytes` past the literal `AsciiCoder.DecodeFiles` marker that opens an
/// archive; return whether it was found (consuming up to and including it).
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

/// Decode one 6-bit-packed block: printable chars `'0'`..`'o'` each carry 6 bits
/// (accumulated LSB-first into bytes), ended by a `#`/`%`/`$` terminator whose
/// identity encodes the leftover-bit count (0/2/4). `None` on a wrong terminator.
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

/// Read a sign-extended little-endian base-128 varint (7 bits per byte, high bit
/// set means "more bytes follow"). `None` if it would exceed 32 bits or input ends.
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

/// Inflate a compressed payload (as carried by `%`-flagged archives): a varint
/// output size, then a predictive byte stream — one "misprediction" bit per byte
/// selects between a hash-indexed prediction table and a following literal byte.
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

/// Read the next whitespace-delimited token (skipping leading whitespace), or
/// `None` at end of input. Used to read the archive's file-name list.
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

/// Extract the files from an Oberon `AsciiCoder` archive.
///
/// Decodes an `AsciiCoder.DecodeFiles` archive — the plain-text file encoding
/// produced by Oberon's `AsciiCoder` — just as the Oberon command
/// `AsciiCoder.DecodeFiles` would. The archive is read from FILE, or from standard
/// input if none is given; each contained file is written into the output
/// directory (the current directory by default).
#[derive(Parser, Debug)]
#[command(name = "asciidecoder", version)]
struct Cli {
    /// Print the name of each extracted file
    #[arg(short, long)]
    verbose: bool,

    /// Extract into DIR, creating it if it does not exist
    #[arg(short = 'C', long = "directory", value_name = "DIR")]
    directory: Option<PathBuf>,

    /// Archive to read (defaults to standard input)
    #[arg(value_name = "FILE")]
    file: Option<PathBuf>,
}

fn main() {
    if let Err(e) = run(&Cli::parse()) {
        eprintln!("asciidecoder: {e}");
        exit(1);
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    let reader: Box<dyn BufRead> = match &cli.file {
        None => Box::new(BufReader::new(stdin())),
        Some(path) => Box::new(BufReader::new(
            fs::File::open(path).map_err(|e| format!("can't open '{}': {e}", path.display()))?,
        )),
    };
    let mut input: Box<dyn Iterator<Item = u8>> = Box::new(reader.bytes().map(Result::unwrap));

    if let Some(dir) = &cli.directory {
        fs::create_dir_all(dir)
            .map_err(|e| format!("can't create directory '{}': {e}", dir.display()))?;
    }

    if !skip_header(&mut input) {
        return Err("no AsciiCoder.DecodeFiles archive found".to_string());
    }

    let mut compressed = false;
    let mut names = Vec::new();
    while let Some(name) = read_name(&mut input) {
        match name.as_str() {
            "~" => break,
            "%" => compressed = true,
            _ => names.push(name),
        }
    }

    for name in &names {
        if cli.verbose {
            println!("{name}");
        }
        let Some(mut data) = decode(&mut input) else {
            return Err(format!("can't decode '{name}' (input file truncated?)"));
        };
        if compressed {
            data = decompress(&mut data.into_iter())
                .ok_or_else(|| format!("can't decompress '{name}'"))?;
        }
        let path = match &cli.directory {
            Some(dir) => dir.join(name),
            None => PathBuf::from(name),
        };
        fs::write(&path, &data).map_err(|e| format!("can't write '{}': {e}", path.display()))?;
    }
    Ok(())
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
