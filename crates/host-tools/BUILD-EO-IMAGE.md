# build-eo-image — porting the headless image build to Extended Oberon

**Goal:** a headless `build-eo-image` host tool that compiles an **Extended Oberon**
(EO — Pirklbauer's "Oberon-2 2020 Edition", system `AP 1.1.26`) disk image from
source, the way `build-po-image` does for Project Oberon 2013.

**Why:** run a coding agent *on* EO, whose `Modules` has safe module unloading +
finalization (`final`/`badfin`) — the memory-safety property stock PO2013 lacks.

> This is the living handoff/plan doc. It is written to be read cold (e.g. after a
> context clear) and resumed from. Branch: **`feat/build-eo-image`**.

---

## TL;DR — where we are

**It boots, compiles, links, and runs — headless, end to end.** The boot blocker is
solved and `build-eo-image` is written.

The fix was the inner-core **top module**: link **`Modules`**, not `Oberon`. Only the
top module's body runs at `PC=0`; the PO2013-norebo design relies on that body being
`Modules`, whose `BEGIN Init; Load("Oberon", M)` runs `Files.Init` → `Kernel.Init`
(heap) and then dynamically loads `Oberon` + the rest. Linking `Oberon` as a
self-contained core never initialised the heap (`Kernel.Init` never fired), so the
first `NEW` corrupted low memory and the PC eventually walked off into RAM
(`PC left RAM (0x00800000)`). Switching the top to `Modules` (the standard EO boot
body) fixed it outright; the `CoreLinker` fixups were already correct.

Now working, all via `eo-inner-run` / `host_tools::shim`:
- the `Modules`-topped inner core (23 KB) boots and dispatches commands (exit 0);
- the EO compiler compiles a module headlessly (`ORP.Compile Foo.Mod/s` → `Foo.rsc`);
- the ported `CoreLinker` re-links the inner core *in the shim*, byte-identical to
  the live-EO link (the golden round-trip property);
- a freshly compiled command runs and its output reaches the host.

**`build-eo-image <sources> <out.dsk>` is complete** — the EO peer of `build-po-image`.
From an Extended Oberon source tree it compiles the EO toolchain in the shim, links a
fresh inner core, compiles the *whole* EO system, and assembles a **bootable
`Oberon.dsk`**. The result boots to the full EO desktop in the emulator, byte-for-byte
identical to the original image's boot (`risc Oberon.dsk` for the GUI; the round-trip
test boots both headless and compares framebuffer hashes). The pipeline is now shared
with `build-po-image` (same `NOREBO_MODULES`, `.rsx` rename, `CoreLinker.LinkDisk` +
`VDiskUtil.InstallFiles`); the only EO specifics are the embedded glue seed and the
`Modules`-top inner core.

---

## What's built (all committed on `feat/build-eo-image`)

Commits (oldest→newest): `extract-source SD+keep-objects` · `EO groundwork` ·
`eo-driver pointer+middle-click` · `eo-driver bidirectional PCLink` ·
`eo-driver --push` · `EO Oberon stub` · `Oberon.Return (full toolchain compiles)` ·
`CoreLinker port (compiles)` · `CoreLinker reads .rsc + links clean InnerCore` ·
`shim→lib + eo-inner-run harness` · `shim PC-trace (OBERON_TRACE)` ·
`build-eo-image + EO bootstrap seed (boot solved: Modules-top)` ·
`build-eo-image → bootable Oberon.dsk (.rsx CoreLinker, shared resolve, round-trip)`.

- **`extract-source`** (`src/bin/extract-source.rs`): the `image` reader
  (`host_tools::image`, `src/image.rs`) auto-detects the FS offset (raw `.dsk` at 0, full
  SD `RISC.img` at `0x10000400` = `0x80002` blocks).
  `--keep-objects` also extracts `.rsc`/`.smb` (seed harvesting).
- **`eo-driver`** (`src/bin/eo-driver.rs`): host-side dev tool to boot, drive, and observe EO headless. Flags:
  `<image>`, `--frames N` (boot frames), `--after N` (post-input frames),
  `--fb-out f.pgm` (framebuffer dump), `--pclink-dir DIR` (PcLink serial backend),
  `--move-to X,Y` (screen px), `--mid-click` (Oberon execute), `--push HOSTFILE`
  (repeatable; PCLink-push after the click). Verified: boots EO to desktop;
  middle-click executes commands; bidirectional byte-accurate PCLink; multi-file
  push.
- **`assets/eo/glue/`** — EO host glue, all compile clean under EO:
  `Kernel.Mod` (EO Kernel + `Trap→Norebo.Trap`), `Disk.Mod` (EO Disk minus SD/SPI;
  `Get/PutSector` abort), `Oberon.Mod` (norebo stub adapted: GC→`Kernel.Collect`+
  `Modules.Collect`, 4-arg `Kernel.New`, `mod.prg`, added `Oberon.Return`),
  `CoreLinker.Mod` (EO object-format offline linker — see below).
  Shared from **`assets/common/`** unchanged: `Norebo.Mod`, `FileDir.Mod`,
  `Files.Mod` (host-backed, work as-is for EO).
- **`risc_core::shim`** (`../risc-core/src/shim.rs`): the headless runtime, moved
  out of `build-po-image` into the core crate so any inner core can be booted. The
  builders use `risc_core::shim::run`.
- **`eo-inner-run`** (`src/bin/eo-inner-run.rs`): `eo-inner-run <DIR> <Module.Proc> [param…]` —
  boots `DIR/InnerCore` and runs one command. The bring-up harness.
- **`assets/eo/bootstrap/`** — the vendored EO bootstrap seed: the `Modules`-topped
  `InnerCore` (23 KB) + the 14 glue-compiled toolchain `.rsc` (`Kernel`…`ORP`,
  `CoreLinker`). `build-eo-image` embeds these; `InnerCore` is the golden image the
  round-trip checks against.
- **`build-eo-image`** (`src/bin/build-eo-image.rs`): `build-eo-image <sources> <out.dsk>`
  — the EO counterpart of `build-po-image`. Embeds the seed (glue + `VDisk` family), then
  mirrors `build-po-image`: compile the toolchain → link a fresh `Modules`-topped inner
  core → compile the *whole* EO source tree → `CoreLinker.LinkDisk` the boot core →
  `VDiskUtil.InstallFiles`, producing a **bootable `Oberon.dsk`**. The whole pipeline
  is shared in `host_tools::pipeline` (compile→link→install) + `host_tools::resolve`
  (compile-order); each binary is just its embedded `Seed` and CLI.
- **`risc-core` PC-trace** (`src/risc.rs` `shim_run`): set `OBERON_TRACE=1` to dump
  the instruction count, a ring of the last instructions, and the registers when a run
  leaves RAM / exhausts its budget — plus a trip-wire on executing a zero word (a wild
  branch into zeroed memory). Zero-cost when unset.

The PO2013 `build-po-image` + its golden round-trip test (`#[ignore]`) are untouched and
green (the `resolve` move is import-only). EO tests: `eo_seed_boots_compiles_and_runs`
(hermetic — the committed seed boots, compiles `Tiny`, runs it) and
`build_eo_image_round_trips_a_bootable_desktop` (`#[ignore]`, needs `EO_IMAGE`: extract
→ rebuild → boot, asserting the rebuilt disk's framebuffer equals the original's).

---

## The EO `CoreLinker` (the hard part — done, validated)

`assets/eo/glue/CoreLinker.Mod` is EO's `Modules.Load` reading/fixup logic ported
onto PO2013 norebo `CoreLinker`'s buffer/relative-address structure. EO deltas vs
PO2013, all handled:
- descriptor `ImageModDesc`: 9 addr fields `var/str/tdx/prg/imp/cmd/ent/ptr/pvr` +
  `final`, **`DescSize=96`**.
- section read order: `var, str, tdx, prg(code), imp, cmd, ent, ptr, **pvr**`.
- **4** fixup origins `P/D/T/**M**` (method-table fixup is EO-new), EO's instruction
  encoding (`MOV/BLT`, `U/B`, `C4..C26`), EO's **absolute** global addressing
  (2-word `MOV`), not PO2013 MT-indirection.
- `ThisFile` reads **`.rsx`** (the offline-link convention: the build renames the
  freshly compiled objects `.rsc`->`.rsx` around each link, so they don't collide with
  the live `.rsc` the shim loads to *run* the linker — same as the PO2013 `CoreLinker`).
- boot setup: `buffer[0] := BCT + body DIV 4 - 1` (branch to top body),
  `buffer[4/5/6]` = AllocPtr/topaddr/0x40000, `buffer[MTOrg DIV 4 + num] := addr`
  (MTOrg=20H). **Validated:** `buffer[6]` (0x40000) is moot — `boot_inner_core`
  overwrites `MEM[24]` with `STACK_ORG=0x80000`, which `Kernel.Init` reads as
  `heapOrg`, matching PO2013.

The fixups were **correct from the start**. The boot failure was the *top module*,
not the linker: `CoreLinker.LinkSerial Modules InnerCore` → `Linking InnerCore 23148`
→ a 23 KB core that boots. Re-linking that same core *inside the shim* with the glue
`CoreLinker` produces a **byte-identical** image (the golden round-trip). The same
`CoreLinker.LinkDisk` writes the boot core onto the output `Oberon.dsk` in
`build-eo-image`, which then boots the full EO desktop.

---

## RESOLVED: boot the inner core (link `Modules`, not `Oberon`)

**Root cause.** `boot_inner_core` sets `PC=0`, so only the **top** module's body runs.
PO2013 norebo links **`Modules`** as the top; its body is
`BEGIN Init; Load("Oberon", M)`, and `Modules.Init` calls `Files.Init`, which calls
**`Kernel.Init`** (the heap) + `FileDir.Init`. Then `Load("Oberon")` dynamically loads
`Oberon` and its remaining imports (`RS232`, `Texts`, `Fonts`; `Norebo` is already in
the core) from `.rsc`, running each body. The EO bring-up had linked **`Oberon`** as a
self-contained top instead — so `Kernel.Init` never ran, the first `NEW` allocated on a
dead free-list (returned NIL), NIL writes corrupted low memory, and the PC eventually
branched into zeroed RAM (`PC left RAM (0x00800000)`).

**Fix:** link `Modules` as the inner-core top. The init chain then runs exactly as in
PO2013, and `Oberon` + the runtime load dynamically — so the core is small (23 KB) and
the boot directory must also carry the dynamically-loaded `.rsc` (`Oberon`, `RS232`,
`Texts`, `Fonts`, and whatever the command pulls in). This is the same model
`build-po-image` uses; `build-eo-image` (below) wires it all up.

Confirmed empirically with the env-gated PC-trace (`OBERON_TRACE=1`, in `shim_run`):
the `Oberon`-top core ran ~1M instructions of confused execution (SP clobbered to
`0xA838`, low RAM full of garbage records) before walking off; the `Modules`-top core
boots, dispatches, and exits cleanly (0 success, 3 `badkey`, 6 `nocmd` — all real
`Modules.res` codes).

## DONE: the bootable disk (`build-eo-image`)

The GUI-bootable `.dsk` is built exactly like `build-po-image`'s, not via EO's
`ORL`/`Disk`-sector path: the boot core is written with **`CoreLinker.LinkDisk`** and
the filesystem with **`VDiskUtil.InstallFiles`**, both running in the shim on a host
file. This works for EO because the on-disk FS format is shared (`FileDir.Mod`/
`Files.Mod` are unchanged from PO2013), so the `VDisk` family compiles against the EO
glue **unchanged** and produces an EO-readable disk; and the inner core boots via the
same `Modules`-top mechanism whether the ROM loads it from disk or the shim sets `PC=0`
(the ROM PROM is shared RISC5 hardware). So `build-eo-image` is structurally identical
to `build-po-image` — the realisation that closed the gap.

Round-trip, validated: extract a pristine EO image's sources → `build-eo-image` → the
rebuilt `Oberon.dsk` boots to the **same** EO desktop as the original, framebuffer
hash `0x1bed5d10ac9ec259` for both (AP 1.1.26). The boot core needs the real EO `Disk`
(real `Files` imports it), which links + runs on the emulator's real (emulated) disk —
no host stub involved at GUI-boot time.

Possible follow-ups (not needed for the round-trip): EO's *native* `ORL.Link`/`ORL.Load`
path (would need a shim disk-sector backend) for parity with how EO builds itself; and
a way to size/trim the output image.

---

## Reproduction recipes (exact)

Binaries: `cargo build --release -p host-tools` (eo-driver, eo-inner-run, build-eo-image,
build-po-image, extract-source, ob2txt, txt2ob). EO sources extract as plain Latin-1
with CR line endings; read one as host text with `target/debug/ob2txt <file>` (writes
`<file>.txt`).

**Round-trip — extract → build → boot (~4 s build):**
```
CLEAN=$(find /tmp/eo-clean -iname RISC.img | head -1)         # a pristine EO AP 1.1.26 image
target/release/extract-source "$CLEAN" /tmp/eo-src            # clean .Mod tree + .packonly
target/release/build-eo-image /tmp/eo-src /tmp/eo-out.dsk     # compile all of EO -> bootable disk
target/release/eo-driver /tmp/eo-out.dsk --frames 1000 --fb-out /tmp/eo.pgm   # boot it headless
target/release/risc /tmp/eo-out.dsk                          # ...or boot it in the GUI window
```
The rebuilt disk boots to the EO desktop, framebuffer hash `0x1bed5d10ac9ec259` —
identical to booting `$CLEAN` itself. Extract *without* `--keep-objects` (a clean
`.Mod`+data tree); a stale `.smb`/`.rsc` in the tree would shadow the fresh build (see
Gotchas). To compile/run ad-hoc EO commands headless, point `eo-inner-run` at any directory
holding an `InnerCore` + the needed `.rsc` (e.g. the seed in `assets/eo/bootstrap/`).

**Regenerating the seed.** Routine re-vendor (after a glue tweak) is easiest via the
shim: recompile the changed glue against the current seed (`eo-inner-run assets/eo/bootstrap
ORP.Compile X.Mod/s` → fresh `X.rsc`) and copy it into `assets/eo/bootstrap/`; a fresh
`build-eo-image` run then re-derives + golden-checks the `InnerCore`. The from-scratch
bootstrap below drives the *live* EO emulator instead — note it predates the `.rsx`
`CoreLinker`, so its `CoreLinker` reads `.rsc` (the live system's own objects) and its
final `ORP.Compile CoreLinker.Mod` must be the `.rsx`-reading source for the seed.
`CoreLinker` is compiled FIRST against EO's *real* modules (so it loads in the running
system to do the link), then recompiled at the end for the seed.
```
CLEAN=$(find /tmp/eo-clean -iname RISC.img | head -1)     # EO AP 1.1.26; or untar /tmp/S3RISCinstall.tar.gz
printf 'Oberon.Batch  ORP.Compile CoreLinker.Mod/s ~  ORP.Compile Norebo.Mod/s Kernel.Mod/s Disk.Mod/s FileDir.Mod/s Files.Mod/s ~  ORP.Compile Modules.Mod/s Fonts.Mod/s Texts.Mod/s RS232.Mod/s Oberon.Mod/s ~  ORP.Compile ORS.Mod/s ORB.Mod/s ~  ORP.Compile ORG.Mod/s ~  ORP.Compile ORP.Mod/s ~  CoreLinker.LinkSerial Modules InnerCore ~  ORP.Compile CoreLinker.Mod/s ~  System.ShowModules ~\r' > /tmp/eo-build/System.Tool
cp "$CLEAN" /tmp/eo-work.img; rm -rf /tmp/eo-xfer && mkdir -p /tmp/eo-xfer
target/release/eo-driver /tmp/eo-work.img --frames 800 --pclink-dir /tmp/eo-xfer --move-to 685,552 --mid-click \
  --push crates/host-tools/assets/common/Norebo.Mod --push crates/host-tools/assets/eo/glue/Kernel.Mod \
  --push crates/host-tools/assets/eo/glue/Disk.Mod --push crates/host-tools/assets/common/FileDir.Mod \
  --push crates/host-tools/assets/common/Files.Mod --push crates/host-tools/assets/eo/glue/Oberon.Mod \
  --push crates/host-tools/assets/eo/glue/CoreLinker.Mod --push /tmp/eo-build/System.Tool --after 500
target/release/eo-driver /tmp/eo-work.img --frames 800 --move-to 685,280 --mid-click --after 80000 --fb-out /tmp/eo.pgm
python3 -c "from PIL import Image; Image.open('/tmp/eo.pgm').save('/tmp/eo.png')"   # read /tmp/eo.png to see the log
rm -rf /tmp/eo-ls && cargo run -q -p host-tools --bin extract-source -- --keep-objects /tmp/eo-work.img /tmp/eo-ls
cp /tmp/eo-ls/InnerCore crates/host-tools/assets/eo/bootstrap/
for m in Kernel FileDir Files Modules Norebo Oberon CoreLinker Fonts Texts RS232 ORS ORB ORG ORP; do cp /tmp/eo-ls/$m.rsc crates/host-tools/assets/eo/bootstrap/; done
```
**Click coords (deterministic):** on EO's *original* System.Tool after a clean boot,
`PCLink1.Run` is at screen **(685,552)**; in our pushed minimal System.Tool the first
content line (`Oberon.Batch …`) is at **(685,280)**.

**Boot test (direct):**
```
target/release/eo-inner-run /tmp/eo-system Oberon.OpenLog          # → exit 0 (boots + dispatches)
OBERON_TRACE=1 target/release/eo-inner-run /tmp/eo-system <cmd>    # → trace on PC-left-RAM / budget
```

---

## Gotchas learned (important)

- **Oberon text = CR (`0x0D`) line endings.** A pushed `System.Tool` with `\n`(LF)
  renders as ONE merged line. Use `\r`. (Glue *source* compiles fine with LF — ORP
  treats LF as whitespace; only viewer/tool text needs CR.)
- **Compile in ~5-module groups** *when driving live EO* (separate `ORP.Compile … ~`
  commands). `Oberon.Batch` GCs between commands; one giant `ORP.Compile` exhausts the
  desktop's heap → `RECURSIVE TRAP 4 in Texts` on the big `ORG`/`ORP`. Under the shim
  this is moot — each `eo-inner-run`/`build-eo-image` `ORP.Compile` is a *fresh boot* with a
  clean 7.5 MB heap — but `build-eo-image` keeps small groups anyway for safety.
- **Stale `.smb` suppress fresh symbol files.** The compiler only *writes* `X.smb` when
  the export interface changed vs. an existing `X.smb` it can find on the search path.
  So if the source tree carries stock `.smb` (an `--keep-objects` tree), compiling
  against it produces `X.rsc` but no `X.smb`, and downstream imports fail with `import
  not available`. `build-eo-image` sidesteps this by staging only `.Mod` into a clean
  compile dir — so every `.smb` is regenerated and shipped with its `.rsc`.
- **Import-key rule:** to *run* a freshly-compiled module in a running system, it must
  be compiled against that system's *loaded* modules' interfaces. In live EO that means
  compiling `CoreLinker` against the *real* EO modules (so it loads to do the link),
  then recompiling it against the *glue* at the end for the seed. Under the shim there's
  no split: the seed compiler is already the glue, so everything compiles glue-first.
- **`Oberon.Batch` mechanism** (`Oberon.Mod`): runs every `Module.Proc … ~` command
  following it in the clicked text, GC-ing between each. Our minimal pushed
  System.Tool replaces EO's, so it must NOT be pushed until everything else is (it
  removes the original `PCLink1.Run`).
- **Don't mutate the pristine image**; always work on a copy from the tar.gz.

## Key code references

- `risc-core/src/risc.rs`: `boot_inner_core` — loads `(len,addr,bytes)` records, sets
  `MEM[12]=memsize`, `MEM[24]=stack_org`, `PC=0`, `R12=0x20`(=MTOrg), `R14=stack_org`.
  `STACK_ORG=0x80000`, `MEM_BYTES=8MB`. `shim_run` — runs to exit/trap/`PC left RAM`;
  honours `OBERON_TRACE` (ring of last instructions + regs + zero-word trip-wire).
  `disk.rs:57` — the `0x80002` SD rebase.
- `risc_core::shim` (`../risc-core/src/shim.rs`): syscall ABI; `trap` (~190) prints
  trap-type + module + pos (so boot crashes are debuggable).
- `src/bin/build-eo-image.rs`: just the embedded EO `Seed` — `TOOLCHAIN`
  (glue + `VDisk` family + `eo/bootstrap` objects) + the golden `InnerCore` + the CLI.
  The pipeline (`build`, `NOREBO_MODULES`, the `.rsx`/link/install steps) lives in
  `host_tools::pipeline`; compile order in `host_tools::resolve`. Both shared with
  `build-po-image`.
- EO reference sources (`ob2txt`'d to `.txt`, regenerate with `target/debug/ob2txt`):
  `/tmp/eo-txt/{Modules,Kernel,Disk,Files,FileDir,Oberon}.Mod`. EO's `Modules.Load`
  (`/tmp/eo-txt/Modules.Mod` ~56–220) is the fixup/format reference for `CoreLinker`.
- Harvested EO seed: `/tmp/eo-seed/` (sources + objects via `--keep-objects`).

## Decisions

- **Inner-core top module is `Modules`, not `Oberon`.** Only the top body runs at boot;
  `Modules`'s body is the standard EO init+load sequence. A self-contained `Oberon`-top
  core can't initialise the heap (its body never calls `Kernel.Init`). See "RESOLVED".
- **The shim inner core uses `CoreLinker.LinkSerial` (norebo serializer); the output
  disk uses `CoreLinker.LinkDisk`** — not EO's native `ORL`. The shim boots the
  `(len,addr,bytes)` serial core; the disk boot core is the `LinkDisk` format the shared
  RISC5 PROM loads. Both link `Modules`-top, so both boot the same way. (EO's own
  `ORL.Link`/`ORL.Load` would need a shim disk-sector backend — a possible follow-up.)
- **`build-eo-image` is structurally `build-po-image` with a different seed.** Same
  `NOREBO_MODULES`, `.rsx` rename, `CoreLinker.LinkDisk` + `VDiskUtil.InstallFiles`. The
  `VDisk` family + the FS format are shared (PO2013 ≡ EO `FileDir`/`Files`), so they
  port unchanged. Compile-order resolution lives in `host_tools::resolve` (shared).
- **`build-eo-image` output is a bootable `Oberon.dsk`** (the EO desktop), like
  `build-po-image`. For ad-hoc headless runs, `eo-inner-run` runs commands against any
  `InnerCore`+objects directory (e.g. the vendored seed).
- **Bootstrap via the scripted `eo-driver`** (drive the real EO emulator), not a
  manual GUI session — reproducible and scriptable. It's host-side scaffolding, not
  the on-EO agent's interface: that agent is an Oberon module using EO's own internal
  interfaces.
