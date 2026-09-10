use reqwest::{Client, Response};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{ProviderError, ProviderEvent};

const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;

pub(super) fn retry_after(response: &Response) -> Option<std::time::Duration> {
    parse_retry_after(
        response
            .headers()
            .get(reqwest::header::RETRY_AFTER)?
            .to_str()
            .ok()?,
        std::time::SystemTime::now(),
    )
}

fn parse_retry_after(value: &str, now: std::time::SystemTime) -> Option<std::time::Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        // Bound untrusted server hints to one day; waits remain cancellable.
        return Some(std::time::Duration::from_secs(seconds.clamp(1, 86_400)));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let timestamp = u64::try_from(date.timestamp()).ok()?;
    let deadline = std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(timestamp))?;
    Some(deadline.duration_since(now).unwrap_or_default().clamp(
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(86_400),
    ))
}

pub(super) fn build_client() -> Result<Client, ProviderError> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| ProviderError::Request(error.to_string()))
}

pub(super) fn require_api_key(api_key: &str) -> Result<(), ProviderError> {
    if api_key.trim().is_empty() {
        Err(ProviderError::Request(
            "API key is not configured".to_string(),
        ))
    } else {
        Ok(())
    }
}

pub(super) async fn read_error_message(
    mut response: Response,
    cancellation: &CancellationToken,
    secret: &str,
) -> Result<String, ProviderError> {
    let mut bytes = Vec::new();
    while bytes.len() < MAX_ERROR_BODY_BYTES {
        let chunk = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
            chunk = response.chunk() => chunk,
        };
        match chunk {
            Ok(Some(chunk)) => {
                let remaining = MAX_ERROR_BODY_BYTES - bytes.len();
                bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            Ok(None) | Err(_) => break,
        }
    }

    let text = String::from_utf8_lossy(&bytes).trim().to_string();
    let message = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| {
            if text.is_empty() {
                "provider returned an empty error response".to_string()
            } else {
                text
            }
        });
    Ok(redact(&message, secret))
}

pub(super) fn redact_event(event: ProviderEvent, secret: &str) -> ProviderEvent {
    match event {
        ProviderEvent::TextDelta { delta } => ProviderEvent::TextDelta {
            delta: redact(&delta, secret),
        },
        ProviderEvent::ReasoningSummaryDelta { item_id, delta } => {
            ProviderEvent::ReasoningSummaryDelta {
                item_id,
                delta: redact(&delta, secret),
            }
        }
        ProviderEvent::ReasoningSummaryCompleted { item_id, summary } => {
            ProviderEvent::ReasoningSummaryCompleted {
                item_id,
                summary: redact(&summary, secret),
            }
        }
        ProviderEvent::ToolCall { mut call } => {
            redact_json(&mut call.arguments, secret);
            ProviderEvent::ToolCall { call }
        }
        ProviderEvent::ProviderContext { provider, mut item } => {
            redact_json(&mut item, secret);
            ProviderEvent::ProviderContext { provider, item }
        }
        other => other,
    }
}

fn redact_json(value: &mut Value, secret: &str) {
    match value {
        Value::String(text) => *text = redact(text, secret),
        Value::Array(values) => {
            for value in values {
                redact_json(value, secret);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                redact_json(value, secret);
            }
        }
        _ => {}
    }
}

pub(super) fn redact_error(error: ProviderError, secret: &str) -> ProviderError {
    match error {
        ProviderError::RateLimited {
            message,
            retry_after,
        } => ProviderError::RateLimited {
            message: redact(&message, secret),
            retry_after,
        },
        ProviderError::Request(message) => ProviderError::Request(redact(&message, secret)),
        ProviderError::Http { status, message } => ProviderError::Http {
            status,
            message: redact(&message, secret),
        },
        ProviderError::InvalidResponse(message) => {
            ProviderError::InvalidResponse(redact(&message, secret))
        }
        ProviderError::Unavailable(message) => ProviderError::Unavailable(redact(&message, secret)),
        other => other,
    }
}

pub(super) fn classify_event_error(
    message: String,
    code: Option<&str>,
    error_type: Option<&str>,
) -> ProviderError {
    if code.into_iter().chain(error_type).any(|value| {
        matches!(
            value.trim(),
            "rate_limit_exceeded" | "rate_limit_error" | "rate_limited"
        )
    }) {
        return ProviderError::RateLimited {
            message,
            retry_after: None,
        };
    }
    let transient_code = code
        .into_iter()
        .chain(error_type)
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .any(|value| {
            matches!(
                value.as_str(),
                "server_error"
                    | "internal_error"
                    | "internal_server_error"
                    | "overloaded_error"
                    | "server_overloaded"
                    | "service_unavailable"
                    | "temporarily_unavailable"
            )
        });
    let normalized_message = message.to_ascii_lowercase();
    let transient_message = normalized_message.contains("overloaded")
        || normalized_message.contains("temporarily unavailable")
        || normalized_message.contains("server is busy")
        || normalized_message.contains("servers are busy");
    if transient_code || transient_message {
        ProviderError::Unavailable(message)
    } else {
        ProviderError::InvalidResponse(message)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn retry_after_hints_are_parsed_bounded_and_invalid_hints_are_ignored() {
        use std::time::{Duration, UNIX_EPOCH};
        let now = UNIX_EPOCH + Duration::from_secs(1_445_412_420);
        assert_eq!(
            super::parse_retry_after("120", now),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            super::parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", now),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            super::parse_retry_after("0", now),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            super::parse_retry_after("999999999", now),
            Some(Duration::from_secs(86400))
        );
        for value in ["bad", "-1", "1.5", ""] {
            assert_eq!(super::parse_retry_after(value, now), None);
        }
    }

    #[tokio::test]
    async fn http_rate_limit_preserves_header_and_redacts_the_secret() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            let body = r#"{"error":{"message":"Free-tier request limit reached secret-fixture"}}"#;
            let wire = format!(
                "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 73\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(wire.as_bytes()).await.unwrap();
        });
        let response = super::build_client()
            .unwrap()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap();
        let hint = super::retry_after(&response);
        let status = response.status().as_u16();
        let message = super::read_error_message(
            response,
            &tokio_util::sync::CancellationToken::new(),
            "secret-fixture",
        )
        .await
        .unwrap();
        let error = crate::providers::ProviderError::from_http(status, message, hint);
        assert_eq!(
            error.rate_limit_delay(),
            Some(std::time::Duration::from_secs(73))
        );
        assert!(!error.to_string().contains("secret-fixture"));
        assert_eq!(error.turn_error(error.to_string()).code, "rate_limited");
        server.await.unwrap();
    }

    use super::{classify_event_error, redact_error};
    use crate::providers::ProviderError;

    #[test]
    fn classifies_structured_and_legacy_overload_events_as_unavailable() {
        assert!(matches!(
            classify_event_error(
                "Our servers are currently overloaded. Please try again later.".into(),
                Some("server_error"),
                None,
            ),
            ProviderError::Unavailable(message) if message.contains("overloaded")
        ));
        assert!(matches!(
            classify_event_error("Service temporarily unavailable".into(), None, None),
            ProviderError::Unavailable(_)
        ));
        assert!(matches!(
            classify_event_error(
                "invalid tool schema".into(),
                Some("invalid_request_error"),
                None
            ),
            ProviderError::InvalidResponse(_)
        ));
    }

    #[test]
    fn redacts_secrets_from_temporary_unavailability_errors() {
        let error = redact_error(
            ProviderError::Unavailable("server rejected secret-key while overloaded".into()),
            "secret-key",
        );
        assert_eq!(
            error,
            ProviderError::Unavailable("server rejected [REDACTED] while overloaded".into())
        );
    }
}

fn redact(value: &str, secret: &str) -> String {
    if secret.is_empty() {
        value.to_string()
    } else {
        value.replace(secret, "[REDACTED]")
    }
}
