# host-tools

Host-side command-line tools for working with Project Oberon: three pure-`std`
file/image utilities and a Project Oberon disk-image builder. The binaries are
auto-discovered from [`src/bin/`](src/bin); all four use
[`clap`](https://crates.io/crates/clap) for a proper `--help`.

| Binary           | What it does                                                            | Deps        |
| ---------------- | ----------------------------------------------------------------------- | ----------- |
| `ob2unix`        | dump the plain-text content of an Oberon text                           | pure `std`  |
| `asciidecoder`   | unpack an `AsciiCoder.DecodeFiles` archive                              | pure `std`  |
| `build-image`    | compile Project Oberon from sources and assemble a runnable disk image  | `risc-core` |
| `extract-source` | extract a build-ready source tree from a disk image (drops .rsc/.smb)   | pure `std`  |

## ob2unix

Drops the binary Oberon-text header and converts CR line endings to LF (a file
that isn't an Oberon text passes through unchanged). Takes the file to convert as
an argument and writes to stdout:

```sh
cargo run -p host-tools --bin ob2unix -- Input.Mod > input.txt
```

## asciidecoder

Extracts the member files from an `AsciiCoder.DecodeFiles` archive. `-v` lists
each extracted name; `-C DIR` sets the output directory:

```sh
cargo run -p host-tools --bin asciidecoder -- -v -C outdir archive.txt
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
cargo run -p host-tools --release --bin build-image -- path/to/sources out.dsk
```

Which files get compiled is governed by a **`.packonly`** manifest in the source
tree (required): every file is compiled as Oberon source and packed into the
image *except* those listed, which are packed verbatim — data (fonts, tools) and
reference modules that ship as source but aren't meant to compile. An empty
manifest compiles everything. `extract-source` generates it; for a
`fetch-sources.py` tree, derive it from the manifest's non-`source` rows.

**Mixing in custom modules** then needs no special flags: drop your `.Mod` files
into the source tree, leave them off `.packonly`, and they are compiled (in
dependency order, worked out from their `IMPORT` lists) and packed as loadable
objects like any other module. List any custom *data* you add in `.packonly`;
anything left off it that isn't valid Oberon source fails the build with a clear
error rather than being fed to the compiler.

Internally the headless runtime is one function — [`shim::run`](src/bin/build-image/shim.rs) —
which executes one Oberon command (e.g. `ORP.Compile Foo.Mod/s`) to completion against
the host filesystem and returns its guest exit code; `build-image` drives it
repeatedly to compile the system and lay down the disk.

## extract-source

Extracts the source files from a Project Oberon `.dsk` image into a host
directory — the inverse of `build-image`. It drops the compiled artifacts
(`.rsc`/`.smb`, which `build-image` regenerates from source), so the output is a
build-ready tree. It also writes the `.packonly` manifest `build-image` requires,
derived from which `.Mod` files carry a compiled object on the image. It reads the
Oberon on-disk filesystem directly (see
[`dsk.rs`](src/bin/extract-source/dsk.rs)), so it needs no emulator and no boot:

```sh
cargo run -p host-tools --bin extract-source -- Oberon.dsk out/
cargo run -p host-tools --release --bin build-image -- out rebuilt.dsk   # round-trips
```

Files come out byte-for-byte. Oberon sources (`*.Mod`, `*.Tool`, `*.Text`) are
"Oberon Text", so pipe them through `ob2unix` to read them as plain text.

## Tests

```sh
cargo test -p host-tools
```

Unit tests cover the converters' pure functions, the `.packonly` and import-order
logic, and `build-image`'s filesystem helpers; [`tests/cli.rs`](tests/cli.rs)
exercises each binary end-to-end (argument handling, exit codes, the build's
fail-clear paths). One heavy, self-contained integration test,
`build_image_round_trips_the_golden`, extracts the committed golden image and
rebuilds it, checking the result boots to the same C-derived golden hashes as the
core's boot test. It compiles all of Oberon through the shim, so it's `#[ignore]`d
by default:

```sh
cargo test -p host-tools --release -- --ignored
```

## License

[ISC](../../LICENSE), uniform with the rest of the workspace. The toolchain
assets under [`assets/`](assets) are vendored from project-norebo and derive from
Project Oberon 2013; they keep their upstream copyright (also ISC) — see
[`assets/README.md`](assets/README.md). The [workspace README](../../README.md)
covers the whole stack.
