# Vendored toolchain assets

These files are vendored from Peter De Wachter's
[project-norebo](https://github.com/pdewacht/project-norebo) and embedded into the
`build-image` binary (via `include_bytes!`) so it can build a Project Oberon disk
image without an external checkout:

- `Norebo/` — the Norebo host-side Oberon modules (`Kernel`, `Files`, `Oberon`,
  `FileDir`, `CoreLinker`, `VDisk`, `VFileDir`, `VFiles`, `VDiskUtil`, `Norebo`),
  which talk to the Norebo syscalls instead of FPGA hardware.
- `Bootstrap/` — prebuilt `.rsc` objects and the `InnerCore` image that seed the
  first compile (you need a working Oberon to compile Oberon).

They derive from Project Oberon 2013; the upstream Wirth sources proper are *not*
vendored — they are fetched separately into the `sources_dir` passed to `build-image`.

This material is under the **ISC license**, the same license as this repository,
but the copyright is held upstream. The Norebo-specific modules and build tooling
are © Peter De Wachter (project-norebo); they derive from Project Oberon 2013, whose
notice — reproduced here as the license requires — is:

```
Project Oberon, Revised Edition 2013

Book copyright (C)2013 Niklaus Wirth and Juerg Gutknecht;
software copyright (C)2013 Niklaus Wirth (NW), Juerg Gutknecht (JG), Paul
Reed (PR/PDR).

Permission to use, copy, modify, and/or distribute this software and its
accompanying documentation (the "Software") for any purpose with or
without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
WITH REGARD TO THE SOFTWARE, INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY, FITNESS AND NONINFRINGEMENT.  IN NO EVENT SHALL THE
AUTHORS BE LIABLE FOR ANY CLAIM, SPECIAL, DIRECT, INDIRECT, OR
CONSEQUENTIAL DAMAGES OR ANY DAMAGES OR LIABILITY WHATSOEVER, WHETHER IN
AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE DEALINGS IN OR USE OR PERFORMANCE OF THE SOFTWARE.
```
