//! Live differential tests against the C reference, which `build.rs` compiles
//! and links under the `cosim` feature. Run with:
//!
//!   cargo test --features cosim
//!
//! Needs a C toolchain and the sibling C repo (path via `OBERON_C_SRC`); the
//! boot lockstep additionally needs `OBERON_DISK`. These prove Rust == C across
//! the whole instruction space and a full boot, beyond the frozen vectors and
//! golden hashes.
#![cfg(feature = "cosim")]

use risc_core::cosim::{self, CRisc};
use risc_core::disk::Disk;
use risc_core::fp;
use risc_core::risc::Risc;

const CPU_HZ: u32 = 25_000_000;
const FPS: u32 = 60;

/// Deterministic xorshift64* RNG (reproducible; no proptest dependency).
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
}

fn iters(var: &str, default: u32) -> u32 {
    std::env::var(var)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// The one intentional divergence (see DIVERGENCES.md / RISC5.v:139): a `MOV`
/// with q=0, u=1, v=1 reads the flags byte, where our port emits the hardware's
/// 0x50 and the C reference emits 0xD0. It cannot be oracled against C, so the
/// differential tests steer around it (`mov_flags_read_is_hardware_0x50` pins the
/// actual value).
fn is_mov_flags_read(ir: u32) -> bool {
    ir & 0x8000_0000 == 0        // register class
        && (ir >> 16) & 0xF == 0 // MOV
        && ir & 0x4000_0000 == 0 // q = 0
        && ir & 0x2000_0000 != 0 // u = 1
        && ir & 0x1000_0000 != 0 // v = 1
}

/// Branch class (register or immediate): top two opcode bits set.
fn is_branch(ir: u32) -> bool {
    ir & 0xC000_0000 == 0xC000_0000
}

/// Layer 1: software FP / idiv, live against C over the random `u32` space.
#[test]
fn fp_matches_c_live() {
    let mut rng = Rng::new(0x9E37_79B9_7F4A_7C15);
    let n = iters("COSIM_FP_ITERS", 1_000_000);
    for _ in 0..n {
        let x = rng.u32();
        let y = rng.u32();
        for &(u, v) in &[(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(
                fp::fp_add(x, y, u, v),
                cosim::fp_add(x, y, u, v),
                "fp_add({x:08X},{y:08X},{},{})",
                u as u8,
                v as u8
            );
        }
        assert_eq!(
            fp::fp_mul(x, y),
            cosim::fp_mul(x, y),
            "fp_mul({x:08X},{y:08X})"
        );
        assert_eq!(
            fp::fp_div(x, y),
            cosim::fp_div(x, y),
            "fp_div({x:08X},{y:08X})"
        );
        for &s in &[false, true] {
            let r = fp::idiv(x, y, s);
            assert_eq!(
                (r.quot, r.rem),
                cosim::idiv(x, y, s),
                "idiv({x:08X},{y:08X},{})",
                s as u8
            );
        }
    }
    eprintln!("FP live differential: {n} iterations matched C");
}

/// Layer 2: one random instruction over random architectural state, stepped
/// once in both, comparing the full CPU state + a small RAM window. This covers
/// the whole decode/ALU/shifter/flag/branch space, including paths the boot
/// never reaches.
#[test]
fn single_instruction_matches_c() {
    const WINDOW: usize = 8; // RAM words seeded + compared
    let mut c = CRisc::new();
    let mut rs = Risc::new();
    let mut rng = Rng::new(0x1234_5678_9ABC_DEF0);
    let n = iters("COSIM_INSN_ITERS", 5_000_000);
    let mut skipped = 0u32;

    for _ in 0..n {
        let ir = rng.u32();
        // Steer around the one expected divergence (see `is_mov_flags_read`).
        if is_mov_flags_read(ir) {
            skipped += 1;
            continue;
        }
        // PC = 0 so the fetch reads the instruction we plant at RAM word 0.
        let mut st = [0u32; 19];
        for s in st.iter_mut().skip(1).take(17) {
            *s = rng.u32(); // R0..R15, H
        }
        st[18] = rng.u32() & 0xF; // flags
        let mut ram = [0u32; WINDOW];
        ram[0] = ir;
        for w in ram.iter_mut().skip(1) {
            *w = rng.u32();
        }

        for (i, &val) in ram.iter().enumerate() {
            c.ram_write(i, val);
            rs.cosim_ram_write(i, val);
        }
        c.set_state(&st);
        rs.cosim_set_state(&st);
        c.single_step();
        rs.cosim_step();

        assert_eq!(
            rs.cosim_dump_state(),
            c.dump_state(),
            "instruction {ir:08X}, state in {st:?}"
        );
        for i in 0..WINDOW {
            assert_eq!(
                rs.cosim_ram_read(i),
                c.ram_read(i),
                "RAM[{i}] after {ir:08X}"
            );
        }
    }
    eprintln!(
        "single-instruction differential: {} iterations matched C \
         ({skipped} MOV-flags-read skipped as the one expected divergence)",
        n - skipped
    );
}

/// Layer 2b: random *multi-step* lockstep. Plant a region of random non-branch
/// instructions, set random state, and run the stream — fetching and executing
/// in sequence — comparing the full CPU state every step (and the region at the
/// end). This reaches what the single-instruction sampler can't: instruction
/// *streams* the boot never emits — back-to-back fetch/PC progression,
/// store-then-load memory chains, and values/flags flowing between ops. Branches
/// are left to layer 3 (real control flow) and the single-step sampler (their PC
/// math in isolation); excluding them keeps PC marching through the planted
/// region, and we stop the moment a self-modified word sends PC out of it, so we
/// never fetch from the void or touch peripherals.
#[test]
fn burst_lockstep_matches_c() {
    const REGION: usize = 64; // planted code words; also the per-burst step cap
    let mut c = CRisc::new();
    let mut rs = Risc::new();
    let n = iters("COSIM_BURST_ITERS", 200_000);

    for burst in 0..n {
        // Per-burst seed: a failure is replayable from just the burst index.
        let mut rng = Rng::new(0xB00B_5EED_0000_0000 ^ u64::from(burst));

        let mut code = [0u32; REGION];
        for w in &mut code {
            // Random, but no branch (would leave the sandbox) and no flags-read.
            *w = loop {
                let ir = rng.u32();
                if !is_branch(ir) && !is_mov_flags_read(ir) {
                    break ir;
                }
            };
        }
        let mut st = [0u32; 19]; // PC = 0: start at the region origin
        for s in st.iter_mut().skip(1).take(17) {
            *s = rng.u32(); // R0..R15, H
        }
        st[18] = rng.u32() & 0xF; // flags

        for (i, &val) in code.iter().enumerate() {
            c.ram_write(i, val);
            rs.cosim_ram_write(i, val);
        }
        c.set_state(&st);
        rs.cosim_set_state(&st);

        let mut pc = 0usize; // invariant: pc < REGION at the top of each step
        for step in 0..REGION {
            let insn = rs.cosim_ram_read(pc);
            assert_eq!(
                c.ram_read(pc),
                insn,
                "burst {burst} step {step}: RAM[{pc}] (next insn) diverged"
            );
            // A store may have written a flags-read into the path; neutralise it
            // in place on both sides before it executes.
            if is_mov_flags_read(insn) {
                let patched = insn & !0x1000_0000; // clear v
                c.ram_write(pc, patched);
                rs.cosim_ram_write(pc, patched);
            }

            c.single_step();
            rs.cosim_step();

            let state = rs.cosim_dump_state();
            assert_eq!(
                state,
                c.dump_state(),
                "burst {burst} step {step}: CPU state diverged"
            );

            let next = state[0] as usize;
            if next >= REGION {
                break; // a self-modified branch left the sandbox — stop here
            }
            pc = next;
        }

        // Catch store-value divergences that never flowed back into a register.
        for i in 0..REGION {
            assert_eq!(
                rs.cosim_ram_read(i),
                c.ram_read(i),
                "burst {burst}: RAM[{i}] diverged"
            );
        }
    }
    eprintln!("burst lockstep: {n} random bursts (up to {REGION} steps) matched C");
}

/// Layer 3: full-boot lockstep. Same deterministic schedule on both; assert the
/// entire CPU state and framebuffer match C at every frame.
#[test]
fn boot_lockstep_matches_c() {
    let Ok(src) = std::env::var("OBERON_DISK") else {
        eprintln!("OBERON_DISK not set; skipping cosim boot lockstep");
        return;
    };
    // Each side writes to its own fresh copy.
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let c_disk = dir.join(format!("oberon_cosim_c_{pid}.dsk"));
    let rs_disk = dir.join(format!("oberon_cosim_rs_{pid}.dsk"));
    std::fs::copy(&src, &c_disk).unwrap();
    std::fs::copy(&src, &rs_disk).unwrap();

    let mut c = CRisc::new();
    c.attach_disk(&c_disk);
    let mut rs = Risc::new();
    rs.set_spi(1, Box::new(Disk::new(Some(&rs_disk)).unwrap()));

    let frame_ms = 1000 / FPS;
    for frame in 0..600u32 {
        let ms = frame.wrapping_mul(frame_ms);
        c.set_time(ms);
        c.run(CPU_HZ / FPS);
        rs.set_time(ms);
        rs.run(CPU_HZ / FPS);

        assert_eq!(
            rs.cosim_dump_state(),
            c.dump_state(),
            "CPU state diverged at frame {frame}"
        );
        let cfb = c.framebuffer();
        assert!(
            rs.framebuffer()[..cfb.len()] == *cfb,
            "framebuffer diverged at frame {frame}"
        );
    }

    let _ = std::fs::remove_file(&c_disk);
    let _ = std::fs::remove_file(&rs_disk);
    eprintln!("boot lockstep: 600 frames matched C exactly");
}
