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

/// Build an HTTP client with standard settings for cloud API usage.
///
/// Configures a consistent User-Agent header, connection timeout (10s),
/// and request timeout (30s).
pub fn build_http_client() -> Result<Client, Error> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
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
/// | 401    | `Authentication` |
/// | 403    | `NotPermitted` |
/// | 404    | `NotFound` |
/// | 409    | `AlreadyExists` |
/// | other  | `Network` |
pub fn map_status_error(status: StatusCode, body: &str) -> Error {
    if status == StatusCode::NOT_FOUND {
        Error::NotFound(format!("Resource not found: {}", body))
    } else if status == StatusCode::UNAUTHORIZED {
        Error::Authentication("Invalid or expired token".to_string())
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
        let body = response.text().await.unwrap_or_default();
        Err(map_status_error(status, &body))
    }
}

/// Handle a `reqwest::Response` that returns an error, consuming the
/// response and mapping to an appropriate error. Always returns `Err`.
pub async fn handle_error_response<T>(response: reqwest::Response) -> axiomvault_common::Result<T> {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
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
        assert!(matches!(err, Error::Authentication(_)));
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
}
