# host-tools

Host-side command-line tools for working with Oberon: pure-`std` file/image
converters, a source extractor, and bootable disk-image builders for both Project
Oberon 2013 and Extended Oberon. The binaries are auto-discovered from
[`src/bin/`](src/bin); all use [`clap`](https://crates.io/crates/clap) for a
proper `--help`.

| Binary           | What it does                                                              | Deps        |
| ---------------- | ------------------------------------------------------------------------- | ----------- |
| `ob2unix`        | dump the plain-text content of an Oberon text                             | pure `std`  |
| `asciidecoder`   | unpack an `AsciiCoder.DecodeFiles` archive                                | pure `std`  |
| `extract-source` | extract a build-ready source tree from a disk image (drops .rsc/.smb)     | pure `std`  |
| `build-po-image` | compile Project Oberon 2013 from sources and assemble a bootable disk     | `risc-core` |
| `build-eo-image` | the Extended Oberon counterpart of `build-po-image`                       | `risc-core` |
| `eo-driver`      | boot an Oberon image headless and drive it (pointer, clicks, PCLink)      | `risc-core` |
| `eo-shim`        | boot an `InnerCore` under the shim and run one Oberon command             | `risc-core` |

The two image builders share their whole pipeline — compile a source tree against
an embedded toolchain, link a fresh inner core, lay down a bootable `Oberon.dsk` —
in [`host_tools::image`](src/image.rs); each binary differs only in its embedded
seed and CLI. Compile-order resolution is shared in [`resolve`](src/resolve.rs),
and the headless runtime in [`shim`](src/shim.rs).

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

## build-po-image

Compiles Project Oberon 2013 from a source tree and assembles a bootable `.dsk`.
It runs a **headless Oberon on the [`risc-core`](../risc-core) CPU** — a Rust port
of [`project-norebo`](https://github.com/pdewacht/project-norebo) — so it needs no
FPGA, no external Oberon, and no C toolchain. The Norebo host modules and the
bootstrap objects that seed the first compile are embedded in the binary (see
[`assets/`](assets)); only the Wirth sources proper are fetched separately.

```sh
# 1. fetch a PO2013 source tree (e.g. with project-norebo's fetch-sources.py)
# 2. build the image (release — compiling Oberon is CPU-heavy):
cargo run -p host-tools --release --bin build-po-image -- path/to/sources out.dsk
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

Internally the headless runtime is one function — [`shim::run`](src/shim.rs) —
which executes one Oberon command (e.g. `ORP.Compile Foo.Mod/s`) to completion
against the host filesystem and returns its guest exit code; the shared
[`image::build`](src/image.rs) pipeline drives it repeatedly to compile the system
and lay down the disk.

## build-eo-image

The Extended Oberon (EO) peer of `build-po-image`: from an EO source tree (e.g.
`extract-source` on an EO `RISC.img`) it compiles the whole EO system through the
shim and assembles a bootable `Oberon.dsk` that boots to the EO desktop.

```sh
cargo run -p host-tools --release --bin build-eo-image -- path/to/eo-sources out.dsk
```

It is *structurally identical* to `build-po-image` — same `image::build` pipeline,
same `.packonly` rules — and differs only in its embedded seed: the EO-specific
host glue and a `Modules`-topped bootstrap inner core. The full story (why
`Modules` is the inner-core top, the `.rsx` offline-link convention, the EO
`CoreLinker` port) is in [`BUILD-EO-IMAGE.md`](BUILD-EO-IMAGE.md).

## extract-source

Extracts the source files from an Oberon `.dsk`/`RISC.img` image into a host
directory — the inverse of the image builders. It drops the compiled artifacts
(`.rsc`/`.smb`, which the builders regenerate from source), so the output is a
build-ready tree. It also writes the `.packonly` manifest the builders require,
derived from which `.Mod` files carry a compiled object on the image. It reads the
Oberon on-disk filesystem directly (see [`dsk.rs`](src/bin/extract-source/dsk.rs)),
so it needs no emulator and no boot:

```sh
cargo run -p host-tools --bin extract-source -- Oberon.dsk out/
cargo run -p host-tools --release --bin build-po-image -- out rebuilt.dsk   # round-trips
```

Files come out byte-for-byte. Oberon sources (`*.Mod`, `*.Tool`, `*.Text`) are
"Oberon Text", so pipe them through `ob2unix` to read them as plain text.

## eo-driver

A headless driver for a *full* Oberon image: boots it on the `risc-core` CPU with
no window, drives it (move the pointer, middle-click to execute, push files over
PCLink), and observes it (framebuffer hash, ink density, PGM dump, serial capture).
It is the seed-regeneration tool for `build-eo-image` and the foundation for the
on-EO control plane — see [`BUILD-EO-IMAGE.md`](BUILD-EO-IMAGE.md) for the flags
and recipes.

```sh
cargo run -p host-tools --release --bin eo-driver -- Oberon.dsk --frames 1000 --fb-out out.pgm
```

## eo-shim

Boots a directory's `InnerCore` under the headless [`shim`](src/shim.rs) and runs a
single Oberon command — the bring-up harness for the EO toolchain core, and a handy
way to run ad-hoc commands (or debug a boot with `OBERON_TRACE=1`):

```sh
cargo run -p host-tools --release --bin eo-shim -- assets/eo/bootstrap ORP.Compile Foo.Mod/s
```

## Tests

```sh
cargo test -p host-tools
```

Unit tests cover the converters' pure functions, the `.packonly` and import-order
logic, and the `image` pipeline's filesystem helpers; [`tests/cli.rs`](tests/cli.rs)
exercises each binary end-to-end (argument handling, exit codes, the build's
fail-clear paths) and boots the committed EO bootstrap seed to compile and run a
module (`eo_seed_boots_compiles_and_runs`). Two heavy, `#[ignore]`d round-trip
tests rebuild a whole system and check it boots identically:
`build_po_image_round_trips_the_golden` (the committed golden image) and
`build_eo_image_round_trips_a_bootable_desktop` (needs `EO_IMAGE`). They compile
all of Oberon through the shim, so run them explicitly:

```sh
cargo test -p host-tools --release -- --ignored
```

## License

[ISC](../../LICENSE), uniform with the rest of the workspace. The toolchain
assets under [`assets/`](assets) are vendored from project-norebo / Extended
Oberon and derive from Project Oberon 2013; they keep their upstream copyright
(also ISC) — see [`assets/README.md`](assets/README.md). The
[workspace README](../../README.md) covers the whole stack.
