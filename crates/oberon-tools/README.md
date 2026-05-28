# oberon-tools

Host-side command-line tools for working with Project Oberon: two pure-`std`
file converters and a Project Oberon disk-image builder. The binaries are
auto-discovered from [`src/bin/`](src/bin); all three use
[`clap`](https://crates.io/crates/clap) for a proper `--help`.

| Binary         | What it does                                                            | Deps        |
| -------------- | ----------------------------------------------------------------------- | ----------- |
| `ob2unix`      | dump the plain-text content of an Oberon text                           | pure `std`  |
| `asciidecoder` | unpack an `AsciiCoder.DecodeFiles` archive                              | pure `std`  |
| `build-image`  | compile Project Oberon from sources and assemble a runnable disk image  | `risc-core` |

## ob2unix

Drops the binary Oberon-text header and converts CR line endings to LF (a file
that isn't an Oberon text passes through unchanged). Takes the file to convert as
an argument and writes to stdout:

```sh
cargo run -p oberon-tools --bin ob2unix -- Input.Mod > input.txt
```

## asciidecoder

Extracts the member files from an `AsciiCoder.DecodeFiles` archive. `-v` lists
each extracted name; `-C DIR` sets the output directory:

```sh
cargo run -p oberon-tools --bin asciidecoder -- -v -C outdir archive.txt
```

## build-image

Compiles Project Oberon from a source tree and assembles a bootable `.dsk`. It
runs a **headless Oberon on the [`risc-core`](../risc-core) CPU** — a Rust port
of [`project-norebo`](https://github.com/pdewacht/project-norebo) — so it needs
no FPGA, no external Oberon, and no C toolchain. The Norebo host modules and the
bootstrap objects that seed the first compile are embedded in the binary (see
[`assets/`](assets)); only the Wirth sources proper are fetched separately.

```sh
# 1. fetch a PO2013 source tree (e.g. with project-norebo's fetch-sources.py)
# 2. build the image (release — compiling Oberon is CPU-heavy):
cargo run -p oberon-tools --release --bin build-image -- path/to/sources out.dsk
```

Internally the headless runtime is one function — [`shim::run`](src/bin/build-image/shim.rs) —
which executes one Oberon command (e.g. `ORP.Compile Foo.Mod/s`) to completion against
the host filesystem and returns its guest exit code; `build-image` drives it
repeatedly to compile the system and lay down the disk.

## Tests

```sh
cargo test -p oberon-tools
```

Unit tests cover the converters' pure functions and `build-image`'s filesystem
helpers; [`tests/cli.rs`](tests/cli.rs) exercises each binary end-to-end (argument
handling, exit codes, round-trips). One integration test,
`build_image_reproduces_the_boot_golden`, builds a disk image from a source tree
and boots it to the same C-derived golden hash as the core's boot test; it's
gated on `OBERON_SOURCES` (a fetched PO2013 tree, e.g. project-norebo's
`upstream/`):

```sh
OBERON_SOURCES=/path/to/po2013/sources cargo test -p oberon-tools
```

## License

[ISC](../../LICENSE), uniform with the rest of the workspace. The toolchain
assets under [`assets/`](assets) are vendored from project-norebo and derive from
Project Oberon 2013; they keep their upstream copyright (also ISC) — see
[`assets/README.md`](assets/README.md). The [workspace README](../../README.md)
covers the whole stack.
