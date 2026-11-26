//! Shared SSH destination and remote-session name checks.

mod wire;

pub use wire::{validate_name, ATTACH_MARKER, BOOTSTRAP_FAILED, SSH_DISCONNECTED};

pub fn validate_destination(destination: &str) -> Result<(), String> {
    let host = destination.rsplit('@').next().unwrap_or_default();
    if host.is_empty()
        || destination.len() > 1024
        || destination.starts_with('-')
        || destination.starts_with('@')
        || host.starts_with('-')
        || destination.chars().any(|c| {
            !c.is_alphanumeric() && !matches!(c, '.' | '_' | '-' | '@' | ':' | '[' | ']' | '%')
        })
    {
        return Err("invalid SSH destination: use a host, SSH alias, or user@host without whitespace, control characters, or shell punctuation".into());
    }
    Ok(())
}
