// Dumps the ASCII content of Oberon texts. Doesn't properly
// parse the files so doesn't work very well.

use std::env;
use std::io::{self, copy, sink, stdin, stdout, Read};
use std::process::exit;

fn ob2unix() -> io::Result<()> {
    let mut input = stdin();
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

const HELP: &str = "\
ob2unix - dump the plain-text content of an Oberon text

Reads an Oberon text (such as a .Text, .Mod or .Tool file) on standard input
and writes its content to standard output:

  - drops the binary file header
  - converts CR line endings to LF

Input that is not an Oberon text is copied through unchanged. The conversion is
deliberately crude: it does not interpret the formatting information an Oberon
text can carry, so some non-text bytes may pass through.

Usage:
  ob2unix < INPUT > OUTPUT";

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| matches!(a.as_str(), "-h" | "--help")) {
        println!("{HELP}");
        return;
    }
    if let Some(arg) = args.first() {
        eprintln!("ob2unix: unexpected argument '{arg}'");
        eprintln!("Usage: ob2unix < INPUT > OUTPUT  (try --help)");
        exit(1);
    }
    if let Err(e) = ob2unix() {
        eprintln!("ob2unix: {e}");
        exit(1);
    }
}
