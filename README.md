# oberon-risc-emu-rs

A Rust port of Peter De Wachter's [`oberon-risc-emu`](https://github.com/pdewacht/oberon-risc-emu) —
an emulator for Niklaus Wirth's Project Oberon RISC5 machine. It boots a disk
image to the interactive desktop.

![Project Oberon (Oberon V5) desktop](po2013.png)

- **Pure Rust, no system SDL** — [`winit`](https://crates.io/crates/winit) +
  [`softbuffer`](https://crates.io/crates/softbuffer) +
  [`arboard`](https://crates.io/crates/arboard).
- **Workspace** — a dependency-light [`risc-core`](crates/risc-core) library
  (CPU, software FP, MMIO, disk, serial), the top-level `oberon-risc-emu`
  crate (the windowing frontend, which builds the `risc` executable you run),
  and [`host-tools`](crates/host-tools): standalone host utilities for
  Oberon files.
- **Bit-exact to the C reference** — FP vectors, a C-derived boot golden, and
  live co-simulation — save one [documented divergence](DIVERGENCES.md).

## Quickstart

If you already have cargo installed and just want to play with the Oberon system, run:

```sh
make oberon
```

This builds the emulator and boots `Oberon-2020-08-18.dsk` from [`DiskImage/`](DiskImage).

## Other Make targets

A `Makefile` wraps Cargo for the common workflows:

| Command       | What it does                                                          |
| ------------- | -------------------------------------------------------------------- |
| `make`        | build the emulator → `target/release/risc`                           |
| `make tools`  | build the host CLI tools → `target/release/`                         |
| `make test`   | run the whole-workspace test suite                                    |
| `make bench`  | run the render hot-path microbenchmark                                |
| `make clean`  | `cargo clean`                                                         |

`make oberon DISK=other.dsk` boots a different image; the upstream repo has
[other dated versions](https://github.com/pdewacht/oberon-risc-emu/tree/master/DiskImage).

## Controls

Oberon expects a US keyboard layout and a three-button mouse; the left `Alt` key
acts as the middle button. Hotkeys: `F12` / `Ctrl+Shift+Delete` reset · `F11` /
`Alt+Enter` fullscreen · `Alt+F4` quit.

## Command line options

`./target/release/risc [OPTIONS] DISK-IMAGE`:

- `--fullscreen` — start in fullscreen.
- `--mem MEGS` — give the machine more than its default 1 MB of RAM.
- `--size WIDTHxHEIGHT` — use a non-standard framebuffer/window size.
- `--leds` — print LED changes to stdout (handy for kernel work, noisy otherwise).

`./target/release/risc --help` lists the rest (`--zoom`, `--serial-in`/`--serial-out`, `--boot-from-serial`).

## Transferring files

Oberon's `PCLink` (the default serial device) copies files to and from the host:

1. In Oberon, middle-click **`PCLink1.Run`** to start the transfer task.
2. On the host, use upstream's
   [`pcsend.sh` / `pcreceive.sh`](https://github.com/pdewacht/oberon-risc-emu)
   — our protocol is wire-compatible. They drop a `PCLink.REC` (a host file to
   send *to* Oberon) or `PCLink.SND` (a file to receive *from* Oberon) job file
   in the emulator's working directory.

For text, the clipboard is simpler.

## Clipboard integration

The bundled image ships a `Clipboard` module bridging Oberon to the host OS
clipboard (via [`arboard`](https://crates.io/crates/arboard)). Middle-click:

- `Clipboard.Paste` — insert the host clipboard at the caret.
- `Clipboard.CopySelection` — copy the current text selection to the host.
- `Clipboard.CopyViewer` — copy the focused viewer to the host.

## Host tools

The [`host-tools`](crates/host-tools) crate bundles command-line tools for
working with Oberon on the host. `ob2unix` and `asciidecoder` are pure-`std` file
converters (ported from `oberon-risc-emu`'s `tools/`) and `extract-source` reads a
disk image directly; `build-image` runs a headless Oberon on the `risc-core` CPU
(a port of [`project-norebo`](https://github.com/pdewacht/project-norebo)). Build
them with `make tools`; the examples below run them straight from `target/release/`.

- **`ob2unix`** — dump the plain-text content of an Oberon text: drops the binary
  header and converts CR line endings to LF (a non-Oberon file passes through). It
  takes the file to convert as an argument:

  ```sh
  ./target/release/ob2unix Input.Mod > input.txt
  ```

- **`asciidecoder`** — extract the files from an `AsciiCoder.DecodeFiles` archive;
  `-v` lists each extracted name, `-C DIR` sets the output directory:

  ```sh
  ./target/release/asciidecoder -v -C outdir archive.txt
  ```

- **`build-image`** — compile Project Oberon from a source tree and assemble a
  runnable disk image (the Norebo toolchain is embedded). Fetch the sources
  separately first, e.g. with project-norebo's `fetch-sources.py`:

  ```sh
  ./target/release/build-image path/to/sources out.dsk
  ```

- **`extract-source`** — the inverse of `build-image`: extract the source files
  from a disk image into a host directory (reads the Oberon filesystem directly;
  no boot). Drops compiled `.rsc`/`.smb`, so the result feeds straight back into
  `build-image`:

  ```sh
  ./target/release/extract-source Oberon.dsk out/
  ```

## Known issues

- The wireless network interface is not emulated.
- Raw serial (`--serial-in` / `--serial-out`) is Unix-only.
- Oberon assumes a US keyboard layout.

## Test

```sh
cargo test --workspace    # core + frontend units
cargo test -p risc-core   # core alone, no GUI deps
```

Boot tests are gated on `OBERON_DISK`; point it at the bundled image with an
absolute path (`cargo test` runs each crate's tests from its own directory):

```sh
OBERON_DISK="$PWD/DiskImage/Oberon-2020-08-18.dsk" cargo test
```

- **FP** — ~15k C-generated vectors (`crates/risc-core/tests/data/fp_vectors.txt`).
- **Boot golden** — hashes the framebuffer + CPU state against C at fixed
  checkpoints; regenerated by the C harnesses in `crates/risc-core/tools/`.
- **Image build** — `host-tools`' `build_image_reproduces_the_boot_golden` builds
  a disk image from a source tree and boots it to that same golden hash; gated on
  `OBERON_SOURCES` (a fetched PO2013 tree, e.g. project-norebo's `upstream/`).
- **Live co-simulation** — the `cosim` feature compiles the C reference and
  compares every FP/`idiv` result, a random instruction over random state, and a
  full-boot lockstep, frame by frame. Needs a C toolchain and the sibling C repo:

```sh
OBERON_C_SRC=/path/to/oberon-risc-emu/src \
OBERON_DISK="$PWD/DiskImage/Oberon-2020-08-18.dsk" \
  cargo test -p risc-core --release --features cosim
```

Iteration counts are tunable via `COSIM_FP_ITERS` / `COSIM_INSN_ITERS`; the
render hot path has a `cargo bench` microbenchmark.

### Headless boots

The `headless` subcommand runs the core on the same deterministic 60 Hz clock as
the boot golden — windowless and byte-for-byte reproducible — so it's handy for
CI smoke checks and for regenerating golden hashes:

```sh
# run 250 frames, then print the framebuffer + CPU-state FNV-1a hashes
./target/release/risc headless --frames 250 --hash DiskImage/Oberon-2020-08-18.dsk
```

- `--frames N` — how many 60 Hz frames to boot (default 250).
- `--hash` — print FNV-1a hashes of the framebuffer and the `{PC, R, H, flags}`
  state; these line up with `boot_matches_c_reference`'s checkpoints (at frame
  250 they reproduce the C-derived golden). Omit it for a one-line liveness
  summary (frames run, blank framebuffer words) instead.

It boots a throwaway copy of the image, so the original is left untouched.

## License

[ISC](LICENSE) — the same license as the upstream
[`oberon-risc-emu`](https://github.com/pdewacht/oberon-risc-emu) it ports
(© Peter De Wachter) and Project Oberon itself, so the whole stack is uniform.

Bundled third-party material keeps its own upstream copyright (also ISC):

- `DiskImage/` — Project Oberon 2013 system software (its authors' work, from the
  upstream distribution);
- `crates/host-tools/assets/` — Norebo host modules and bootstrap objects vendored
  from [`project-norebo`](https://github.com/pdewacht/project-norebo), derived from
  Project Oberon 2013 (see [`README.md`](crates/host-tools/assets/README.md)).
