# Divergences from the C reference

This port is otherwise bit-exact to Peter De Wachter's C `oberon-risc-emu`,
proven by the frozen FP vectors, the C-derived boot golden, and the live
co-simulation net (FP, single-instruction, and full-boot lockstep, all
zero-divergence). It deliberately differs in exactly one place.

## `MOV` flags read — `0x53` (hardware) vs `0xD0` (C)

Reading the CPU flags via `MOV` with `q=0, u=1, v=1` returns the four status
flags `N Z C V` in the top nibble, over a fixed low byte:

| Source | Low byte | Result |
| --- | --- | --- |
| RISC5 hardware (`RISC5.v`, `{N, Z, C, OV, 20'b0, 8'h53}`) | `0x53` | `0x53 \| (N<<31) \| (Z<<30) \| (C<<29) \| (V<<28)` |
| C `oberon-risc-emu` (`risc.c`) | `0xD0` | `0xD0 \| …` |

The low byte is a CPU-id / version field. The FPGA emits `0x53`; the C emulator
deliberately uses `0xD0` (its source comments it `// ???`). **This port follows
the hardware and emits `0x53`.**

This is safe and inert for booting Project Oberon: the boot path never reads
this byte, so the boot golden and the full-boot cosim lockstep stay green
unchanged. It is guarded by the unit test
`risc::tests::mov_flags_read_is_hardware_0x53` (the differential fuzzer cannot
oracle this byte against C, so the single-instruction and burst layers in
`tests/cosim.rs` steer around it as the one expected divergence).

## Inherited C-reference quirks (shared with C, not divergences from it)

The contract above is C-exactness, and the differential net oracles everything
it can reach — including corners where the C reference itself is known to
differ from the FPGA (`RISC5.v`). Those corners are inherited deliberately:
matching the hardware there would put this port at odds with its own oracle.
The working rule: follow the C wherever the cosim can oracle it; follow the
hardware only where it cannot (the `0x50` ID byte above).

Known inherited differences from the hardware:

- **`ADD'`/`SUB'` carry flag with carry-in.** Both emulators derive the C flag
  by comparing the result against the first operand (`a < b` after an add,
  `a > b` after a subtract). That is exact for plain `ADD`/`SUB`, but with the
  `u` modifier (add/subtract the incoming carry) it misses exactly one case: a
  second operand of `0xFFFFFFFF` with the incoming flag set wraps the result
  back to the first operand, where the hardware's adder reports a carry
  (borrow) out of 1 and the comparison reads 0. The V flag's formula is exact
  for the full three-input sum, so only C is affected. Unreachable from
  compiled code: Oberon-07 has no carry-chain arithmetic, so the compiler
  never emits `ADD'`/`SUB'`.
- **Full 32-bit addressing.** The FPGA's 20-bit address bus ignores the top
  address bits, so an out-of-range access aliases into the 1 MB RAM; both
  emulators decode all 32 bits (which is what lets `--mem` offer more RAM) and
  treat a fetch from unmapped space as a reset. Identical for well-behaved
  software in the default configuration — the memory-layout note at the top of
  `risc.rs`.

## Scope: guest-visible behavior

The proofs (and this document) cover the guest-visible machine: CPU state,
memory, and what the devices answer on their MMIO ports. *Host-side* behavior
of the emulator-only devices is not part of the contract, and is allowed to be
better than the C where the guest can't tell: the clipboard bridge decodes
Oberon's Latin-1 to proper text for the host clipboard (`sdl-clipboard.c`
hands SDL the raw bytes), and `PCLink` reads the host file through a buffer
rather than a byte per `read(2)`. The bytes on the guest's wire are identical
in both cases.
