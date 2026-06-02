//! The `arboard`-backed host clipboard that bridges the OS clipboard to the
//! core's [`ClipboardBridge`](risc_core::clipboard::ClipboardBridge).

use risc_core::clipboard::HostClipboard;

/// The `arboard`-backed host clipboard used by the frontend.
pub struct ArboardClipboard {
    inner: Option<arboard::Clipboard>,
}

impl ArboardClipboard {
    pub fn new() -> Self {
        ArboardClipboard {
            inner: arboard::Clipboard::new().ok(),
        }
    }
}

impl Default for ArboardClipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl HostClipboard for ArboardClipboard {
    fn get_text(&mut self) -> Option<String> {
        self.inner.as_mut()?.get_text().ok()
    }
    fn set_text(&mut self, text: &str) {
        if let Some(c) = self.inner.as_mut() {
            let _ = c.set_text(text.to_owned());
        }
    }
}
