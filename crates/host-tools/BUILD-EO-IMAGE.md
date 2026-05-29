# build-eo-image — porting the headless image build to Extended Oberon

**Goal:** a headless `build-eo-image` host tool that compiles an **Extended Oberon**
(EO — Pirklbauer's "Oberon-2 2020 Edition", system `AP 1.1.26`) disk image from
source, the way `build-image` does for Project Oberon 2013.

**Why:** run a coding agent *on* EO, whose `Modules` has safe module unloading +
finalization (`final`/`badfin`) — the memory-safety property stock PO2013 lacks.

Working notes for review; nothing here is committed yet.

## Status — branch `feat/build-eo-image`

Done + tested:

- **R3** — `extract-source/dsk.rs` auto-detects the filesystem offset, so the
  host reader handles EO's full `RISC.img` SD-card image (FS behind the
  `0x10000400` = `0x80002`-block prefix, the same base `risc-core/src/disk.rs`
  rebases by), not just a raw `.dsk`.
- **R1** — `extract-source --keep-objects` also extracts the compiled `.rsc`/`.smb`,
  to harvest a toolchain seed from a prebuilt image.
- Seed harvested from `/tmp/S3RISCinstall/RISC.img`: 95 sources + 107 objects
  (55 `.rsc`, 52 `.smb`) — the whole toolchain `ORS/ORB/ORG/ORP/ORL` plus the
  split core `Kernel`+`Disk`/`Files`/`FileDir`/`Modules`.
- `cargo test -p host-tools` green; the PO2013 golden round-trip is unaffected
  (`build_image_round_trips_the_golden` stays `#[ignore]`, unchanged).

Drafted (untested — no compiler in the loop yet): the EO host glue `Kernel` +
`Disk` in `assets/eo-norebo/` (see "Glue" below).

In progress: the **headless driver** (`eo-driver` bin). M1 (boot) and M2 (drive)
both working: boots EO headless (~86 ms to desktop); executes commands via a
synthetic middle-click (verified `System.Grow`, `Hilbert.Draw`); and moves files
both ways over PCLink, byte-accurate (fetched `Disk.Mod`, pushed a module onto
EO's disk and read it back). `risc-core` is unchanged.

Not started: the `ORL.Link` inner-core step, harvesting the seed, and the
`build-eo-image` bin. (Compiler `ORS/ORB/ORG/ORP` + `ORL` compile against the glue
— in progress.)

## Verified facts (this session)

- EO boots **and self-builds** on our emulator (`Build.Tool`/`Oberon.Batch`), so
  `risc-core` needs no changes — EO is stock RISC5.
- EO build recipe (`Build.Tool`): `ORP.Compile <core>` → `ORL.Link Modules`
  (→ `Modules.bin`) → `ORL.Load Modules.bin` (→ disk boot area). `ORL` is the
  replacement for `CoreLinker`.
- EO splits `Kernel` into `Kernel` + `Disk`; `ORL` writes the boot file via
  `Disk.PutSector` / `Disk.SectorLength`.
- `ORL` imports `SYSTEM, Kernel, Disk, Files, Modules, Texts, Oberon`. Its
  interactive surface (`Texts.*`, `Oberon.Par/Log/GetSelection`) is the same one
  `ORP` already uses headless under our Norebo glue — so the only **new** thing to
  host-map is `Disk`. (`Oberon.GetSelection` is for `ORL.Run`/`Execute`, not
  `Link`/`Load`; a no-op stub covers it.)
- `Modules.Mod` descriptor gained `final: Command`, `PROCEDURE Final`, and the
  `badfin` result; the loader computes `mod.final := mod.prg + w`. → reuse `ORL`
  (which emits that layout correctly) rather than reimplement it in a ported
  `CoreLinker`.
- On-disk FS format is unchanged PO2013 ↔ EO (the same `dsk.rs` reader works), so
  `VDisk*` image assembly, `resolve`, `.packonly`, and the `shim` syscall ABI all
  transfer unchanged.

## Architecture: build-image → build-eo-image

| Concern | Disposition |
| --- | --- |
| `shim` syscall runtime, `resolve` import-sort, `.packonly`, `VDisk*` assembly | **reuse as-is** |
| Compiler + linker + core objects (`ORS/ORB/ORG/ORP/ORL` + `Kernel/Disk/Files/FileDir/Modules`) | **harvest** from `RISC.img` (done) |
| Norebo glue: `Norebo`, `Kernel`, **+ new `Disk`**, `Files`, `FileDir`, `Oberon` stub | **port to EO** |
| Linker step | `CoreLinker.LinkSerial/LinkDisk` → `ORL.Link` / `ORL.Load` |
| `NOREBO_MODULES` list | `+Disk`, `+ORL`/`ORX`, `−CoreLinker` |
| Output | a `.dsk` (boots via the `disk.rs` rebase; no 260 MB SD image needed) |

## Glue (`assets/eo-norebo/` + shared `assets/Norebo/`)

The host glue for EO. **All of it compiles cleanly under EO's own compiler**
(verified via the `eo-driver` compile-test loop), and EO's real runtime
(`Modules`/`Fonts`/`Texts`/`RS232`) compiles against it.

| Module | Source | EO change | Status |
| --- | --- | --- | --- |
| `Norebo` | `assets/Norebo/` | none (syscall wrapper) | compiles ✓ |
| `Kernel` | `assets/eo-norebo/` | EO `Kernel` + Trap→`Norebo.Trap` | compiles ✓ |
| `Disk` | `assets/eo-norebo/` | EO `Disk` minus SD/SPI; `Get/PutSector` abort | compiles ✓ |
| `FileDir` | `assets/Norebo/` | none (host-backed, as-is) | compiles ✓ |
| `Files` | `assets/Norebo/` | none (host-backed, as-is) | compiles ✓ |
| `Oberon` (stub) | `assets/eo-norebo/` | GC→`Kernel.Collect`+`Modules.Collect`; 4-arg `New`; `mod.prg` | compiles ✓ |

Validated compile chain: `Norebo → Kernel → Disk → FileDir → Files → Modules →
Fonts → Texts → RS232 → Oberon`, all clean.

## Remaining work (ordered)

1. **EO Norebo glue.** `Kernel` + `Disk` drafted (see "Glue"); still to author:
   host `Files`/`FileDir` and the `Oberon` stub, matched to EO's interfaces.
   Compile against EO's real `Modules`/`Texts`/`Fonts`.
2. **Bootstrap a headless EO inner core — GATING.** `shim` needs a bootable core
   before it can run anything, but EO's prebuilt image only boots the full GUI.
   Two ways (we take **(b)** — see Decisions):
   - (a) **Interactive, one-time:** in the GUI emulator, compile the glue +
     `ORL.Link` a Norebo core, harvest the core + its `.rsc`, vendor them. Fastest
     to unblock.
   - (b) **Scripted (chosen) — the `eo-driver` bin.** M1 done: boots EO headless on
     `risc-core`, captures the serial line, dumps the framebuffer (EO reaches its
     desktop, verified by screenshot). M2: inject commands — middle-click
     (`mouse_button(2,…)`) a `Module.Proc` word, transfer files with `PcLink`, read
     results back from the System.Log viewer / serial. The same control plane the
     on-EO coding agent will need (inject command, read log, detect trap).
3. **`build-eo-image` bin.** Embed the EO seed; pipeline mirrors `build-image`:
   compile glue → `ORL.Link` inner core → compile cross-compiler → compile user
   modules → `ORL.Link`/`ORL.Load` boot file → `VDiskUtil.InstallFiles`.
4. **Test.** EO golden round-trip (extract → build → identical), mirroring the
   PO2013 `build_image_round_trips_the_golden`.

## Decisions

- **Bootstrap: (b) scripted headless driver** — chosen. It enables autonomous
  compile-test iteration on the glue and is the same control plane the on-EO agent
  will need. Building it is the next step after the glue draft.
