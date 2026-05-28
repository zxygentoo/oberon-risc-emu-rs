# risc-core

The pure Project Oberon RISC5 machine: CPU, software floating-point, the MMIO
device traits, and the disk/serial/clipboard devices — with **no windowing or
platform UI**. A faithful, bit-exact Rust port of Peter De Wachter's
[`oberon-risc-emu`](https://github.com/pdewacht/oberon-risc-emu).

The port is structurally 1:1 with the C reference — each module corresponds to
one C source file — and is `std` but otherwise dependency-light (`bitflags` for
the ALU flags; `libc` only for the unix raw-serial device). That keeps it usable
behind a [`winit`](https://crates.io/crates/winit) frontend (the windowed
[`oberon-risc-emu`](../..) crate is the reference consumer), a headless runner, or —
in future — wasm/libretro. The crate is `#![deny(unsafe_code)]` save two audited
spots (the `cosim` FFI and the unix `raw_serial` device), each carrying a
module-level allow and safety note.

## Modules

| Module      | Port of          | Role                                                          |
| ----------- | ---------------- | ------------------------------------------------------------- |
| `risc`      | `risc.c`         | RISC5 CPU core, memory map, and the public [`Risc`] API       |
| `fp`        | `risc-fp.c`      | software floating-point and integer division                  |
| `io`        | `risc-io.h`      | device callback traits the CPU calls out through              |
| `disk`      | `disk.c`         | the SPI SD-card state machine                                 |
| `boot_rom`  | `risc-boot.inc`  | the 512-word boot ROM                                         |
| `pclink`     | —                | `PCLink` host file-transfer serial protocol                   |
| `raw_serial` | —                | raw host serial line over a unix tty/pipe (unix only)         |
| `clipboard` | —                | the clipboard GET/PUT state machine bridging a host clipboard |
| `headless`  | —                | deterministic windowless driver + framebuffer/state hashing   |

[`Risc`]: src/risc.rs

## Using it

Build the core alone — no GUI, no system libraries:

```sh
cargo build -p risc-core
```

Construct a machine, attach the disk as the SPI slave at index 1, and drive it.
The `headless` helper runs the same deterministic 60 Hz clock as the boot golden:

```rust
use std::path::Path;
use risc_core::disk::Disk;
use risc_core::risc::Risc;

let mut risc = Risc::new();
risc.set_spi(1, Box::new(Disk::new(Some(Path::new("Oberon.dsk")))?));

// Boot 250 frames windowlessly, then hash the result.
risc_core::headless::run_frames(&mut risc, 250);
let fb = risc_core::headless::framebuffer_hash(&risc);
let st = risc_core::headless::state_hash(&risc);
```

A live frontend instead pumps `set_time` / `run` per frame and reads
`framebuffer()`; see the windowed crate's `app.rs`. Devices are injected through
the `io` traits (`set_serial`, `set_spi`, `set_clipboard`, `set_leds`), so the
core stays free of platform code.

## Tests

```sh
cargo test -p risc-core           # CPU + FP units, no GUI deps
```

- **FP** — ~15k C-generated vectors in [`tests/data/fp_vectors.txt`](tests/data/fp_vectors.txt).
- **Boot golden** — `tests/cpu.rs` hashes the framebuffer + CPU state against the
  C reference at fixed checkpoints. Gated on `OBERON_DISK` (an absolute path to a
  disk image, since each crate's tests run from its own directory):

  ```sh
  OBERON_DISK="$PWD/../../DiskImage/Oberon-2020-08-18.dsk" cargo test -p risc-core
  ```

- **Live co-simulation** — the opt-in `cosim` feature compiles the C reference
  (via [`cosim/shim.c`](cosim/shim.c)) and compares every FP/`idiv` result, a
  random instruction over random state, and a full-boot lockstep. Needs a C
  toolchain and the sibling C repo:

  ```sh
  OBERON_C_SRC=/path/to/oberon-risc-emu/src \
  OBERON_DISK="$PWD/../../DiskImage/Oberon-2020-08-18.dsk" \
    cargo test -p risc-core --release --features cosim
  ```

  Iteration counts are tunable via `COSIM_FP_ITERS` / `COSIM_INSN_ITERS`.

### Regenerating the fixtures

The FP vectors and boot-golden hashes above aren't hand-written — they're
captured from the C reference by the two harnesses in [`tools/`](tools), which
`#include` it directly to reach its `static` internals. Both need a checkout of
[`oberon-risc-emu`](https://github.com/pdewacht/oberon-risc-emu) (the same C
source the `cosim` feature compiles); run them from this crate's directory:

```sh
C=/path/to/oberon-risc-emu/src        # the reference's src/, same as OBERON_C_SRC

# FP vectors: deterministic, no disk needed. Overwrites the vector file in place.
gcc -O2 -I "$C" tools/gen_fp_vectors.c "$C/risc-fp.c" -o /tmp/gen_fp
/tmp/gen_fp > tests/data/fp_vectors.txt

# Boot golden: the boot writes to the disk, so run on a throwaway copy. Prints
# one "<frame> <fb_hash> <state_hash>" line per checkpoint on stdout.
gcc -O2 -I "$C" tools/gen_boot_golden.c "$C/risc-fp.c" "$C/disk.c" -o /tmp/gen_boot
cp "$PWD/../../DiskImage/Oberon-2020-08-18.dsk" /tmp/golden.dsk
/tmp/gen_boot /tmp/golden.dsk
```

`gen_fp_vectors` rewrites [`tests/data/fp_vectors.txt`](tests/data/fp_vectors.txt)
directly. `gen_boot_golden` only prints — paste its lines into the checkpoint
table in [`tests/cpu.rs`](tests/cpu.rs), whose frame list must match the
`checkpoints[]` in the harness. Regenerate only when the reference's behaviour
legitimately changes; the one intentional divergence from it is recorded in
[`DIVERGENCES.md`](../../DIVERGENCES.md).

## License

[ISC](../../LICENSE) — the same license as the upstream `oberon-risc-emu` it
ports (© Peter De Wachter) and Project Oberon itself. See the
[workspace README](../../README.md) for the whole stack.
