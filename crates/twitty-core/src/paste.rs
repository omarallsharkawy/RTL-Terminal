/// Prepare clipboard text for the PTY. With bracketed paste the shell receives it verbatim
/// (and won't run it line by line); otherwise newlines become carriage returns like a typed Enter.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        // Strip ESC, 8-bit C1 CSI (U+009B) and NUL so pasted content can't terminate the
        // bracket early or inject control sequences.
        let body: String = text
            .chars()
            .filter(|&c| !matches!(c, '\x1b' | '\u{009b}' | '\0'))
            .collect();
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(body.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}
