# Divergences from the C reference

This port is otherwise bit-exact to Peter De Wachter's C `oberon-risc-emu`,
proven by the frozen FP vectors, the C-derived boot golden, and the live
co-simulation net (FP, single-instruction, and full-boot lockstep, all
zero-divergence). It deliberately differs in exactly one place.

## `MOV` flags read — `0x50` (hardware) vs `0xD0` (C)

Reading the CPU flags via `MOV` with `q=0, u=1, v=1` returns the four status
flags `N Z C V` in the top nibble, over a fixed low byte:

| Source | Low byte | Result |
| --- | --- | --- |
| RISC5 hardware (`RISC5.v:139`) | `0x50` | `0x50 \| (N<<31) \| (Z<<30) \| (C<<29) \| (V<<28)` |
| C `oberon-risc-emu` (`risc.c`) | `0xD0` | `0xD0 \| …` |

The low byte is a CPU-id / version field. The FPGA emits `0x50`; the C emulator
deliberately uses `0xD0` (its source comments it `// ???`). **This port follows
the hardware and emits `0x50`.**

This is safe and inert for booting Project Oberon: the boot path never reads
this byte, so the boot golden and the full-boot cosim lockstep stay green
unchanged. It is guarded by the unit test
`risc::tests::mov_flags_read_is_hardware_0x50` (the differential fuzzer cannot
oracle this byte against C, so it is treated as one expected divergence in
`tests/cosim.rs::single_instruction_matches_c`).
