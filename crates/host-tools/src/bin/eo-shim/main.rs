//! `eo-shim` — boot an `InnerCore` under the headless [`shim`] runtime and run a
//! single Oberon command. A thin wrapper over `host_tools::shim::run` for
//! bringing up the Extended Oberon toolchain core: the `InnerCore` image and any
//! modules the command loads (`.rsc`) are taken from `DIR`.
//!
//! Usage: `eo-shim <DIR> <Module.Proc> [param ...]` — e.g.
//! `eo-shim /tmp/eo-core ORP.Compile Foo.Mod/s`. Prints the guest exit code.

use std::path::PathBuf;
use std::process::exit;

use clap::Parser;

/// Boot an inner core under the shim and run one command.
#[derive(Parser, Debug)]
#[command(name = "eo-shim", version)]
struct Cli {
    /// Directory holding `InnerCore` and the command's `.rsc` modules.
    #[arg(value_name = "DIR")]
    dir: PathBuf,

    /// The Oberon command and its parameters (e.g. `ORP.Compile Foo.Mod/s`).
    #[arg(value_name = "ARG", required = true, num_args = 1..)]
    command: Vec<String>,
}

fn main() {
    let cli = Cli::parse();
    match host_tools::shim::run(&cli.command, &cli.dir, std::slice::from_ref(&cli.dir)) {
        Ok(code) => {
            eprintln!("eo-shim: guest exit code {code}");
            exit(code);
        }
        Err(e) => {
            eprintln!("eo-shim: {e}");
            exit(1);
        }
    }
}
