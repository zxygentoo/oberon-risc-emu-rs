//! The RISC5 CPU core, memory map, and public API (port of `risc.c`).
//!
//! The memory layout differs slightly from the FPGA: the FPGA uses a 20-bit
//! address bus and ignores the top 12 bits, while we use all 32 bits so the
//! emulator can offer more than 1 MB of RAM. In the default configuration the
//! emulator is bit-compatible with the FPGA system.

use crate::boot_rom::BOOTLOADER;
use crate::fp::{fp_add, fp_div, fp_mul, idiv};
use crate::io::{Clipboard, Led, Serial, Spi};

/// Standard framebuffer width in pixels (overridable via [`Risc::configure_memory`]).
pub const FRAMEBUFFER_WIDTH: usize = 1024;
/// Standard framebuffer height in pixels.
pub const FRAMEBUFFER_HEIGHT: usize = 768;

const DEFAULT_MEM_SIZE: u32 = 0x0010_0000;
const DEFAULT_DISPLAY_START: u32 = 0x000E_7F00;

const ROM_START: u32 = 0xFFFF_F800;
const ROM_WORDS: usize = 512;
const IO_START: u32 = 0xFFFF_FFC0;

/// A register-instruction opcode: the 4-bit `op` field (the C's anonymous enum).
/// Discriminants are the opcode values, so dispatch is exhaustive and the test
/// encoder can use the variants directly.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
enum Op {
    Mov = 0,
    Lsl,
    Asr,
    Ror,
    And,
    Ann,
    Ior,
    Xor,
    Add,
    Sub,
    Mul,
    Div,
    Fad,
    Fsb,
    Fml,
    Fdv,
}

impl Op {
    /// Decode the 4-bit opcode field. The argument is `(ir >> 16) & 0xF`, so all
    /// 16 values map to a variant.
    fn from_u4(v: u32) -> Op {
        match v {
            0 => Op::Mov,
            1 => Op::Lsl,
            2 => Op::Asr,
            3 => Op::Ror,
            4 => Op::And,
            5 => Op::Ann,
            6 => Op::Ior,
            7 => Op::Xor,
            8 => Op::Add,
            9 => Op::Sub,
            10 => Op::Mul,
            11 => Op::Div,
            12 => Op::Fad,
            13 => Op::Fsb,
            14 => Op::Fml,
            _ => Op::Fdv,
        }
    }
}

/// A branch condition: the 3-bit `cc` field, under its ISA mnemonic. The branch
/// is taken when [`Cond::holds`], optionally inverted by the negate bit.
#[derive(Clone, Copy)]
enum Cond {
    Mi,   // negative
    Eq,   // zero
    Cs,   // carry set
    Vs,   // overflow
    Ls,   // lower or same (C | Z)
    Lt,   // less than (N != V)
    Le,   // less or equal ((N != V) | Z)
    True, // always
}

impl Cond {
    /// Decode the 3-bit condition field. The argument is `(ir >> 24) & 7`, so
    /// all 8 values map to a variant.
    fn from_u3(v: u32) -> Cond {
        match v {
            0 => Cond::Mi,
            1 => Cond::Eq,
            2 => Cond::Cs,
            3 => Cond::Vs,
            4 => Cond::Ls,
            5 => Cond::Lt,
            6 => Cond::Le,
            _ => Cond::True,
        }
    }

    /// Whether the (un-negated) condition holds for the given flags.
    fn holds(self, f: Flags) -> bool {
        let (n, z, c, v) = (
            f.contains(Flags::N),
            f.contains(Flags::Z),
            f.contains(Flags::C),
            f.contains(Flags::V),
        );
        match self {
            Cond::Mi => n,
            Cond::Eq => z,
            Cond::Cs => c,
            Cond::Vs => v,
            Cond::Ls => c | z,
            Cond::Lt => n ^ v,
            Cond::Le => (n ^ v) | z,
            Cond::True => true,
        }
    }
}

// Instruction-class selector bits in the top nibble of every instruction.
const PBIT: u32 = 0x8000_0000;
const QBIT: u32 = 0x4000_0000;
const UBIT: u32 = 0x2000_0000;
const VBIT: u32 = 0x1000_0000;

bitflags::bitflags! {
    /// The four ALU status flags. Bit positions match the `CpuState` / cosim
    /// packing (`Z | N<<1 | C<<2 | V<<3`), so state round-trips as `flags.bits()`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Flags: u8 {
        const Z = 1 << 0;
        const N = 1 << 1;
        const C = 1 << 2;
        const V = 1 << 3;
    }
}

/// A byte address in the 32-bit address space (a memory operand or IO register).
/// Kept distinct from [`WordAddr`] so the `/ 4` (word index) and `% 4` (byte
/// offset) conversions are explicit and unmixable at the load/store boundary.
#[derive(Clone, Copy)]
struct ByteAddr(u32);

/// A word (4-byte) index into RAM or ROM.
#[derive(Clone, Copy)]
struct WordAddr(u32);

impl ByteAddr {
    /// The index of the word containing this address (`/ 4`).
    fn word(self) -> WordAddr {
        WordAddr(self.0 / 4)
    }
    /// This address's byte offset within its word (`% 4`, in `0..4`).
    fn byte_in_word(self) -> u32 {
        self.0 % 4
    }
}

impl WordAddr {
    /// The byte address of this word's first byte (`* 4`).
    fn byte(self) -> ByteAddr {
        ByteAddr(self.0.wrapping_mul(4))
    }
    /// As a slice index into RAM or ROM.
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// A damaged (dirty) rectangle of the framebuffer, in framebuffer-word columns
/// and line rows. `y1 > y2` means "nothing damaged".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Damage {
    pub x1: i32,
    pub x2: i32,
    pub y1: i32,
    pub y2: i32,
}

/// A snapshot of the architectural CPU state, for inspection and differential
/// testing (mirrors the C cosim `dump_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuState {
    pub pc: u32,
    pub r: [u32; 16],
    pub h: u32,
    pub flags: Flags,
}

/// The RISC5 machine: CPU registers, RAM/ROM, and attached devices.
pub struct Risc {
    pc: u32,
    r: [u32; 16],
    h: u32,
    flags: Flags,

    mem_size: u32,
    display_start: u32,

    progress: u32,
    current_tick: u32,
    mouse: u32,
    key_buf: [u8; 16],
    key_cnt: u32,
    switches: u32,

    leds: Option<Box<dyn Led>>,
    serial: Option<Box<dyn Serial>>,
    spi_selected: u32,
    spi: [Option<Box<dyn Spi>>; 4],
    clipboard: Option<Box<dyn Clipboard>>,

    fb_width: i32,  // words
    fb_height: i32, // lines
    damage: Damage,

    ram: Vec<u32>,
    rom: [u32; ROM_WORDS],
}

impl Risc {
    /// Build a machine in the default (FPGA-compatible) configuration and reset
    /// it. Port of `risc_new`.
    pub fn new() -> Self {
        let fb_width = (FRAMEBUFFER_WIDTH / 32) as i32;
        let fb_height = FRAMEBUFFER_HEIGHT as i32;
        let mut risc = Risc {
            pc: 0,
            r: [0; 16],
            h: 0,
            flags: Flags::empty(),
            mem_size: DEFAULT_MEM_SIZE,
            display_start: DEFAULT_DISPLAY_START,
            progress: 0,
            current_tick: 0,
            mouse: 0,
            key_buf: [0; 16],
            key_cnt: 0,
            switches: 0,
            leds: None,
            serial: None,
            spi_selected: 0,
            spi: [None, None, None, None],
            clipboard: None,
            fb_width,
            fb_height,
            damage: Damage {
                x1: 0,
                y1: 0,
                x2: fb_width - 1,
                y2: fb_height - 1,
            },
            ram: vec![0u32; (DEFAULT_MEM_SIZE / 4) as usize],
            rom: BOOTLOADER,
        };
        risc.reset();
        risc
    }

    /// Resize RAM and the framebuffer, patching the boot ROM accordingly. Port
    /// of `risc_configure_memory`.
    pub fn configure_memory(&mut self, megabytes_ram: i32, screen_width: i32, screen_height: i32) {
        let megs = megabytes_ram.clamp(1, 32) as u32;

        self.display_start = megs << 20;
        self.mem_size = self.display_start + (screen_width * screen_height / 8) as u32;
        self.fb_width = screen_width / 32;
        self.fb_height = screen_height;
        self.damage = Damage {
            x1: 0,
            y1: 0,
            x2: self.fb_width - 1,
            y2: self.fb_height - 1,
        };

        self.ram = vec![0u32; (self.mem_size / 4) as usize];

        // Patch the new constants into the bootloader.
        let mem_lim = self.display_start - 16;
        self.rom[372] = 0x6100_0000 + (mem_lim >> 16);
        self.rom[373] = 0x4116_0000 + (mem_lim & 0x0000_FFFF);
        let stack_org = self.display_start / 2;
        self.rom[376] = 0x6100_0000 + (stack_org >> 16);

        // Inform the display driver of the framebuffer layout. This isn't very
        // pretty, but this way our disk images still boot on the standard FPGA.
        let d = (DEFAULT_DISPLAY_START / 4) as usize;
        self.ram[d] = 0x5369_7A67;
        self.ram[d + 1] = screen_width as u32;
        self.ram[d + 2] = screen_height as u32;
        self.ram[d + 3] = self.display_start;

        self.reset();
    }

    /// Attach the LED logging device. Port of `risc_set_leds`.
    pub fn set_leds(&mut self, leds: Box<dyn Led>) {
        self.leds = Some(leds);
    }

    /// Attach the serial device. Port of `risc_set_serial`.
    pub fn set_serial(&mut self, serial: Box<dyn Serial>) {
        self.serial = Some(serial);
    }

    /// Attach an SPI slave at index 1 or 2 (others ignored). Port of `risc_set_spi`.
    pub fn set_spi(&mut self, index: usize, spi: Box<dyn Spi>) {
        if index == 1 || index == 2 {
            self.spi[index] = Some(spi);
        }
    }

    /// Attach the clipboard bridge. Port of `risc_set_clipboard`.
    pub fn set_clipboard(&mut self, clipboard: Box<dyn Clipboard>) {
        self.clipboard = Some(clipboard);
    }

    /// Set the switch register (`--boot-from-serial` sets bit 0). Port of `risc_set_switches`.
    pub fn set_switches(&mut self, switches: u32) {
        self.switches = switches;
    }

    /// Reset: jump to the boot ROM. Port of `risc_reset`.
    pub fn reset(&mut self) {
        self.pc = ROM_START / 4;
    }

    /// Run up to `cycles` instructions, stopping early when the CPU is detected
    /// idle-spinning on the ms-counter or keyboard-ready bit. Port of `risc_run`.
    pub fn run(&mut self, cycles: u32) {
        self.progress = 20;
        // `progress` lets us pause emulation until the next frame when the CPU
        // is busy-waiting on the millisecond counter or keyboard ready bit.
        let mut i = 0;
        while i < cycles && self.progress != 0 {
            self.single_step();
            i += 1;
        }
    }

    fn single_step(&mut self) {
        let ir: u32 = if self.pc < self.mem_size / 4 {
            self.ram[self.pc as usize]
        } else if self.pc >= ROM_START / 4 && self.pc < ROM_START / 4 + ROM_WORDS as u32 {
            self.rom[(self.pc - ROM_START / 4) as usize]
        } else {
            eprintln!(
                "Branched into the void (PC=0x{:08X}), resetting...",
                self.pc
            );
            self.reset();
            return;
        };
        self.pc = self.pc.wrapping_add(1);

        if ir & PBIT == 0 {
            // Register instructions.
            let a = (ir & 0x0F00_0000) >> 24;
            let b = (ir & 0x00F0_0000) >> 20;
            let op = (ir & 0x000F_0000) >> 16;
            let im = ir & 0x0000_FFFF;
            let c = ir & 0x0000_000F;

            let b_val = self.r[b as usize];
            let c_val = if ir & QBIT == 0 {
                self.r[c as usize]
            } else if ir & VBIT == 0 {
                im
            } else {
                0xFFFF_0000 | im
            };

            let a_val: u32 = match Op::from_u4(op) {
                Op::Mov => {
                    if ir & UBIT == 0 {
                        c_val
                    } else if ir & QBIT != 0 {
                        c_val << 16
                    } else if ir & VBIT != 0 {
                        0xD0 // ???
                            | (u32::from(self.flags.contains(Flags::N)) << 31)
                            | (u32::from(self.flags.contains(Flags::Z)) << 30)
                            | (u32::from(self.flags.contains(Flags::C)) << 29)
                            | (u32::from(self.flags.contains(Flags::V)) << 28)
                    } else {
                        self.h
                    }
                }
                Op::Lsl => b_val.wrapping_shl(c_val & 31),
                Op::Asr => ((b_val as i32) >> (c_val & 31)) as u32,
                Op::Ror => b_val.rotate_right(c_val & 31),
                Op::And => b_val & c_val,
                Op::Ann => b_val & !c_val,
                Op::Ior => b_val | c_val,
                Op::Xor => b_val ^ c_val,
                Op::Add => {
                    let mut a_val = b_val.wrapping_add(c_val);
                    if ir & UBIT != 0 {
                        a_val = a_val.wrapping_add(u32::from(self.flags.contains(Flags::C)));
                    }
                    self.flags.set(Flags::C, a_val < b_val);
                    self.flags
                        .set(Flags::V, (((a_val ^ c_val) & (a_val ^ b_val)) >> 31) != 0);
                    a_val
                }
                Op::Sub => {
                    let mut a_val = b_val.wrapping_sub(c_val);
                    if ir & UBIT != 0 {
                        a_val = a_val.wrapping_sub(u32::from(self.flags.contains(Flags::C)));
                    }
                    self.flags.set(Flags::C, a_val > b_val);
                    self.flags
                        .set(Flags::V, (((b_val ^ c_val) & (a_val ^ b_val)) >> 31) != 0);
                    a_val
                }
                Op::Mul => {
                    let tmp: u64 = if ir & UBIT == 0 {
                        ((b_val as i32 as i64) * (c_val as i32 as i64)) as u64
                    } else {
                        (b_val as u64) * (c_val as u64)
                    };
                    self.h = (tmp >> 32) as u32;
                    tmp as u32
                }
                Op::Div => {
                    if (c_val as i32) > 0 {
                        if ir & UBIT == 0 {
                            let mut a_val = ((b_val as i32) / (c_val as i32)) as u32;
                            self.h = ((b_val as i32) % (c_val as i32)) as u32;
                            if (self.h as i32) < 0 {
                                a_val = a_val.wrapping_sub(1);
                                self.h = self.h.wrapping_add(c_val);
                            }
                            a_val
                        } else {
                            self.h = b_val % c_val;
                            b_val / c_val
                        }
                    } else {
                        let q = idiv(b_val, c_val, ir & UBIT != 0);
                        self.h = q.rem;
                        q.quot
                    }
                }
                Op::Fad => fp_add(b_val, c_val, ir & UBIT != 0, ir & VBIT != 0),
                Op::Fsb => fp_add(b_val, c_val ^ 0x8000_0000, ir & UBIT != 0, ir & VBIT != 0),
                Op::Fml => fp_mul(b_val, c_val),
                Op::Fdv => fp_div(b_val, c_val),
            };
            self.set_register(a, a_val);
        } else if ir & QBIT == 0 {
            // Memory instructions.
            let a = (ir & 0x0F00_0000) >> 24;
            let b = (ir & 0x00F0_0000) >> 20;
            let off = (ir & 0x000F_FFFF) as i32;
            let off = (off ^ 0x0008_0000) - 0x0008_0000; // sign-extend 20-bit

            let address = ByteAddr(self.r[b as usize].wrapping_add(off as u32));
            if ir & UBIT == 0 {
                let a_val = if ir & VBIT == 0 {
                    self.load_word(address)
                } else {
                    self.load_byte(address) as u32
                };
                self.set_register(a, a_val);
            } else if ir & VBIT == 0 {
                self.store_word(address, self.r[a as usize]);
            } else {
                self.store_byte(address, self.r[a as usize] as u8);
            }
        } else {
            // Branch instructions. Bit 27 negates the condition.
            let negate = ((ir >> 27) & 1) != 0;
            let t = negate ^ Cond::from_u3((ir >> 24) & 7).holds(self.flags);
            if t {
                if ir & VBIT != 0 {
                    // The link register holds the return point as a byte address.
                    self.set_register(15, WordAddr(self.pc).byte().0);
                }
                if ir & UBIT == 0 {
                    // Register-indirect: the register holds a byte address.
                    let c = ir & 0x0000_000F;
                    self.pc = ByteAddr(self.r[c as usize]).word().0;
                } else {
                    let off = (ir & 0x00FF_FFFF) as i32;
                    let off = (off ^ 0x0080_0000) - 0x0080_0000; // sign-extend 24-bit
                    self.pc = self.pc.wrapping_add(off as u32);
                }
            }
        }
    }

    fn set_register(&mut self, reg: u32, value: u32) {
        self.r[reg as usize] = value;
        self.flags.set(Flags::Z, value == 0);
        self.flags.set(Flags::N, (value as i32) < 0);
    }

    fn load_word(&mut self, addr: ByteAddr) -> u32 {
        if addr.0 < self.mem_size {
            self.ram[addr.word().index()]
        } else {
            self.load_io(addr.0)
        }
    }

    fn load_byte(&mut self, addr: ByteAddr) -> u8 {
        let w = self.load_word(addr);
        (w >> (addr.byte_in_word() * 8)) as u8
    }

    fn update_damage(&mut self, w: i32) {
        let row = w / self.fb_width;
        let col = w % self.fb_width;
        if row < self.fb_height {
            if col < self.damage.x1 {
                self.damage.x1 = col;
            }
            if col > self.damage.x2 {
                self.damage.x2 = col;
            }
            if row < self.damage.y1 {
                self.damage.y1 = row;
            }
            if row > self.damage.y2 {
                self.damage.y2 = row;
            }
        }
    }

    fn store_word(&mut self, addr: ByteAddr, value: u32) {
        if addr.0 < self.display_start {
            self.ram[addr.word().index()] = value;
        } else if addr.0 < self.mem_size {
            self.ram[addr.word().index()] = value;
            let fb_word0 = ByteAddr(self.display_start).word();
            self.update_damage((addr.word().0 - fb_word0.0) as i32);
        } else {
            self.store_io(addr.0, value);
        }
    }

    fn store_byte(&mut self, addr: ByteAddr, value: u8) {
        if addr.0 < self.mem_size {
            let mut w = self.load_word(addr);
            let shift = addr.byte_in_word() * 8;
            w &= !(0xFFu32 << shift);
            w |= (value as u32) << shift;
            self.store_word(addr, w);
        } else {
            self.store_io(addr.0, value as u32);
        }
    }

    // Keep each offset's logic in its own arm, mirroring the C's switch.
    #[allow(clippy::collapsible_match)]
    fn load_io(&mut self, address: u32) -> u32 {
        match address.wrapping_sub(IO_START) {
            0 => {
                // Millisecond counter.
                self.progress = self.progress.wrapping_sub(1);
                self.current_tick
            }
            4 => self.switches,
            8 => self.serial.as_mut().map_or(0, |s| s.read_data()),
            12 => self.serial.as_mut().map_or(0, |s| s.read_status()),
            16 => {
                let sel = self.spi_selected as usize;
                self.spi[sel].as_mut().map_or(255, |s| s.read_data())
            }
            20 => {
                // SPI status: bit 0 = rx ready.
                1
            }
            24 => {
                // Mouse input / keyboard status.
                let mut mouse = self.mouse;
                if self.key_cnt > 0 {
                    mouse |= 0x1000_0000;
                } else {
                    self.progress = self.progress.wrapping_sub(1);
                }
                mouse
            }
            28 => {
                // Keyboard input.
                if self.key_cnt > 0 {
                    let scancode = self.key_buf[0];
                    self.key_cnt -= 1;
                    self.key_buf.copy_within(1..=(self.key_cnt as usize), 0);
                    scancode as u32
                } else {
                    0
                }
            }
            40 => self.clipboard.as_mut().map_or(0, |c| c.read_control()),
            44 => self.clipboard.as_mut().map_or(0, |c| c.read_data()),
            _ => 0,
        }
    }

    fn store_io(&mut self, address: u32, value: u32) {
        match address.wrapping_sub(IO_START) {
            4 => {
                // LED control.
                if let Some(l) = self.leds.as_mut() {
                    l.write(value);
                }
            }
            8 => {
                if let Some(s) = self.serial.as_mut() {
                    s.write_data(value);
                }
            }
            16 => {
                let sel = self.spi_selected as usize;
                if let Some(s) = self.spi[sel].as_mut() {
                    s.write_data(value);
                }
            }
            20 => {
                // SPI control: bits 0-1 slave select, bit 2 fast, bit 3 net enable.
                self.spi_selected = value & 3;
            }
            40 => {
                if let Some(c) = self.clipboard.as_mut() {
                    c.write_control(value);
                }
            }
            44 => {
                if let Some(c) = self.clipboard.as_mut() {
                    c.write_data(value);
                }
            }
            _ => {}
        }
    }

    /// Set the synthetic millisecond clock. Port of `risc_set_time`.
    pub fn set_time(&mut self, tick: u32) {
        self.current_tick = tick;
    }

    /// Report a mouse move (coordinates in the Oberon frame). Port of `risc_mouse_moved`.
    pub fn mouse_moved(&mut self, mouse_x: i32, mouse_y: i32) {
        if (0..4096).contains(&mouse_x) {
            self.mouse = (self.mouse & !0x0000_0FFF) | mouse_x as u32;
        }
        if (0..4096).contains(&mouse_y) {
            self.mouse = (self.mouse & !0x00FF_F000) | ((mouse_y as u32) << 12);
        }
    }

    /// Report a mouse button (1=left, 2=middle, 3=right). Port of `risc_mouse_button`.
    pub fn mouse_button(&mut self, button: i32, down: bool) {
        if (1..4).contains(&button) {
            let bit = 1u32 << (27 - button);
            if down {
                self.mouse |= bit;
            } else {
                self.mouse &= !bit;
            }
        }
    }

    /// Enqueue PS/2 scancodes for the keyboard (dropped if the buffer is full).
    /// Port of `risc_keyboard_input`.
    pub fn keyboard_input(&mut self, scancodes: &[u8]) {
        let len = scancodes.len();
        if self.key_buf.len() - self.key_cnt as usize >= len {
            let start = self.key_cnt as usize;
            self.key_buf[start..start + len].copy_from_slice(scancodes);
            self.key_cnt += len as u32;
        }
    }

    /// The framebuffer words, starting at `display_start`. Port of `risc_get_framebuffer_ptr`.
    pub fn framebuffer(&self) -> &[u32] {
        &self.ram[ByteAddr(self.display_start).word().index()..]
    }

    /// Take the accumulated damage rectangle and reset it to empty. Port of
    /// `risc_get_framebuffer_damage`.
    pub fn framebuffer_damage(&mut self) -> Damage {
        let dmg = self.damage;
        self.damage = Damage {
            x1: self.fb_width,
            x2: 0,
            y1: self.fb_height,
            y2: 0,
        };
        dmg
    }

    /// Framebuffer width in 32-pixel words.
    pub fn fb_width(&self) -> i32 {
        self.fb_width
    }

    /// Framebuffer height in lines.
    pub fn fb_height(&self) -> i32 {
        self.fb_height
    }

    /// Snapshot the architectural CPU state (for inspection / differential testing).
    pub fn cpu_state(&self) -> CpuState {
        CpuState {
            pc: self.pc,
            r: self.r,
            h: self.h,
            flags: self.flags,
        }
    }
}

impl Default for Risc {
    fn default() -> Self {
        Self::new()
    }
}

/// State injection/extraction + raw stepping for differential testing against
/// the C reference. Only present under the `cosim` feature so the normal public
/// API stays clean.
#[cfg(feature = "cosim")]
impl Risc {
    /// State vector: `[PC, R0..R15, H, flags]`, flags = `Z|N<<1|C<<2|V<<3`.
    pub fn cosim_set_state(&mut self, st: &[u32; 19]) {
        self.pc = st[0];
        self.r.copy_from_slice(&st[1..17]);
        self.h = st[17];
        self.flags = Flags::from_bits_truncate(st[18] as u8);
    }

    pub fn cosim_dump_state(&self) -> [u32; 19] {
        let mut st = [0u32; 19];
        st[0] = self.pc;
        st[1..17].copy_from_slice(&self.r);
        st[17] = self.h;
        st[18] = u32::from(self.flags.bits());
        st
    }

    pub fn cosim_ram_read(&self, word: usize) -> u32 {
        self.ram[word]
    }
    pub fn cosim_ram_write(&mut self, word: usize, value: u32) {
        self.ram[word] = value;
    }

    pub fn cosim_step(&mut self) {
        self.single_step();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    // The opcodes under their ISA mnemonics, for terse instruction encoding.
    use super::Op::{
        Add as ADD, And as AND, Ann as ANN, Asr as ASR, Div as DIV, Fsb as FSB, Ior as IOR,
        Lsl as LSL, Mov as MOV, Mul as MUL, Ror as ROR, Sub as SUB, Xor as XOR,
    };

    /// Encode a register-format instruction. `ci` is the c register index (when
    /// `q == 0`) or the 16-bit immediate (when `q == 1`).
    fn reg(q: u32, u: u32, v: u32, a: u32, b: u32, op: Op, ci: u32) -> u32 {
        (q << 30) | (u << 29) | (v << 28) | (a << 24) | (b << 20) | ((op as u32) << 16) | ci
    }
    /// Encode a memory-format instruction. `u`: 0 = load, 1 = store. `v`: 0 = word, 1 = byte.
    fn mem(u: u32, v: u32, a: u32, b: u32, off: u32) -> u32 {
        0x8000_0000 | (u << 29) | (v << 28) | (a << 24) | (b << 20) | (off & 0x000F_FFFF)
    }
    /// Encode an immediate (PC-relative) branch.
    fn br_imm(negate: u32, cond: u32, link: u32, off: u32) -> u32 {
        0xE000_0000 | (link << 28) | (negate << 27) | (cond << 24) | (off & 0x00FF_FFFF)
    }
    /// Encode a register (indirect) branch.
    fn br_reg(negate: u32, cond: u32, link: u32, c: u32) -> u32 {
        0xC000_0000 | (link << 28) | (negate << 27) | (cond << 24) | (c & 0xF)
    }

    /// Fresh machine executing from RAM word 0.
    fn cpu() -> Risc {
        let mut r = Risc::new();
        r.pc = 0;
        r
    }

    // Terse flag accessors for the assertions below.
    impl Risc {
        fn z(&self) -> bool {
            self.flags.contains(Flags::Z)
        }
        fn n(&self) -> bool {
            self.flags.contains(Flags::N)
        }
        fn c(&self) -> bool {
            self.flags.contains(Flags::C)
        }
        fn v(&self) -> bool {
            self.flags.contains(Flags::V)
        }
    }

    // ---- MOV ----

    #[test]
    fn mov_immediate() {
        let mut r = cpu();
        r.ram[0] = reg(1, 0, 0, 1, 0, MOV, 0x1234);
        r.single_step();
        assert_eq!(r.r[1], 0x1234);
        assert!(!r.z() && !r.n());
        assert_eq!(r.pc, 1);
    }

    #[test]
    fn mov_immediate_sign_extended() {
        let mut r = cpu();
        // q=1, v=1 -> c_val = 0xFFFF0000 | im.
        r.ram[0] = reg(1, 0, 1, 1, 0, MOV, 0x8000);
        r.single_step();
        assert_eq!(r.r[1], 0xFFFF_8000);
        assert!(r.n() && !r.z());
    }

    #[test]
    fn mov_high_shifts_left_16() {
        let mut r = cpu();
        r.ram[0] = reg(1, 1, 0, 1, 0, MOV, 0x1234);
        r.single_step();
        assert_eq!(r.r[1], 0x1234_0000);
    }

    #[test]
    fn mov_register() {
        let mut r = cpu();
        // MOV moves the c operand: R1 = R3 (the b field is unused).
        r.ram[0] = reg(0, 0, 0, 1, 0, MOV, 3);
        r.r[3] = 0xCAFE;
        r.single_step();
        assert_eq!(r.r[1], 0xCAFE);
    }

    #[test]
    fn mov_flags_quirk() {
        let mut r = cpu();
        // q=0, u=1, v=1 -> 0xD0 | NZCV.
        r.ram[0] = reg(0, 1, 1, 1, 0, MOV, 0);
        r.flags.insert(Flags::N);
        r.flags.insert(Flags::C);
        r.single_step();
        assert_eq!(r.r[1], 0xD0 | 0x8000_0000 | 0x2000_0000);
    }

    #[test]
    fn mov_h_register() {
        let mut r = cpu();
        // q=0, u=1, v=0 -> R = H.
        r.ram[0] = reg(0, 1, 0, 1, 0, MOV, 0);
        r.h = 0x9ABC_DEF0;
        r.single_step();
        assert_eq!(r.r[1], 0x9ABC_DEF0);
    }

    // ---- Shifts / logical ----

    #[test]
    fn lsl() {
        let mut r = cpu();
        r.ram[0] = reg(0, 0, 0, 1, 2, LSL, 3);
        r.r[2] = 0x1;
        r.r[3] = 4;
        r.single_step();
        assert_eq!(r.r[1], 0x10);
    }

    #[test]
    fn asr_sign_fill() {
        let mut r = cpu();
        r.ram[0] = reg(0, 0, 0, 1, 2, ASR, 3);
        r.r[2] = 0x8000_0000;
        r.r[3] = 4;
        r.single_step();
        assert_eq!(r.r[1], 0xF800_0000);
    }

    #[test]
    fn ror_wraps() {
        let mut r = cpu();
        r.ram[0] = reg(0, 0, 0, 1, 2, ROR, 3);
        r.r[2] = 0x0000_000F;
        r.r[3] = 4;
        r.single_step();
        assert_eq!(r.r[1], 0xF000_0000);
    }

    #[test]
    fn logical_ops() {
        for (op, want) in [
            (AND, 0x0F00 & 0x00F0),
            (ANN, 0x0F00 & !0x00F0),
            (IOR, 0x0F00 | 0x00F0),
            (XOR, 0x0F00 ^ 0x00F0),
        ] {
            let mut r = cpu();
            r.ram[0] = reg(0, 0, 0, 1, 2, op, 3);
            r.r[2] = 0x0F00;
            r.r[3] = 0x00F0;
            r.single_step();
            assert_eq!(r.r[1], want, "op {op:?}");
        }
    }

    // ---- ADD / SUB flags ----

    #[test]
    fn add_signed_overflow() {
        let mut r = cpu();
        r.ram[0] = reg(0, 0, 0, 1, 2, ADD, 3);
        r.r[2] = 0x7FFF_FFFF;
        r.r[3] = 1;
        r.single_step();
        assert_eq!(r.r[1], 0x8000_0000);
        assert!(r.v(), "signed overflow");
        assert!(!r.c(), "no unsigned carry");
        assert!(r.n() && !r.z());
    }

    #[test]
    fn add_unsigned_carry() {
        let mut r = cpu();
        r.ram[0] = reg(0, 0, 0, 1, 2, ADD, 3);
        r.r[2] = 0xFFFF_FFFF;
        r.r[3] = 1;
        r.single_step();
        assert_eq!(r.r[1], 0);
        assert!(r.c(), "unsigned carry");
        assert!(!r.v());
        assert!(r.z() && !r.n());
    }

    #[test]
    fn add_with_carry_in() {
        let mut r = cpu();
        r.ram[0] = reg(0, 1, 0, 1, 2, ADD, 3); // u=1 -> add carry
        r.r[2] = 1;
        r.r[3] = 1;
        r.flags.insert(Flags::C);
        r.single_step();
        assert_eq!(r.r[1], 3);
    }

    #[test]
    fn sub_signed_overflow() {
        let mut r = cpu();
        r.ram[0] = reg(0, 0, 0, 1, 2, SUB, 3);
        r.r[2] = 0x8000_0000;
        r.r[3] = 1;
        r.single_step();
        assert_eq!(r.r[1], 0x7FFF_FFFF);
        assert!(r.v(), "signed overflow");
        assert!(!r.c());
    }

    #[test]
    fn sub_borrow_sets_carry() {
        let mut r = cpu();
        r.ram[0] = reg(0, 0, 0, 1, 2, SUB, 3);
        r.r[2] = 0;
        r.r[3] = 1;
        r.single_step();
        assert_eq!(r.r[1], 0xFFFF_FFFF);
        assert!(r.c(), "borrow");
    }

    // ---- MUL / DIV ----

    #[test]
    fn mul_signed_high() {
        let mut r = cpu();
        r.ram[0] = reg(0, 0, 0, 1, 2, MUL, 3);
        r.r[2] = (-2i32) as u32;
        r.r[3] = 3;
        r.single_step();
        assert_eq!(r.r[1], (-6i32) as u32);
        assert_eq!(r.h, 0xFFFF_FFFF); // sign-extended high word
    }

    #[test]
    fn mul_unsigned_high() {
        let mut r = cpu();
        r.ram[0] = reg(0, 1, 0, 1, 2, MUL, 3); // u=1 -> unsigned
        r.r[2] = 0x1_0000;
        r.r[3] = 0x1_0000;
        r.single_step();
        assert_eq!(r.r[1], 0);
        assert_eq!(r.h, 1);
    }

    #[test]
    fn div_signed_floors_with_positive_divisor() {
        let mut r = cpu();
        r.ram[0] = reg(0, 0, 0, 1, 2, DIV, 3);
        r.r[2] = (-7i32) as u32;
        r.r[3] = 2;
        r.single_step();
        assert_eq!(r.r[1], (-4i32) as u32); // floored
        assert_eq!(r.h, 1);
    }

    #[test]
    fn div_unsigned() {
        let mut r = cpu();
        r.ram[0] = reg(0, 1, 0, 1, 2, DIV, 3); // u=1
        r.r[2] = 17;
        r.r[3] = 5;
        r.single_step();
        assert_eq!(r.r[1], 3);
        assert_eq!(r.h, 2);
    }

    #[test]
    fn div_nonpositive_divisor_uses_idiv() {
        let mut r = cpu();
        r.ram[0] = reg(0, 0, 0, 1, 2, DIV, 3); // signed
        r.r[2] = 7;
        r.r[3] = (-2i32) as u32; // c_val <= 0 -> idiv path
        r.single_step();
        let q = crate::fp::idiv(7, (-2i32) as u32, false);
        assert_eq!((r.r[1], r.h), (q.quot, q.rem));
    }

    // ---- FP dispatch (FSB sign flip) ----

    #[test]
    fn fsb_flips_operand_sign() {
        let mut r = cpu();
        r.ram[0] = reg(0, 0, 0, 1, 2, FSB, 3);
        r.r[2] = 0x4000_0000; // 2.0
        r.r[3] = 0x3F80_0000; // 1.0
        r.single_step();
        // 2.0 - 1.0 == 1.0
        assert_eq!(r.r[1], 0x3F80_0000);
    }

    // ---- Memory ----

    #[test]
    fn store_then_load_word() {
        let mut r = cpu();
        r.ram[0] = mem(1, 0, 1, 2, 0); // store R1 -> [R2]
        r.ram[1] = mem(0, 0, 3, 2, 0); // load [R2] -> R3
        r.r[1] = 0xDEAD_BEEF;
        r.r[2] = 0x100;
        r.single_step();
        r.single_step();
        assert_eq!(r.r[3], 0xDEAD_BEEF);
    }

    #[test]
    fn mem_offset_sign_extends() {
        let mut r = cpu();
        // load [R2 + (-4)] -> R1. off field 0xFFFFC == -4 (20-bit).
        r.ram[0] = mem(0, 0, 1, 2, 0xFFFFC);
        r.r[2] = 0x200;
        r.ram[0x1FC / 4] = 0x1234_5678;
        r.single_step();
        assert_eq!(r.r[1], 0x1234_5678);
    }

    #[test]
    fn store_byte_rmw_little_endian() {
        let mut r = cpu();
        r.ram[0x40] = 0x1122_3344; // address 0x100
        r.store_byte(ByteAddr(0x100), 0xAB);
        assert_eq!(r.ram[0x40], 0x1122_33AB, "low byte replaced, others kept");
        r.store_byte(ByteAddr(0x102), 0xEE);
        assert_eq!(r.ram[0x40], 0x11EE_33AB, "byte 2 replaced");
    }

    #[test]
    fn load_byte_little_endian() {
        let mut r = cpu();
        r.ram[0x40] = 0x1122_3344;
        assert_eq!(r.load_byte(ByteAddr(0x100)), 0x44);
        assert_eq!(r.load_byte(ByteAddr(0x101)), 0x33);
        assert_eq!(r.load_byte(ByteAddr(0x102)), 0x22);
        assert_eq!(r.load_byte(ByteAddr(0x103)), 0x11);
    }

    // ---- Branches ----

    #[test]
    fn branch_taken_forward() {
        let mut r = cpu();
        r.ram[0] = br_imm(0, 7, 0, 5); // unconditional, +5
        r.single_step();
        assert_eq!(r.pc, 6); // 0 -> +1 (fetch) -> +5
    }

    #[test]
    fn branch_taken_backward() {
        let mut r = cpu();
        r.pc = 10;
        r.ram[10] = br_imm(0, 7, 0, 0xFFFFFB); // -5 (24-bit)
        r.single_step();
        assert_eq!(r.pc, 6); // 10 -> +1 -> -5
    }

    #[test]
    fn branch_not_taken() {
        let mut r = cpu();
        r.ram[0] = br_imm(0, 1, 0, 5); // cond Z, Z=false -> not taken
        r.flags.remove(Flags::Z);
        r.single_step();
        assert_eq!(r.pc, 1);
    }

    #[test]
    fn branch_register_indirect() {
        let mut r = cpu();
        r.ram[0] = br_reg(0, 7, 0, 3);
        r.r[3] = 0x40; // byte address -> PC = 0x40/4 = 0x10
        r.single_step();
        assert_eq!(r.pc, 0x10);
    }

    #[test]
    fn branch_with_link_saves_return() {
        let mut r = cpu();
        r.ram[0] = br_imm(0, 7, 1, 5); // link
        r.single_step();
        assert_eq!(r.r[15], 4); // PC (1) * 4
        assert_eq!(r.pc, 6);
    }

    #[test]
    fn branched_into_void_resets() {
        let mut r = cpu();
        r.pc = 0x8_0000; // past RAM, below ROM -> the void
        r.single_step();
        assert_eq!(r.pc, ROM_START / 4);
    }

    // ---- Framebuffer damage ----

    #[test]
    fn store_to_display_marks_damage() {
        let mut r = cpu();
        let _ = r.framebuffer_damage(); // clear initial full-screen damage
        r.store_word(ByteAddr(DEFAULT_DISPLAY_START), 0x0000_00FF);
        let d = r.framebuffer_damage();
        assert_eq!(
            d,
            Damage {
                x1: 0,
                x2: 0,
                y1: 0,
                y2: 0
            }
        );
        assert_eq!(r.framebuffer()[0], 0x0000_00FF);
    }

    // ---- MMIO ----

    #[test]
    fn mmio_millisecond_counter_decrements_progress() {
        let mut r = cpu();
        r.set_time(0x0001_2345);
        r.progress = 20;
        assert_eq!(r.load_io(IO_START), 0x0001_2345);
        assert_eq!(r.progress, 19);
    }

    #[test]
    fn mmio_switches() {
        let mut r = cpu();
        r.set_switches(1);
        assert_eq!(r.load_io(IO_START + 4), 1);
    }

    #[test]
    fn mmio_spi_default_reads_255() {
        let mut r = cpu();
        assert_eq!(r.load_io(IO_START + 16), 255);
        assert_eq!(r.load_io(IO_START + 20), 1); // status: rx ready
    }

    #[test]
    fn mmio_keyboard_and_mouse_status() {
        let mut r = cpu();
        r.progress = 20;
        // No keys: status bit clear, progress decremented.
        assert_eq!(r.load_io(IO_START + 24) & 0x1000_0000, 0);
        assert_eq!(r.progress, 19);
        // Enqueue scancodes.
        r.keyboard_input(&[0x1C, 0x32]);
        assert_eq!(
            r.load_io(IO_START + 24) & 0x1000_0000,
            0x1000_0000,
            "ready bit"
        );
        assert_eq!(r.load_io(IO_START + 28), 0x1C);
        assert_eq!(r.load_io(IO_START + 28), 0x32);
        assert_eq!(r.load_io(IO_START + 28), 0, "drained");
    }

    #[test]
    fn mouse_packing_and_buttons() {
        let mut r = cpu();
        r.mouse_moved(0x123, 0x456);
        assert_eq!(
            r.load_io(IO_START + 24) & 0x00FF_FFFF,
            (0x456 << 12) | 0x123
        );
        r.mouse_button(1, true); // left -> bit 1<<(27-1)=26
        assert_eq!(r.load_io(IO_START + 24) & (1 << 26), 1 << 26);
        r.mouse_button(1, false);
        assert_eq!(r.load_io(IO_START + 24) & (1 << 26), 0);
        // Out-of-range coordinates are ignored.
        r.mouse_moved(-1, 9999);
        assert_eq!(
            r.load_io(IO_START + 24) & 0x00FF_FFFF,
            (0x456 << 12) | 0x123
        );
    }

    // Devices recording dispatch, observable via a shared log.
    type Log = Rc<RefCell<Vec<(char, u32)>>>;

    struct RecSerial(Log, u32);
    impl Serial for RecSerial {
        fn read_status(&mut self) -> u32 {
            self.0.borrow_mut().push(('S', 0));
            self.1
        }
        fn read_data(&mut self) -> u32 {
            self.0.borrow_mut().push(('s', 0));
            self.1
        }
        fn write_data(&mut self, v: u32) {
            self.0.borrow_mut().push(('w', v));
        }
    }

    struct RecSpi(Log, u32);
    impl Spi for RecSpi {
        fn read_data(&mut self) -> u32 {
            self.0.borrow_mut().push(('r', 0));
            self.1
        }
        fn write_data(&mut self, v: u32) {
            self.0.borrow_mut().push(('x', v));
        }
    }

    struct RecLed(Log);
    impl Led for RecLed {
        fn write(&mut self, v: u32) {
            self.0.borrow_mut().push(('l', v));
        }
    }

    struct RecClip(Log);
    impl Clipboard for RecClip {
        fn read_control(&mut self) -> u32 {
            self.0.borrow_mut().push(('C', 0));
            10
        }
        fn write_control(&mut self, v: u32) {
            self.0.borrow_mut().push(('c', v));
        }
        fn read_data(&mut self) -> u32 {
            self.0.borrow_mut().push(('D', 0));
            20
        }
        fn write_data(&mut self, v: u32) {
            self.0.borrow_mut().push(('d', v));
        }
    }

    #[test]
    fn mmio_dispatches_to_devices_by_offset() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut r = cpu();
        r.set_serial(Box::new(RecSerial(log.clone(), 0xAB)));
        r.set_leds(Box::new(RecLed(log.clone())));
        r.set_clipboard(Box::new(RecClip(log.clone())));

        // Serial: offset 8 = data, 12 = status.
        assert_eq!(r.load_io(IO_START + 8), 0xAB);
        assert_eq!(r.load_io(IO_START + 12), 0xAB);
        r.store_io(IO_START + 8, 0x55);

        // SPI: select slave 1, then attach a device there.
        r.store_io(IO_START + 20, 1);
        assert_eq!(r.spi_selected, 1);
        r.set_spi(1, Box::new(RecSpi(log.clone(), 0xCD)));
        assert_eq!(r.load_io(IO_START + 16), 0xCD);
        r.store_io(IO_START + 16, 0x99);

        // LEDs: offset 4 (write only).
        r.store_io(IO_START + 4, 0xF0);

        // Clipboard: 40 = control, 44 = data.
        assert_eq!(r.load_io(IO_START + 40), 10);
        assert_eq!(r.load_io(IO_START + 44), 20);
        r.store_io(IO_START + 40, 7);
        r.store_io(IO_START + 44, 0x41);

        let log = log.borrow();
        assert_eq!(
            *log,
            vec![
                ('s', 0),
                ('S', 0),
                ('w', 0x55),
                ('r', 0),
                ('x', 0x99),
                ('l', 0xF0),
                ('C', 0),
                ('D', 0),
                ('c', 7),
                ('d', 0x41),
            ]
        );
    }

    #[test]
    fn set_spi_ignores_invalid_index() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut r = cpu();
        r.set_spi(0, Box::new(RecSpi(log.clone(), 1)));
        r.set_spi(3, Box::new(RecSpi(log.clone(), 1)));
        assert!(r.spi[0].is_none());
        assert!(r.spi[3].is_none());
    }

    #[test]
    fn configure_memory_patches_rom_and_resizes() {
        let mut r = Risc::new();
        r.configure_memory(2, 800, 600);
        assert_eq!(r.display_start, 2 << 20);
        assert_eq!(r.mem_size, (2 << 20) + 800 * 600 / 8);
        assert_eq!(r.fb_width, 800 / 32);
        assert_eq!(r.fb_height, 600);
        // The magic words land at the default display start.
        let d = (DEFAULT_DISPLAY_START / 4) as usize;
        assert_eq!(r.ram[d], 0x5369_7A67);
        assert_eq!(r.ram[d + 1], 800);
        assert_eq!(r.ram[d + 2], 600);
        assert_eq!(r.ram[d + 3], 2 << 20);
        // Megabytes clamp to [1, 32].
        r.configure_memory(0, 1024, 768);
        assert_eq!(r.display_start, 1 << 20);
        r.configure_memory(99, 1024, 768);
        assert_eq!(r.display_start, 32 << 20);
    }

    #[test]
    fn reset_jumps_to_rom() {
        let mut r = Risc::new();
        assert_eq!(r.pc, ROM_START / 4);
        r.pc = 0;
        r.reset();
        assert_eq!(r.pc, ROM_START / 4);
    }

    #[test]
    fn fetches_first_instruction_from_rom() {
        // Reset PC points at the boot ROM; the first ROM word is a branch that
        // does not fault ("branched into the void" would reset PC).
        let mut r = Risc::new();
        assert_eq!(r.rom[0], BOOTLOADER[0]);
        r.single_step();
        assert_ne!(r.pc, ROM_START / 4 + 1, "first ROM word should branch");
    }
}
