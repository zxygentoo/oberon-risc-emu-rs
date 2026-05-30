//! `eo-inner-run` — boot an `InnerCore` under the headless [`shim`] runtime and run
//! a single Oberon command. A thin wrapper over `host_tools::shim::run`: the
//! `InnerCore` image and any modules the command loads (`.rsc`) are taken from `DIR`.
//!
//! A host-side developer tool for hacking on the Extended Oberon bootstrap and the
//! host toolchain — run a command against a toolchain inner core to regenerate the
//! EO seed, compile a module, or trace a boot with `OBERON_TRACE`. (The on-EO
//! coding agent runs *inside* Oberon via EO's own interfaces; it does not use this.)
//!
//! Usage: `eo-inner-run <DIR> <Module.Proc> [param ...]` — e.g.
//! `eo-inner-run /tmp/eo-core ORP.Compile Foo.Mod/s`. Prints the guest exit code.

use std::path::PathBuf;
use std::process::exit;

use clap::Parser;

/// Boot an inner core under the shim and run one command.
#[derive(Parser, Debug)]
#[command(name = "eo-inner-run", version)]
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
            eprintln!("eo-inner-run: guest exit code {code}");
            exit(code);
        }
        Err(e) => {
            eprintln!("eo-inner-run: {e}");
            exit(1);
        }
    }
}
