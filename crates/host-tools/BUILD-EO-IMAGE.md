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

In progress: the **headless driver** (`eo-driver` bin). Milestone 1 done + verified
— it boots EO's `RISC.img` on `risc-core` with no GUI, captures the serial line,
and dumps the framebuffer; EO `AP 1.1.26` reaches its desktop headless in ~86 ms,
confirming `risc-core` needs no changes.

Not started: the rest of the glue (`Files`/`FileDir`/`Oberon` stub), command
injection in `eo-driver` (M2), the ORL link step driven through it, and the
`build-eo-image` bin itself.

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

## Glue (`assets/eo-norebo/`)

Mirrors `assets/Norebo/` but for EO. `Kernel`/`Disk` are drafted from EO's *real*
sources (so types + export interface match exactly), patched only at the
hardware/headless boundary. Nothing is compile-tested yet — that needs the driver.

| Module | Plan | Status |
| --- | --- | --- |
| `Norebo` | syscall wrapper, EO-independent | reuse `assets/Norebo/Norebo.Mod` |
| `Kernel` | EO `Kernel` + Trap→`Norebo.Trap` (2-line patch); GC/heap/Init unchanged | **drafted** |
| `Disk` | EO `Disk` minus SD/SPI; sector-map allocator kept; `Get/PutSector` abort | **drafted** |
| `Files` / `FileDir` | host-backed (Norebo syscalls), matched to EO's expanded interface | next |
| `Oberon` (stub) | `Par`/`Log`/`GetSelection`/`Call` only — what ORP/ORL use | next |

`Files`, `FileDir`, and the `Oberon` stub are deferred to the driver-enabled loop,
where the compiler's own errors guide the interface reconciliation.

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
