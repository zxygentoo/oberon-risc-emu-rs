//! The clipboard GET/PUT state machine bridging the host clipboard to Oberon
//! (port of `sdl-clipboard.c`).
//!
//! Oberon uses bare `CR` line endings; the host uses `LF` (or `CRLF`). On GET
//! (host -> Oberon) `CRLF`/`LF` are folded to `CR`; on PUT (Oberon -> host) `CR`
//! becomes `LF`. The state machine and conversions are host-agnostic (a
//! [`HostClipboard`] supplies the actual text), so the core builds and tests
//! without the `arboard` dependency; the arboard backend is feature-gated.

use crate::io::Clipboard;

/// Abstraction over the host system clipboard, so the bridge is testable and the
/// core doesn't depend on `arboard`.
pub trait HostClipboard {
    fn get_text(&mut self) -> Option<String>;
    fn set_text(&mut self, text: &str);
}

#[derive(PartialEq, Eq)]
enum State {
    Idle,
    Get,
    Put,
}

/// The clipboard device exposed to the CPU over the [`Clipboard`] MMIO ports.
pub struct ClipboardBridge {
    host: Box<dyn HostClipboard>,
    state: State,
    data: Vec<u8>,
    ptr: usize,
    len: usize,
}

impl ClipboardBridge {
    pub fn new(host: Box<dyn HostClipboard>) -> Self {
        ClipboardBridge {
            host,
            state: State::Idle,
            data: Vec::new(),
            ptr: 0,
            len: 0,
        }
    }

    fn reset(&mut self) {
        self.state = State::Idle;
        self.data = Vec::new();
        self.len = 0;
        self.ptr = 0;
    }
}

impl Clipboard for ClipboardBridge {
    fn read_control(&mut self) -> u32 {
        self.reset();
        let Some(text) = self.host.get_text() else {
            return 0;
        };
        let data = text.into_bytes();
        let data_len = data.len();
        if data_len == 0 || data_len > u32::MAX as usize {
            return 0;
        }
        // Announce the length Oberon will receive: each CRLF collapses to one CR.
        let mut r = data_len as u32;
        for w in data.windows(2) {
            if w == b"\r\n" {
                r -= 1;
            }
        }
        self.data = data;
        self.len = data_len;
        self.ptr = 0;
        self.state = State::Get;
        r
    }

    fn write_control(&mut self, len: u32) {
        self.reset();
        if len < u32::MAX {
            self.data = vec![0u8; len as usize];
            self.len = len as usize;
            self.state = State::Put;
        }
    }

    fn read_data(&mut self) -> u32 {
        if self.state != State::Get || self.ptr >= self.len {
            return 0;
        }
        let mut result = self.data[self.ptr] as u32;
        self.ptr += 1;
        if result == b'\r' as u32 && self.ptr < self.len && self.data[self.ptr] == b'\n' {
            self.ptr += 1; // CRLF -> CR
        } else if result == b'\n' as u32 {
            result = b'\r' as u32; // lone LF -> CR
        }
        if self.ptr == self.len {
            self.reset();
        }
        result
    }

    fn write_data(&mut self, c: u32) {
        if self.state != State::Put || self.ptr >= self.len {
            return;
        }
        let byte = if c == b'\r' as u32 { b'\n' } else { c as u8 }; // CR -> LF
        self.data[self.ptr] = byte;
        self.ptr += 1;
        if self.ptr == self.len {
            let text = String::from_utf8_lossy(&self.data).into_owned();
            self.host.set_text(&text);
            self.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct FakeHost(Rc<RefCell<String>>);
    impl HostClipboard for FakeHost {
        fn get_text(&mut self) -> Option<String> {
            Some(self.0.borrow().clone())
        }
        fn set_text(&mut self, text: &str) {
            *self.0.borrow_mut() = text.to_owned();
        }
    }

    fn bridge(content: &Rc<RefCell<String>>) -> ClipboardBridge {
        ClipboardBridge::new(Box::new(FakeHost(content.clone())))
    }

    #[test]
    fn get_folds_crlf_and_lf_to_cr() {
        let content = Rc::new(RefCell::new("ab\r\ncd\nef".to_string()));
        let mut clip = bridge(&content);
        // 9 bytes; one CRLF collapses -> announced length 8.
        assert_eq!(clip.read_control(), 8);
        let mut out = Vec::new();
        for _ in 0..8 {
            out.push(clip.read_data() as u8);
        }
        assert_eq!(out, b"ab\rcd\ref");
        // Drained -> back to idle.
        assert_eq!(clip.read_data(), 0);
    }

    #[test]
    fn put_converts_cr_to_lf_and_sets_host() {
        let content = Rc::new(RefCell::new(String::new()));
        let mut clip = bridge(&content);
        let input = b"x\ry"; // Oberon sends CR line endings
        clip.write_control(input.len() as u32);
        for &b in input {
            clip.write_data(b as u32);
        }
        assert_eq!(*content.borrow(), "x\ny");
    }

    #[test]
    fn empty_clipboard_reads_zero() {
        let content = Rc::new(RefCell::new(String::new()));
        let mut clip = bridge(&content);
        assert_eq!(clip.read_control(), 0);
        assert_eq!(clip.read_data(), 0);
    }
}
