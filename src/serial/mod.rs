//! Serial-line devices: PCLink file transfer and raw host serial.

pub mod pclink;

#[cfg(unix)]
pub mod raw_serial;
