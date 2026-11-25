//! Session identity and the attach handshake shared by the two binaries.

pub const ATTACH_MARKER: &[u8] = b"\x1b]777;twitty-remote-attached\x07";
pub const BOOTSTRAP_FAILED: u32 = 254;
pub const SSH_DISCONNECTED: u32 = 255;

pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(
            "invalid SSH session name: use 1–64 letters A–Z or a–z, digits, '_' or '-'".into(),
        );
    }
    Ok(())
}
