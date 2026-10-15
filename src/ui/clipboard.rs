//! The system clipboard, opened on first use and then kept for the whole session: on Linux,
//! copied text is served by this object and would vanish if it were dropped right after
//! copying.

#[derive(Default)]
pub struct Clipboard {
    inner: Option<arboard::Clipboard>,
}

impl Clipboard {
    fn get(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.inner.is_none() {
            self.inner = arboard::Clipboard::new().ok();
        }
        self.inner.as_mut()
    }

    /// The clipboard's text, if any could be read.
    pub fn text(&mut self) -> Option<String> {
        self.get()?.get_text().ok()
    }

    pub fn set_text(&mut self, text: String) {
        if let Some(clipboard) = self.get() {
            let _ = clipboard.set_text(text);
        }
    }
}
