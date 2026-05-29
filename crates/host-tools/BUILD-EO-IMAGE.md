# build-eo-image — porting the headless image build to Extended Oberon

**Goal:** a headless `build-eo-image` host tool that compiles an **Extended Oberon**
(EO — Pirklbauer's "Oberon-2 2020 Edition", system `AP 1.1.26`) disk image from
source, the way `build-image` does for Project Oberon 2013.

**Why:** run a coding agent *on* EO, whose `Modules` has safe module unloading +
finalization (`final`/`badfin`) — the memory-safety property stock PO2013 lacks.

> This is the living handoff/plan doc. It is written to be read cold (e.g. after a
> context clear) and resumed from. Branch: **`feat/build-eo-image`**.

---

## TL;DR — where we are

The full pipeline runs end-to-end headless: **compile → link → boot.** The entire
EO toolchain + host glue compiles under EO; the ported `CoreLinker` links a clean
43 KB inner core; `eo-shim` boots it and reaches the CPU. **Current blocker:** that
boot ends in `shim: PC left RAM (0x00800000)` — a linker fixup/layout value and/or
missing module-init orchestration. We are at **first-light debugging** of the
linked core. Everything before this is done and committed (10 commits).

---

## What's built (all committed on `feat/build-eo-image`)

Commits (oldest→newest): `extract-source SD+keep-objects` · `EO groundwork` ·
`eo-driver pointer+middle-click` · `eo-driver bidirectional PCLink` ·
`eo-driver --push` · `EO Oberon stub` · `Oberon.Return (full toolchain compiles)` ·
`CoreLinker port (compiles)` · `CoreLinker reads .rsc + links clean InnerCore` ·
`shim→lib + eo-shim harness`.

- **`extract-source`** (`src/bin/extract-source/`): `dsk.rs` auto-detects the FS
  offset (raw `.dsk` at 0, full SD `RISC.img` at `0x10000400` = `0x80002` blocks).
  `--keep-objects` also extracts `.rsc`/`.smb` (seed harvesting).
- **`eo-driver`** (`src/bin/eo-driver/`): the headless EO control plane. Flags:
  `<image>`, `--frames N` (boot frames), `--after N` (post-input frames),
  `--fb-out f.pgm` (framebuffer dump), `--pclink-dir DIR` (PcLink serial backend),
  `--move-to X,Y` (screen px), `--mid-click` (Oberon execute), `--push HOSTFILE`
  (repeatable; PCLink-push after the click). Verified: boots EO to desktop;
  middle-click executes commands; bidirectional byte-accurate PCLink; multi-file
  push.
- **`assets/eo-norebo/`** — EO host glue, all compile clean under EO:
  `Kernel.Mod` (EO Kernel + `Trap→Norebo.Trap`), `Disk.Mod` (EO Disk minus SD/SPI;
  `Get/PutSector` abort), `Oberon.Mod` (norebo stub adapted: GC→`Kernel.Collect`+
  `Modules.Collect`, 4-arg `Kernel.New`, `mod.prg`, added `Oberon.Return`),
  `CoreLinker.Mod` (EO object-format offline linker — see below).
  Shared from **`assets/Norebo/`** unchanged: `Norebo.Mod`, `FileDir.Mod`,
  `Files.Mod` (host-backed, work as-is for EO).
- **`host_tools::shim`** (`src/shim.rs`): the headless runtime, moved out of
  `build-image` into the lib so any inner core can be booted. `build-image` now
  uses `host_tools::shim::run`.
- **`eo-shim`** (`src/bin/eo-shim/`): `eo-shim <DIR> <Module.Proc> [param…]` —
  boots `DIR/InnerCore` and runs one command. The bring-up harness.

The PO2013 `build-image` + its golden round-trip test (`#[ignore]`) are untouched
and green.

---

## The EO `CoreLinker` (the hard part — done, links clean, boot-debugging)

`assets/eo-norebo/CoreLinker.Mod` is EO's `Modules.Load` reading/fixup logic ported
onto PO2013 norebo `CoreLinker`'s buffer/relative-address structure. EO deltas vs
PO2013, all handled:
- descriptor `ImageModDesc`: 9 addr fields `var/str/tdx/prg/imp/cmd/ent/ptr/pvr` +
  `final`, **`DescSize=96`**.
- section read order: `var, str, tdx, prg(code), imp, cmd, ent, ptr, **pvr**`.
- **4** fixup origins `P/D/T/**M**` (method-table fixup is EO-new), EO's instruction
  encoding (`MOV/BLT`, `U/B`, `C4..C26`), EO's **absolute** global addressing
  (2-word `MOV`), not PO2013 MT-indirection.
- `ThisFile` reads **`.rsc`** (in-system objects aren't renamed `.rsx`).
- boot setup: `buffer[0] := BCT + body DIV 4 - 1` (branch to top body),
  `buffer[4/5/6]` = AllocPtr/topaddr/0x40000, `buffer[MTOrg DIV 4 + num] := addr`
  (MTOrg=20H) — **mirrored from PO2013, NOT yet validated for EO**.

It **compiles** and **runs**: `CoreLinker.LinkSerial Oberon InnerCore` →
`Linking InnerCore 43304`, no errors → a 43316-byte core.

---

## CURRENT BLOCKER + next steps (the resume plan)

`eo-shim /tmp/eo-core System.Time` → **`shim: PC left RAM (0x00800000)`**
(0x800000 = `MEM_BYTES` = 8 MB ceiling). Boot reaches the CPU (`PC=0` → `buffer[0]`
→ branch to top=`Oberon`-stub body), then a branch/fixup sends PC out of RAM.

**Two likely causes (probably both):**

1. **A `CoreLinker` fixup/layout value is wrong** (the link is clean but an emitted
   *address* is off): the boot-branch (`body = prg + w`), or one of the 4 fixups
   (`BL`/global/type/method) computing a bad target. Reference to diff against is
   EO's real `Modules.Load` at `/tmp/eo-txt/Modules.Mod` lines ~112–213 (ob2unix'd).
2. **Module-init orchestration missing.** `boot_inner_core` sets `PC=0` → only the
   **top** module's body runs. The `Oberon` stub body currently does
   `Kernel.Install` + tasks + `OpenLog` + `ParamCall` but **never calls
   `Kernel.Init`** (heap) — so `NEW` in `OpenLog` hits an uninitialized heap.
   A working core must initialize *every* core module (`Kernel.Init`, `Modules`
   init, `Files`/`FileDir`/`Texts`/`Fonts`) before dispatch. Check which init via
   callable `Init` procs vs module bodies (bodies WON'T run — only the top's does).
   Fix: make the top stub orchestrate all inits, or teach `CoreLinker` to run each
   module body in load order.

**Debugging approach (ordered):**
1. Instrument `risc-core/src/risc.rs` `shim_run` (~line 406) to print the PC of the
   instruction *before* `PC left RAM` (the bad branch site), or add a small
   PC-trace. Map it back to a module via the core layout.
2. Likely fix path: first add `Kernel.Init` (+ `Modules`/`Files`/`Texts` init) to
   the `Oberon` stub body (cause #2); re-test. Then chase any remaining fixup bug
   (cause #1) by inspecting the linked image (dump `buffer[0]`, the `Oberon` prg/
   body, fixup targets) vs EO's `Modules.Load`.
3. Iterate: edit glue/`CoreLinker` → re-link in EO (recipe below, ~30 s) → harvest
   `InnerCore` → `eo-shim` boot → read `shim`'s trap line (it prints trap type +
   module) → fix. Repeat to first-light (boots + a trivial compile produces a `.rsc`).
4. After first-light: write the **`build-eo-image` bin** — embed the EO seed,
   pipeline like `build-image` but with the EO glue + `CoreLinker.LinkSerial` for the
   inner core, then compile user modules + `ORL.Link`/`ORL.Load` (or CoreLinker
   LinkDisk) the output `.dsk` + `VDiskUtil.InstallFiles`. Then an EO golden
   round-trip test mirroring `build_image_round_trips_the_golden`.

---

## Reproduction recipes (exact)

Binaries: `cargo build --release -p host-tools` (eo-driver, eo-shim, build-image,
extract-source, ob2unix). EO source as plain text: `target/debug/ob2unix <file>`.

**Clean EO image** (the original `/tmp/S3RISCinstall/RISC.img` was dirtied by boots):
```
mkdir -p /tmp/eo-clean && tar xzf /tmp/S3RISCinstall.tar.gz -C /tmp/eo-clean
CLEAN=$(find /tmp/eo-clean -iname RISC.img | head -1)     # full SD image, EO AP 1.1.26
cp "$CLEAN" /tmp/eo-work.img                              # writable working copy
```

**Click coords (deterministic):** on EO's *original* System.Tool after a clean boot,
`PCLink1.Run` is at screen **(685,552)**. In our pushed minimal System.Tool, the
first content line (`Oberon.Batch …`) is at **(685,280)**.

**Build the InnerCore (the current state):** push glue + a CR-terminated System.Tool,
then click `Oberon.Batch`. Note the **ordering**: `CoreLinker` is compiled FIRST
(against EO's *real* modules, so it loads in the running system), THEN the glue, THEN
`LinkSerial` (it reads the glue `.rsc` as files — import vs file-read are independent).
```
printf 'Oberon.Batch  ORP.Compile CoreLinker.Mod/s ~  ORP.Compile Norebo.Mod/s Kernel.Mod/s Disk.Mod/s FileDir.Mod/s Files.Mod/s ~  ORP.Compile Modules.Mod/s Fonts.Mod/s Texts.Mod/s RS232.Mod/s Oberon.Mod/s ~  CoreLinker.LinkSerial Oberon InnerCore ~  System.ShowModules ~\r' > /tmp/eo-build/System.Tool
cp "$CLEAN" /tmp/eo-work.img; rm -rf /tmp/eo-xfer && mkdir -p /tmp/eo-xfer
target/release/eo-driver /tmp/eo-work.img --frames 800 --pclink-dir /tmp/eo-xfer --move-to 685,552 --mid-click \
  --push crates/host-tools/assets/Norebo/Norebo.Mod --push crates/host-tools/assets/eo-norebo/Kernel.Mod \
  --push crates/host-tools/assets/eo-norebo/Disk.Mod --push crates/host-tools/assets/Norebo/FileDir.Mod \
  --push crates/host-tools/assets/Norebo/Files.Mod --push crates/host-tools/assets/eo-norebo/Oberon.Mod \
  --push crates/host-tools/assets/eo-norebo/CoreLinker.Mod --push /tmp/eo-build/System.Tool --after 500
target/release/eo-driver /tmp/eo-work.img --frames 800 --move-to 685,280 --mid-click --after 50000 --fb-out /tmp/eo.pgm
python3 -c "from PIL import Image; Image.open('/tmp/eo.pgm').save('/tmp/eo.png')"   # read /tmp/eo.png to see the log
rm -rf /tmp/eo-ls && cargo run -q -p host-tools --bin extract-source -- /tmp/eo-work.img /tmp/eo-ls
cp /tmp/eo-ls/InnerCore /tmp/eo-core/                     # 43316B
```

**Boot test:**
```
mkdir -p /tmp/eo-core && cp /tmp/eo-ls/InnerCore /tmp/eo-core/
timeout 30 target/release/eo-shim /tmp/eo-core System.Time    # → "PC left RAM (0x00800000)"
```
(For a real compile test later, also stage the compiler `.rsc` — `ORP/ORS/ORB/ORG`
compiled against the glue — + a `Foo.Mod` into `/tmp/eo-core`.)

---

## Gotchas learned (important)

- **Oberon text = CR (`0x0D`) line endings.** A pushed `System.Tool` with `\n`(LF)
  renders as ONE merged line. Use `\r`. (Glue *source* compiles fine with LF — ORP
  treats LF as whitespace; only viewer/tool text needs CR.)
- **Compile in ~5-module groups** (separate `ORP.Compile … ~` commands). `Oberon.Batch`
  GCs between commands; one giant `ORP.Compile` exhausts the heap → `RECURSIVE TRAP
  4 in Texts` on the big `ORG`/`ORP`.
- **Import-key rule:** to *run* a freshly-compiled module in the live EO system, it
  must be compiled against the system's *loaded* (real EO) modules. Hence compile
  `CoreLinker` before overwriting `Files`/`Oberon` with the glue.
- **`Oberon.Batch` mechanism** (`Oberon.Mod`): runs every `Module.Proc … ~` command
  following it in the clicked text, GC-ing between each. Our minimal pushed
  System.Tool replaces EO's, so it must NOT be pushed until everything else is (it
  removes the original `PCLink1.Run`).
- **Don't mutate the pristine image**; always work on a copy from the tar.gz.

## Key code references

- `risc-core/src/risc.rs`: `boot_inner_core` (~361) — loads `(len,addr,bytes)`
  records, sets `MEM[12]=memsize`, `MEM[24]=stack_org`, `PC=0`, `R12=0x20`(=MTOrg),
  `R14=stack_org`. `STACK_ORG=0x80000`, `MEM_BYTES=8MB`. `shim_run` (~406) — runs to
  exit/trap/`PC left RAM`. `disk.rs:57` — the `0x80002` SD rebase.
- `host_tools::shim` (`src/shim.rs`): syscall ABI; `trap` (~190) prints
  trap-type + module + pos (so boot crashes are debuggable).
- EO reference sources (ob2unix'd, regenerate with `target/debug/ob2unix`):
  `/tmp/eo-txt/{Modules,Kernel,Disk,Files,FileDir,Oberon}.Mod`. EO's `Modules.Load`
  (`/tmp/eo-txt/Modules.Mod` ~56–220) is the fixup/format reference for `CoreLinker`.
- Harvested EO seed: `/tmp/eo-seed/` (sources + objects via `--keep-objects`).

## Decisions

- **Inner core via `CoreLinker.LinkSerial` (norebo serializer), not `ORL.Link`.**
  `ORL.Link` makes the *output disk* boot file (FPGA format); the shim inner core
  needs the `(len,addr,bytes)` serial format → `CoreLinker`. (`ORL` is still the
  right tool for the eventual output-`.dsk` step in `build-eo-image`.)
- **Bootstrap via the scripted `eo-driver`** (drive the real EO emulator), not a
  manual GUI session — it's the same control plane the on-EO agent will use.
