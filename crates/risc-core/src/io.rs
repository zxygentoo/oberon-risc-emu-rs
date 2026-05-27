//! Device callback traits (port of `risc-io.h`).
//!
//! The C uses `const*` callback structs plus mutable `static` globals; the
//! honest Rust equivalent is a trait with interior-mutable state, so every
//! method takes `&mut self`. The core ([`crate::risc::Risc`]) owns each device
//! as an `Option<Box<dyn _>>` and invokes it synchronously from inside the CPU
//! step (`load_io`/`store_io`).

/// RS232 serial line: `PCLink` file transfer or a raw host serial port.
pub trait Serial {
    fn read_status(&mut self) -> u32;
    fn read_data(&mut self) -> u32;
    fn write_data(&mut self, value: u32);
}

/// An SPI slave (the SD-card disk lives here).
pub trait Spi {
    fn read_data(&mut self) -> u32;
    fn write_data(&mut self, value: u32);
}

/// The emulator-only clipboard bridge (host clipboard <-> Oberon).
pub trait Clipboard {
    fn read_control(&mut self) -> u32;
    fn write_control(&mut self, value: u32);
    fn read_data(&mut self) -> u32;
    fn write_data(&mut self, value: u32);
}

/// The board LEDs (optional logging device).
pub trait Led {
    fn write(&mut self, value: u32);
}

/// A byte-addressable view of the machine's RAM, handed to a [`NoreboHost`] so
/// host syscalls can read their arguments and read/write file data directly in
/// the emulated memory (the C `norebo` operates on a global `mem[]` array; this
/// is the borrow-safe equivalent).
pub struct NoreboMem<'a> {
    ram: &'a mut [u32],
    size: u32,
}

impl<'a> NoreboMem<'a> {
    /// Wrap a RAM word-slice of `size` bytes.
    pub fn new(ram: &'a mut [u32], size: u32) -> Self {
        Self { ram, size }
    }

    /// The RAM size in bytes.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Read the 32-bit word at byte address `adr` (must be word-aligned).
    pub fn read_word(&self, adr: u32) -> u32 {
        self.ram[(adr / 4) as usize]
    }

    /// Read the byte at `adr`.
    pub fn read_byte(&self, adr: u32) -> u8 {
        (self.ram[(adr / 4) as usize] >> ((adr % 4) * 8)) as u8
    }

    /// Write the byte at `adr` (read-modify-write of its containing word).
    pub fn write_byte(&mut self, adr: u32, value: u8) {
        let i = (adr / 4) as usize;
        let shift = (adr % 4) * 8;
        self.ram[i] = (self.ram[i] & !(0xFFu32 << shift)) | (u32::from(value) << shift);
    }

    /// Copy `buf.len()` bytes from memory starting at `adr` into `buf`.
    pub fn read_bytes(&self, adr: u32, buf: &mut [u8]) {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = self.read_byte(adr + i as u32);
        }
    }

    /// Copy `buf` into memory starting at `adr`.
    pub fn write_bytes(&mut self, adr: u32, buf: &[u8]) {
        for (i, &b) in buf.iter().enumerate() {
            self.write_byte(adr + i as u32, b);
        }
    }
}

/// The Norebo host: the syscall/IO backend that maps Oberon's `Kernel`/`Files`
/// operations onto the host (a port of `norebo.c`). When a [`crate::risc::Risc`]
/// has one attached it routes the MMIO region through these calls instead of the
/// FPGA device map, and boots from an inner-core image rather than the disk.
///
/// `offset` is the MMIO byte offset (`address - 0xFFFFFFC0`, i.e. norebo's
/// `adr + 64`): `0` clock, `4` LEDs, `8` stdin/stdout, `12` status, `48/52/56`
/// syscall args 2/1/0, `60` syscall trigger / result.
pub trait NoreboHost {
    /// Read the MMIO word at `offset`.
    fn load(&mut self, offset: u32, mem: &mut NoreboMem) -> u32;
    /// Write `value` to the MMIO word at `offset` (offset 60 dispatches a syscall).
    fn store(&mut self, offset: u32, value: u32, mem: &mut NoreboMem);
    /// `Some(code)` once the guest has halted (syscall 1) or trapped.
    fn exit_code(&self) -> Option<i32>;
}
