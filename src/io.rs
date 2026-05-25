//! Device callback traits (port of `risc-io.h`).
//!
//! The C uses `const*` callback structs plus mutable `static` globals; the
//! honest Rust equivalent is a trait with interior-mutable state, so every
//! method takes `&mut self`. The core ([`crate::risc::Risc`]) owns each device
//! as an `Option<Box<dyn _>>` and invokes it synchronously from inside the CPU
//! step (`load_io`/`store_io`).

/// RS232 serial line: PCLink file transfer or a raw host serial port.
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
