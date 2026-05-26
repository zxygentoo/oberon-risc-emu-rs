//! Build script. For the `cosim` differential-testing feature it compiles the C
//! reference (`cosim/shim.c`, which `#include`s `risc.c`, plus `risc-fp.c` and
//! `disk.c`) and links it so the tests can compare the Rust port against the C
//! live over FFI. Does nothing for normal builds.
//!
//! The C reference path defaults to the sibling repo and is overridable via the
//! `OBERON_C_SRC` environment variable.

fn main() {
    #[cfg(feature = "cosim")]
    build_cosim();
}

#[cfg(feature = "cosim")]
fn build_cosim() {
    let c_src = std::env::var("OBERON_C_SRC")
        .unwrap_or_else(|_| "/home/zxy/Projects/oberon-risc-emu/src".to_string());

    println!("cargo:rerun-if-env-changed=OBERON_C_SRC");
    println!("cargo:rerun-if-changed=cosim/shim.c");
    for f in [
        "risc.c",
        "risc.h",
        "risc-fp.c",
        "risc-fp.h",
        "risc-io.h",
        "disk.c",
        "disk.h",
        "risc-boot.inc",
    ] {
        println!("cargo:rerun-if-changed={c_src}/{f}");
    }

    cc::Build::new()
        .file("cosim/shim.c")
        .file(format!("{c_src}/risc-fp.c"))
        .file(format!("{c_src}/disk.c"))
        .include(&c_src)
        .warnings(false)
        .compile("cosim_shim");
}
