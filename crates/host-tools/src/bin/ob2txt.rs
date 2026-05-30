//! `ob2txt` — convert an Oberon source/text file to readable host text.
//!
//! Oberon stores text as Latin-1 with CR (`0x0D`) line separators; on the host we
//! want UTF-8 with LF. `ob2txt A.Mod` writes the converted content to `A.Mod.txt`,
//! leaving the original untouched (its inverse is `txt2ob`).
//!
//! This is a convenience for reading and editing extracted sources, not a
//! byte-exact pipeline step (that is `extract-source`): the CR->LF and
//! Latin-1->UTF-8 conversion is faithful for the plain Oberon text we deal with,
//! but it does not interpret a formatted (`0F1X`) header from the Oberon editor —
//! none appear in the images we build from.

use std::fs;
use std::path::PathBuf;
use std::process::exit;

use clap::Parser;

/// Convert an Oberon source/text file to readable host text (`<FILE>.txt`).
#[derive(Parser, Debug)]
#[command(name = "ob2txt", version)]
struct Cli {
    /// Oberon file to convert (e.g. `A.Mod`); the result is written to `<FILE>.txt`
    #[arg(value_name = "FILE")]
    file: PathBuf,
}

/// Oberon bytes (Latin-1, CR separators) -> host text (UTF-8, LF). Each byte maps
/// to its Latin-1 code point, then CR and CRLF become LF.
fn from_oberon(bytes: &[u8]) -> String {
    let text: String = bytes.iter().map(|&b| char::from(b)).collect();
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn main() {
    let cli = Cli::parse();
    let bytes = match fs::read(&cli.file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("ob2txt: can't read {}: {e}", cli.file.display());
            exit(1);
        }
    };
    let mut name = cli.file.clone().into_os_string();
    name.push(".txt");
    let out = PathBuf::from(name);
    if let Err(e) = fs::write(&out, from_oberon(&bytes)) {
        eprintln!("ob2txt: can't write {}: {e}", out.display());
        exit(1);
    }
    eprintln!("ob2txt: {} -> {}", cli.file.display(), out.display());
}

#[cfg(test)]
mod tests {
    use super::from_oberon;

    #[test]
    fn cr_becomes_lf() {
        assert_eq!(from_oberon(b"MODULE A;\rEND A.\r"), "MODULE A;\nEND A.\n");
    }

    #[test]
    fn crlf_collapses_to_lf() {
        assert_eq!(from_oberon(b"a\r\nb"), "a\nb");
    }

    #[test]
    fn latin1_byte_becomes_utf8() {
        assert_eq!(from_oberon(&[0xE4]), "ä"); // 0xE4 = 'ä' in Latin-1
    }
}
