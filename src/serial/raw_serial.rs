//! Raw serial line over host file descriptors (port of the POSIX branch of
//! `raw-serial.c`), used by `--serial-in`/`--serial-out`. Non-blocking fds with
//! `poll(2)` for the ready/writable status bits. Windows named pipes are a
//! Phase-2 item.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;

use crate::io::Serial;

pub struct RawSerial {
    fd_in: File,
    fd_out: File,
}

impl RawSerial {
    /// Open the input (read-only) and output (read-write) files non-blocking.
    pub fn new(filename_in: &Path, filename_out: &Path) -> std::io::Result<Self> {
        let fd_in = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(filename_in)?;
        let fd_out = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(filename_out)?;
        Ok(RawSerial { fd_in, fd_out })
    }
}

impl Serial for RawSerial {
    fn read_status(&mut self) -> u32 {
        let mut fds = [
            libc::pollfd { fd: self.fd_in.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: self.fd_out.as_raw_fd(), events: libc::POLLOUT, revents: 0 },
        ];
        let mut status = 0;
        let r = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 0) };
        if r > 0 {
            if fds[0].revents & libc::POLLIN != 0 {
                status |= 1; // rx ready
            }
            if fds[1].revents & libc::POLLOUT != 0 {
                status |= 2; // tx ready
            }
        }
        status
    }

    fn read_data(&mut self) -> u32 {
        let mut b = [0u8; 1];
        let _ = self.fd_in.read(&mut b); // non-blocking; 0 on no data/EOF
        b[0] as u32
    }

    fn write_data(&mut self, value: u32) {
        let _ = self.fd_out.write(&[value as u8]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_null_is_writable_and_reads_zero() {
        let dn = Path::new("/dev/null");
        let mut s = RawSerial::new(dn, dn).expect("open /dev/null");
        // /dev/null is always writable; reads hit EOF -> 0.
        assert_ne!(s.read_status() & 2, 0, "tx should be ready");
        assert_eq!(s.read_data(), 0);
    }
}
