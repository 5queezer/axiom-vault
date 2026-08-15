//! Session-scoped credentials and HTTP Basic authentication.

use std::fmt;

use axum::http::header::AUTHORIZATION;
use axum::http::HeaderMap;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use rand::RngExt;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

/// Username used by WebDAV clients for HTTP Basic authentication.
pub const WEBDAV_USERNAME: &str = "axiomvault";

/// A random password valid only for one [`crate::WebDavServer`] instance.
pub struct WebDavCredential {
    password: Zeroizing<String>,
}

impl WebDavCredential {
    /// Generate a credential containing 256 bits of cryptographic randomness.
    pub fn generate() -> Self {
        let mut random = Zeroizing::new([0_u8; 32]);
        rand::rng().fill(&mut random[..]);
        Self {
            password: Zeroizing::new(URL_SAFE_NO_PAD.encode(&random[..])),
        }
    }

    /// Return the fixed username accepted by WebDAV clients.
    pub const fn username(&self) -> &'static str {
        WEBDAV_USERNAME
    }

    /// Expose the password for intentional one-time display to the local user.
    pub fn password(&self) -> &str {
        self.password.as_str()
    }

    pub(crate) fn authorizes(&self, headers: &HeaderMap) -> bool {
        let Some(encoded) = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Basic "))
        else {
            return false;
        };

        let Ok(decoded) = STANDARD.decode(encoded) else {
            return false;
        };
        let decoded = Zeroizing::new(decoded);
        let Some(separator) = decoded.iter().position(|byte| *byte == b':') else {
            return false;
        };
        let (username, password_with_separator) = decoded.split_at(separator);
        let password = &password_with_separator[1..];

        username == WEBDAV_USERNAME.as_bytes()
            && password.len() == self.password.len()
            && bool::from(password.ct_eq(self.password.as_bytes()))
    }
}

impl fmt::Debug for WebDavCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebDavCredential")
            .field("username", &WEBDAV_USERNAME)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_credentials_are_random_and_cli_safe() {
        let first = WebDavCredential::generate();
        let second = WebDavCredential::generate();

        assert_eq!(first.password().len(), 43);
        assert!(first
            .password()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')));
        assert_ne!(first.password(), second.password());
    }

    #[test]
    fn debug_output_redacts_password() {
        let credential = WebDavCredential::generate();
        let debug = format!("{credential:?}");

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(credential.password()));
    }
}
