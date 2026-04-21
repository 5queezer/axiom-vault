//! Shared HTTP client utilities for cloud storage providers.
//!
//! All cloud providers need to:
//! - Build an HTTP client with consistent settings
//! - Map HTTP status codes to [`axiomvault_common::Error`] variants
//! - Construct Bearer authorization headers from access tokens
//!
//! This module consolidates those patterns so individual providers
//! don't duplicate them.

use std::time::Duration;

use reqwest::{Client, StatusCode};

use axiomvault_common::Error;

/// User-Agent string for all cloud API requests.
const USER_AGENT: &str = "AxiomVault/0.1";

/// Total request timeout used by [`build_metadata_http_client`].
///
/// Metadata calls (small JSON requests) are expected to complete quickly;
/// 30 seconds is well above normal latency but bounds the impact of a
/// hung or slow-loris server.
const METADATA_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Build an HTTP client with standard settings for cloud API usage.
///
/// Configures a consistent User-Agent header and connection timeout (10s).
/// No total request timeout is set because this client is also used for
/// streaming uploads and downloads that may legitimately take many minutes
/// (large files, slow uplinks). For small metadata calls use
/// [`build_metadata_http_client`] instead, which adds a bounded
/// per-request timeout to limit DoS / hang exposure.
pub fn build_http_client() -> Result<Client, Error> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| Error::Network(format!("Failed to create HTTP client: {}", e)))
}

/// Build an HTTP client suitable for short-lived metadata requests.
///
/// Identical to [`build_http_client`] but adds a 30-second total request
/// timeout. Use this client for JSON metadata calls (list, get, delete,
/// move, copy, create-folder, session-init, etc.) where a hang would
/// otherwise stall the whole sync indefinitely. Do **not** use it for
/// streaming uploads or downloads — those need to be unbounded so that
/// large transfers can complete.
pub fn build_metadata_http_client() -> Result<Client, Error> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(METADATA_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| Error::Network(format!("Failed to create HTTP client: {}", e)))
}

/// Format an access token as a Bearer authorization header value.
pub fn bearer_header(token: &str) -> String {
    format!("Bearer {}", token)
}

/// Map an HTTP status code and response body to the appropriate
/// [`axiomvault_common::Error`] variant.
///
/// This handles the common status-to-error mapping shared across
/// Google Drive, Dropbox, and OneDrive:
///
/// | Status | Error variant |
/// |--------|--------------|
/// | 401    | `AuthenticationExpired` (transient — token needs refresh) |
/// | 403    | `NotPermitted` |
/// | 404    | `NotFound` |
/// | 409    | `AlreadyExists` |
/// | other  | `Network` |
pub fn map_status_error(status: StatusCode, body: &str) -> Error {
    if status == StatusCode::NOT_FOUND {
        Error::NotFound(format!("Resource not found: {}", body))
    } else if status == StatusCode::UNAUTHORIZED {
        // A 401 is the textbook transient auth case: the access token
        // is no longer accepted, but the refresh token is (probably)
        // still good. Surface as `AuthenticationExpired` so the retry
        // executor gives the token manager a chance to refresh.
        Error::AuthenticationExpired("Invalid or expired token".to_string())
    } else if status == StatusCode::FORBIDDEN {
        Error::NotPermitted("Access denied".to_string())
    } else if status == StatusCode::CONFLICT {
        Error::AlreadyExists(format!("Resource conflict: {}", body))
    } else {
        Error::Network(format!("API error: {} - {}", status, body))
    }
}

/// Handle a `reqwest::Response`, deserializing the JSON body on success
/// or mapping the status code to an error on failure.
pub async fn handle_json_response<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> axiomvault_common::Result<T> {
    let status = response.status();

    if status.is_success() {
        response
            .json()
            .await
            .map_err(|e| Error::Network(format!("Failed to parse response: {}", e)))
    } else {
        let body = response
            .text()
            .await
            .map_err(|e| Error::Network(format!("Failed to read error response: {}", e)))?;
        Err(map_status_error(status, &body))
    }
}

/// Handle a `reqwest::Response` that returns an error, consuming the
/// response and mapping to an appropriate error. Always returns `Err`.
pub async fn handle_error_response<T>(response: reqwest::Response) -> axiomvault_common::Result<T> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| Error::Network(format!("Failed to read error response: {}", e)))?;
    Err(map_status_error(status, &body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bearer_header() {
        assert_eq!(bearer_header("abc123"), "Bearer abc123");
    }

    #[test]
    fn test_map_status_not_found() {
        let err = map_status_error(StatusCode::NOT_FOUND, "file gone");
        match err {
            Error::NotFound(msg) => assert!(msg.contains("file gone")),
            other => panic!("Expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_map_status_unauthorized() {
        let err = map_status_error(StatusCode::UNAUTHORIZED, "bad token");
        assert!(matches!(err, Error::AuthenticationExpired(_)));
    }

    #[test]
    fn test_map_status_forbidden() {
        let err = map_status_error(StatusCode::FORBIDDEN, "nope");
        assert!(matches!(err, Error::NotPermitted(_)));
    }

    #[test]
    fn test_map_status_conflict() {
        let err = map_status_error(StatusCode::CONFLICT, "exists");
        match err {
            Error::AlreadyExists(msg) => assert!(msg.contains("exists")),
            other => panic!("Expected AlreadyExists, got: {:?}", other),
        }
    }

    #[test]
    fn test_map_status_other() {
        let err = map_status_error(StatusCode::INTERNAL_SERVER_ERROR, "oops");
        match err {
            Error::Network(msg) => {
                assert!(msg.contains("500"));
                assert!(msg.contains("oops"));
            }
            other => panic!("Expected Network, got: {:?}", other),
        }
    }

    #[test]
    fn test_build_http_client() {
        let client = build_http_client();
        assert!(client.is_ok());
    }

    #[test]
    fn test_build_metadata_http_client() {
        let client = build_metadata_http_client();
        assert!(client.is_ok());
    }

    /// The bounded metadata client must surface a timeout (rather than
    /// hang indefinitely) when a server accepts the connection but
    /// never sends a response. We verify behaviour against a local TCP
    /// listener that does nothing after `accept()` — `reqwest::Client`
    /// does not expose its configured timeout, so we exercise it.
    #[tokio::test]
    async fn test_metadata_http_client_times_out() {
        use std::time::Duration;
        use tokio::net::TcpListener;

        // Bind a local listener and accept-but-never-respond. The
        // request should resolve as a timeout error within ~200ms.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _accept_task = tokio::spawn(async move {
            // Accept once and hold the connection open without writing.
            if let Ok((stream, _)) = listener.accept().await {
                // Keep the stream alive for the duration of the test.
                tokio::time::sleep(Duration::from_secs(5)).await;
                drop(stream);
            }
        });

        // Replicate the production builder but with a short timeout so
        // the test is fast.
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_millis(200))
            .build()
            .expect("client builds");

        let url = format!("http://{}/", addr);
        let result = tokio::time::timeout(Duration::from_secs(2), client.get(&url).send()).await;

        // Outer guard must not fire — the bounded client should have
        // returned its own timeout well before 2s elapsed.
        let inner = result.expect("bounded client did not hang past its own timeout");
        assert!(
            inner.is_err(),
            "request should have failed (timeout), got: {:?}",
            inner.map(|r| r.status())
        );
    }
}
