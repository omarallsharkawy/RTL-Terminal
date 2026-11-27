//! Synchronous rustls transport. One client per request permits targeted socket shutdown.

use std::io::BufReader;

use twitty_store::{SecretStore, SecretValue};
use ureq::http::{header, HeaderValue, Request, Uri};

use super::{api, Cancellation, ProviderError, ProviderRequest, ProviderTurn};
use crate::engine::{model::ProviderKind, DEFAULT_OLLAMA_URL};

/// Keyring account names shared with AI settings.
pub fn secret_account(kind: &ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::Anthropic => Some("anthropic-api-key"),
        ProviderKind::OpenAi => Some("openai-api-key"),
        ProviderKind::Gemini => Some("gemini-api-key"),
        _ => None,
    }
}

fn endpoint(kind: &ProviderKind, model: &str) -> Result<Uri, ProviderError> {
    let url = match kind {
        ProviderKind::Anthropic => "https://api.anthropic.com/v1/messages".to_owned(),
        ProviderKind::OpenAi => "https://api.openai.com/v1/chat/completions".to_owned(),
        ProviderKind::Gemini
            if !model.is_empty()
                && model
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) =>
        {
            format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?alt=sse")
        }
        ProviderKind::Ollama { base_url } => {
            let base = if base_url.is_empty() {
                DEFAULT_OLLAMA_URL
            } else {
                base_url
            };
            if base.contains(['?', '#', '@', '\\']) {
                return Err(ProviderError::Configuration);
            }
            format!("{}/api/chat", base.trim_end_matches('/'))
        }
        _ => return Err(ProviderError::Configuration),
    };
    let uri: Uri = url.parse().map_err(|_| ProviderError::Configuration)?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.host().is_none() {
        return Err(ProviderError::Configuration);
    }
    if matches!(kind, ProviderKind::Ollama { .. }) && !is_allowed_ollama_uri(&uri) {
        return Err(ProviderError::Configuration);
    }
    Ok(uri)
}

fn is_allowed_ollama_uri(uri: &Uri) -> bool {
    let Some(raw_host) = uri.host() else {
        return false;
    };
    let host = raw_host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(raw_host);
    if host.eq_ignore_ascii_case("localhost") {
        return matches!(uri.scheme_str(), Some("http" | "https"));
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip.is_loopback() {
            return matches!(uri.scheme_str(), Some("http" | "https"));
        }
        if ip.is_unspecified() || ip.is_multicast() || is_link_local(ip) {
            return false;
        }
        return uri.scheme_str() == Some("https");
    }
    uri.scheme_str() == Some("https")
}

fn is_link_local(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_link_local(),
        std::net::IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

fn wire_request(
    request: &ProviderRequest,
    secret: Option<&SecretValue>,
) -> Result<Request<String>, ProviderError> {
    let uri = endpoint(&request.kind, &request.model)?;
    let credential = secret.map(SecretValue::expose);
    if credential.is_some_and(|key| key.is_empty() || uri.to_string().contains(key)) {
        return Err(ProviderError::Configuration);
    }
    let mut body = api::build_request(request)?;
    super::scrub(&mut body, credential);
    let ollama = matches!(request.kind, ProviderKind::Ollama { .. });
    let mut wire = Request::post(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::ACCEPT,
            if ollama {
                "application/x-ndjson"
            } else {
                "text/event-stream"
            },
        )
        .body(body.to_string())
        .map_err(|_| ProviderError::Configuration)?;
    if let Some(key) = credential {
        let (name, value) = match request.kind {
            ProviderKind::Anthropic => {
                wire.headers_mut()
                    .insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
                ("x-api-key", HeaderValue::from_str(key))
            }
            ProviderKind::OpenAi => (
                "authorization",
                HeaderValue::from_str(&format!("Bearer {key}")),
            ),
            ProviderKind::Gemini => ("x-goog-api-key", HeaderValue::from_str(key)),
            _ => return Err(ProviderError::Configuration),
        };
        let mut value = value.map_err(|_| ProviderError::Credential)?;
        value.set_sensitive(true);
        wire.headers_mut().insert(name, value);
    }
    Ok(wire)
}

pub(super) fn complete(
    request: &ProviderRequest,
    secrets: &dyn SecretStore,
    cancel: &Cancellation,
    preview: &mut dyn FnMut(String),
) -> Result<ProviderTurn, ProviderError> {
    if cancel.is_cancelled() {
        return Err(ProviderError::Cancelled);
    }
    let secret = secret_account(&request.kind)
        .map(|account| {
            secrets
                .get(account)
                .ok()
                .flatten()
                .filter(|secret| !secret.is_empty())
                .ok_or(ProviderError::Credential)
        })
        .transpose()?;
    let wire = wire_request(request, secret.as_ref())?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .timeout_global(Some(request.timeout))
        .build()
        .into();
    let interrupter = agent.clone();
    cancel.interrupt_with(move || interrupter.shutdown());
    if cancel.is_cancelled() {
        return Err(ProviderError::Cancelled);
    }
    // Error details can contain peer-controlled strings. Expose only fixed categories.
    let response = agent.run(wire).map_err(|error| {
        if cancel.is_cancelled() {
            ProviderError::Cancelled
        } else if matches!(error, ureq::Error::Timeout(_)) {
            ProviderError::Timeout
        } else {
            ProviderError::Unavailable
        }
    })?;
    if !response.status().is_success() {
        return Err(ProviderError::Unavailable);
    }
    let credential = secret.as_ref().map(SecretValue::expose);
    api::stream(
        &request.kind,
        BufReader::new(response.into_body().into_reader()),
        cancel,
        credential,
        preview,
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::engine::provider::Message;
    use twitty_store::InMemorySecretStore;

    #[test]
    fn keyring_key_is_only_in_sensitive_http_header() {
        let store = InMemorySecretStore::new();
        let key = "opaque-provider-credential-with-no-recognized-prefix";
        for kind in [
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::Gemini,
        ] {
            let account = secret_account(&kind).unwrap();
            store.set(account, key).unwrap();
            let req = ProviderRequest {
                kind,
                model: "test-model".into(),
                system: "instructions".into(),
                messages: vec![Message::User(format!("a credential {key}"))],
                tools: Vec::new(),
                max_output_tokens: 1,
                timeout: Duration::from_secs(1),
            };
            let secret = store.get(account).unwrap().unwrap();
            let wire = wire_request(&req, Some(&secret)).unwrap();
            assert!(!wire.body().contains(key));
            assert!(!format!("{store:?} {secret:?} {wire:?}").contains(key));
            assert!(!wire.uri().to_string().contains(key));
            assert_eq!(wire.headers().values().filter(|header| header.is_sensitive()
                && header.to_str().unwrap().contains(key)).count(), 1);
        }
    }

    #[test]
    fn ollama_endpoint_rejects_remote_cleartext_and_metadata_ips() {
        for ok in [
            "",
            "http://127.0.0.1:11434",
            "http://localhost:11434",
            "http://[::1]:11434",
            "https://ollama.example.com",
        ] {
            let kind = ProviderKind::Ollama {
                base_url: ok.into(),
            };
            assert!(endpoint(&kind, "llama3").is_ok(), "should accept {ok:?}");
        }
        for bad in [
            "http://169.254.169.254",
            "https://169.254.169.254",
            "http://example.com:11434",
            "https://0.0.0.0:11434",
            "https://[fe80::1]:11434",
        ] {
            let kind = ProviderKind::Ollama {
                base_url: bad.into(),
            };
            assert_eq!(
                endpoint(&kind, "llama3"),
                Err(ProviderError::Configuration),
                "should reject {bad:?}"
            );
        }
    }
}
