//! The `risc` binary: parse args, build the core + devices, run the frontend.

#[cfg(feature = "frontend")]
fn main() {
    if let Err(e) = oberon_risc_emu::frontend::run() {
        eprintln!("risc: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(feature = "frontend"))]
fn main() {
    eprintln!("risc was built without the `frontend` feature; nothing to run");
    std::process::exit(1);
}
