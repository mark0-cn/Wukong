//! Bounded, read-only HTTP client shared by all observability backends.

use crate::adapters::telemetry::BackendConfig;
use reqwest::{Client, StatusCode, Url, header};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

pub(crate) const DEFAULT_RESPONSE_CAP: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendErrorKind {
    InvalidEndpoint,
    Authentication,
    Timeout,
    Unavailable,
    Unauthorized,
    Forbidden,
    NotFound,
    RateLimited,
    HttpError,
    ResponseTooLarge,
    InvalidResponse,
    InvalidRequest,
}

/// A deliberately fixed, safe error. Backend response bodies and reqwest errors
/// are never copied into this DTO because either can contain sensitive material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackendError {
    pub kind: BackendErrorKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

impl BackendError {
    fn new(kind: BackendErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            status: None,
        }
    }

    fn status(status: StatusCode) -> Self {
        let (kind, message) = match status {
            StatusCode::UNAUTHORIZED => (
                BackendErrorKind::Unauthorized,
                "backend authentication was rejected",
            ),
            StatusCode::FORBIDDEN => (BackendErrorKind::Forbidden, "backend access was forbidden"),
            StatusCode::NOT_FOUND => (
                BackendErrorKind::NotFound,
                "the requested backend resource was not found",
            ),
            StatusCode::TOO_MANY_REQUESTS => (
                BackendErrorKind::RateLimited,
                "backend query rate limit was reached",
            ),
            _ => (
                BackendErrorKind::HttpError,
                "backend returned an unsuccessful HTTP status",
            ),
        };
        Self {
            kind,
            message: message.into(),
            status: Some(status.as_u16()),
        }
    }
}

#[derive(Clone)]
pub(crate) struct BackendClient {
    client: Client,
    endpoint: Url,
    origin: String,
    response_cap: usize,
    credential: Option<String>,
    credential_error: Option<BackendError>,
}

impl BackendClient {
    pub(crate) fn new(config: &BackendConfig, timeout_secs: u64) -> Result<Self, BackendError> {
        Self::with_credential(config, timeout_secs, DEFAULT_RESPONSE_CAP, |name| {
            std::env::var(name).map(Some).map_err(|_| {
                BackendError::new(
                    BackendErrorKind::Authentication,
                    "credential environment variable is unavailable or invalid",
                )
            })
        })
    }

    fn with_credential(
        config: &BackendConfig,
        timeout_secs: u64,
        response_cap: usize,
        credential: impl FnOnce(&str) -> Result<Option<String>, BackendError>,
    ) -> Result<Self, BackendError> {
        let endpoint = validated_endpoint(&config.url)?;
        if !(1..=60).contains(&timeout_secs) || response_cap == 0 {
            return Err(BackendError::new(
                BackendErrorKind::InvalidEndpoint,
                "backend query limits are invalid",
            ));
        }
        let mut headers = header::HeaderMap::new();
        let mut credential_value = None;
        let mut credential_error = None;
        if let Some(variable) = &config.credential_env {
            match credential(variable) {
                Ok(Some(token)) => {
                    match header::HeaderValue::from_str(&format!("Bearer {token}")) {
                        Ok(mut value) => {
                            value.set_sensitive(true);
                            headers.insert(header::AUTHORIZATION, value);
                            credential_value = Some(token);
                        }
                        Err(_) => {
                            credential_error = Some(BackendError::new(
                                BackendErrorKind::Authentication,
                                "credential environment variable contains an invalid bearer token",
                            ));
                        }
                    }
                }
                Ok(None) => {}
                Err(error) => credential_error = Some(error),
            }
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(headers)
            .build()
            .map_err(|_| {
                BackendError::new(
                    BackendErrorKind::InvalidEndpoint,
                    "backend HTTP client could not be constructed",
                )
            })?;
        let origin = endpoint.origin().ascii_serialization();
        Ok(Self {
            client,
            endpoint,
            origin,
            response_cap,
            credential: credential_value,
            credential_error,
        })
    }

    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }

    pub(crate) fn authenticated(config: &BackendConfig) -> bool {
        config
            .credential_env
            .as_ref()
            .is_some_and(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    }

    pub(crate) async fn get_json(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Value, BackendError> {
        if let Some(error) = &self.credential_error {
            return Err(error.clone());
        }
        let mut url = self.endpoint.clone();
        let base = self.endpoint.path().trim_end_matches('/');
        url.set_path(&format!("{base}/{}", path.trim_start_matches('/')));
        {
            let mut pairs = url.query_pairs_mut();
            for (name, value) in query {
                pairs.append_pair(name, value);
            }
        }
        let response = self.client.get(url).send().await.map_err(transport_error)?;
        if !response.status().is_success() {
            return Err(BackendError::status(response.status()));
        }
        let mut response = response;
        let mut bytes = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if bytes.len().saturating_add(chunk.len()) > self.response_cap {
                        return Err(BackendError::new(
                            BackendErrorKind::ResponseTooLarge,
                            "backend response exceeded the configured byte limit",
                        ));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(error) => return Err(transport_error(error)),
            }
        }
        let mut value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            BackendError::new(
                BackendErrorKind::InvalidResponse,
                "backend returned an invalid JSON response",
            )
        })?;
        if let Some(secret) = &self.credential {
            redact_json(&mut value, secret);
        }
        Ok(value)
    }
}

fn redact_json(value: &mut Value, secret: &str) {
    if secret.is_empty() {
        return;
    }
    match value {
        Value::String(text) => *text = text.replace(secret, "[redacted]"),
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| redact_json(value, secret)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|value| redact_json(value, secret)),
        _ => {}
    }
}

fn transport_error(error: reqwest::Error) -> BackendError {
    if error.is_timeout() {
        BackendError::new(BackendErrorKind::Timeout, "backend query timed out")
    } else {
        BackendError::new(
            BackendErrorKind::Unavailable,
            "backend query transport failed",
        )
    }
}

pub(crate) fn validated_endpoint(value: &str) -> Result<Url, BackendError> {
    let url = Url::parse(value).map_err(|_| {
        BackendError::new(
            BackendErrorKind::InvalidEndpoint,
            "backend endpoint must be an absolute HTTP(S) URL",
        )
    })?;
    let valid = matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && (url.scheme() == "https" || is_loopback(&url));
    if valid {
        Ok(url)
    } else {
        Err(BackendError::new(
            BackendErrorKind::InvalidEndpoint,
            "backend endpoint must use HTTPS or loopback HTTP without credentials, query, or fragment",
        ))
    }
}

fn is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };

    fn config(url: String, credential_env: Option<&str>) -> BackendConfig {
        BackendConfig {
            url,
            credential_env: credential_env.map(str::to_owned),
        }
    }

    fn server(status: u16, body: Vec<u8>) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).expect("read test request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            tx.send(String::from_utf8_lossy(&request).into_owned())
                .expect("send captured request");
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("write test headers");
            stream.write_all(&body).expect("write test body");
        });
        (format!("http://{address}"), rx)
    }

    #[tokio::test]
    async fn bearer_is_injected_but_never_appears_in_safe_errors() {
        let secret = "observability-test-secret";
        let (url, request) = server(401, format!("{{\"token\":\"{secret}\"}}").into_bytes());
        let client =
            BackendClient::with_credential(&config(url, Some("TEST_TOKEN")), 5, 1024, |_| {
                Ok(Some(secret.into()))
            })
            .expect("client");
        let error = client.get_json("api/test", &[]).await.expect_err("401");
        assert_eq!(error.kind, BackendErrorKind::Unauthorized);
        let request = request.recv().expect("request").to_ascii_lowercase();
        assert!(request.contains(&format!("authorization: bearer {secret}")));
        let serialized = serde_json::to_string(&error).expect("serialize error");
        assert!(!serialized.contains(secret));
        assert!(!serialized.contains("token"));
    }

    #[tokio::test]
    async fn successful_payload_cannot_echo_the_bearer_secret() {
        let secret = "observability-test-secret";
        let (url, _request) = server(
            200,
            serde_json::to_vec(&serde_json::json!({"line": format!("Bearer {secret}")}))
                .expect("test JSON"),
        );
        let client =
            BackendClient::with_credential(&config(url, Some("TEST_TOKEN")), 5, 1024, |_| {
                Ok(Some(secret.into()))
            })
            .expect("client");
        let value = client.get_json("api/test", &[]).await.expect("JSON");
        assert_eq!(value["line"], "Bearer [redacted]");
        assert!(!value.to_string().contains(secret));
    }

    #[tokio::test]
    async fn chunked_response_is_bounded() {
        let (url, _request) = server(200, b"1234567890".to_vec());
        let client =
            BackendClient::with_credential(&config(url, None), 5, 8, |_| Ok(None)).expect("client");
        let error = client.get_json("api/test", &[]).await.expect_err("cap");
        assert_eq!(error.kind, BackendErrorKind::ResponseTooLarge);
    }

    #[tokio::test]
    async fn missing_credential_is_a_backend_local_error() {
        let client = BackendClient::with_credential(
            &config("https://tempo.example.com".into(), Some("MISSING_TOKEN")),
            5,
            1024,
            |_| {
                Err(BackendError::new(
                    BackendErrorKind::Authentication,
                    "credential environment variable is unavailable or invalid",
                ))
            },
        )
        .expect("client construction remains independent");
        let error = client.get_json("api/search", &[]).await.expect_err("auth");
        assert_eq!(error.kind, BackendErrorKind::Authentication);
    }

    #[test]
    fn endpoints_allow_only_https_or_loopback_http() {
        assert!(validated_endpoint("https://tempo.example.com/base").is_ok());
        assert!(validated_endpoint("http://127.0.0.1:3200").is_ok());
        assert!(validated_endpoint("http://tempo.example.com").is_err());
        assert!(validated_endpoint("https://user:secret@example.com").is_err());
        assert!(validated_endpoint("https://example.com/?token=secret").is_err());
    }
}
