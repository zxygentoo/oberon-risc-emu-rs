//! Dump the plain-text content of an Oberon text.

use std::fs;
use std::io::{self, copy, sink, stdout, Read};
use std::path::{Path, PathBuf};
use std::process::exit;

use clap::Parser;

/// Dump the plain-text content of an Oberon text.
///
/// Reads an Oberon text (such as a .Text, .Mod or .Tool file) from FILE and
/// writes its content to standard output: it drops the binary file header and
/// converts CR line endings to LF. A file that is not an Oberon text is copied
/// through unchanged. The conversion is deliberately crude — it does not
/// interpret the formatting information an Oberon text can carry, so some
/// non-text bytes may pass through.
///
/// The input is a binary file, so it is taken as a FILE argument rather than
/// read from standard input (which could only be piped, never typed).
#[derive(Parser, Debug)]
#[command(name = "ob2unix", version)]
struct Cli {
    /// Oberon text to convert (e.g. a .Text, .Mod or .Tool file)
    #[arg(value_name = "FILE")]
    file: PathBuf,
}

fn ob2unix(path: &Path) -> io::Result<()> {
    let mut input = fs::File::open(path)
        .map_err(|e| io::Error::new(e.kind(), format!("can't open '{}': {e}", path.display())))?;
    let mut output = stdout();

    let mut buf = [0u8; 1024];
    let res = copy(&mut input.by_ref().take(6), &mut &mut buf[..])? as usize;
    let is_oberon = res == 6 && ((buf[0] == 240 && buf[1] == 1) || (buf[0] == 1 && buf[1] == 240));

    if is_oberon {
        // skip header
        let size =
            (buf[2] as u64) | (buf[3] as u64) << 8 | (buf[4] as u64) << 16 | (buf[5] as u64) << 24;
        copy(&mut input.by_ref().take(size - 6), &mut sink())?;

        // translate '\r' to '\n'
        loop {
            let res = copy(
                &mut input.by_ref().take(buf.len() as u64),
                &mut &mut buf[..],
            )? as usize;
            if res == 0 {
                break;
            }
            for ch in &mut buf[..res] {
                if *ch == b'\r' {
                    *ch = b'\n';
                }
            }
            copy(&mut &buf[..res], &mut output)?;
        }
    } else {
        // Not an Oberon text file: copy input to output
        copy(&mut &buf[..res], &mut output)?;
        copy(&mut input, &mut output)?;
    }

    Ok(())
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = ob2unix(&cli.file) {
        eprintln!("ob2unix: {e}");
        exit(1);
    }
}
