//! `txt2ob` — convert host text back to Oberon source/text format.
//!
//! The inverse of `ob2txt`: host UTF-8 with LF -> Oberon Latin-1 with CR (`0x0D`)
//! line separators. `txt2ob A.Mod.txt` writes the result to `A.Mod` — the input
//! must end in `.txt`, which is dropped to form the output name (so it won't fire
//! on the wrong file and overwrite something it shouldn't).
//!
//! Useful for authoring a file in Oberon's native form — e.g. a `System.Tool`, or
//! a source to push onto a system with `eo-driver`, where CR line endings matter
//! (LF renders as one merged line in the Oberon viewer). Code points beyond
//! Latin-1 are replaced with `?`.

use std::fs;
use std::path::PathBuf;
use std::process::exit;

use clap::Parser;

/// Convert host text (`<NAME>.txt`) back to Oberon format, written to `<NAME>`.
#[derive(Parser, Debug)]
#[command(name = "txt2ob", version)]
struct Cli {
    /// Host text file to convert; must end in `.txt` (the output drops it)
    #[arg(value_name = "FILE.txt")]
    file: PathBuf,
}

/// Host text (UTF-8, LF) -> Oberon bytes (Latin-1, CR separators). CRLF and LF
/// become CR; code points beyond Latin-1 (`> U+00FF`) are replaced with `?`.
fn to_oberon(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\n")
        .replace('\n', "\r")
        .chars()
        .map(|c| if c as u32 <= 0xFF { c as u8 } else { b'?' })
        .collect()
}

fn main() {
    let cli = Cli::parse();
    let Some(out) = cli.file.to_str().and_then(|s| s.strip_suffix(".txt")) else {
        eprintln!("txt2ob: expected a `.txt` file, got {}", cli.file.display());
        exit(1);
    };
    let out = PathBuf::from(out);
    let text = match fs::read_to_string(&cli.file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("txt2ob: can't read {}: {e}", cli.file.display());
            exit(1);
        }
    };
    if let Err(e) = fs::write(&out, to_oberon(&text)) {
        eprintln!("txt2ob: can't write {}: {e}", out.display());
        exit(1);
    }
    eprintln!("txt2ob: {} -> {}", cli.file.display(), out.display());
}

#[cfg(test)]
mod tests {
    use super::to_oberon;

    #[test]
    fn lf_becomes_cr() {
        assert_eq!(
            to_oberon("MODULE A;\nEND A.\n"),
            b"MODULE A;\rEND A.\r".to_vec()
        );
    }

    #[test]
    fn crlf_normalizes_to_cr() {
        assert_eq!(to_oberon("a\r\nb"), b"a\rb".to_vec());
    }

    #[test]
    fn latin1_char_becomes_one_byte() {
        assert_eq!(to_oberon("ä"), vec![0xE4]); // 'ä' -> 0xE4
    }

    #[test]
    fn beyond_latin1_is_replaced() {
        assert_eq!(to_oberon("a→b"), b"a?b".to_vec()); // U+2192 -> '?'
    }
}
