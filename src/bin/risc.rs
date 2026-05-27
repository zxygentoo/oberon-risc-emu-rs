//! The `risc` binary: parse args, build the core + devices, run the frontend.

fn main() {
    if let Err(e) = oberon_risc_emu::run() {
        eprintln!("risc: {e}");
        std::process::exit(1);
    }
}
