//! WebDAV server configuration.

use std::net::{IpAddr, SocketAddr};

use axiomvault_common::{Error, Result};

/// Configuration for the WebDAV server.
#[derive(Debug, Clone)]
pub struct WebDavConfig {
    /// Address to bind to. Must be loopback only for security.
    pub bind_address: String,
    /// Port to listen on.
    pub port: u16,
    /// Maximum request body size in bytes (default: 256 MiB).
    pub max_body_size: usize,
}

impl Default for WebDavConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1".to_string(),
            port: 8080,
            max_body_size: 256 * 1024 * 1024, // 256 MiB
        }
    }
}

impl WebDavConfig {
    /// Build a socket address string from bind_address and port.
    pub fn socket_addr(&self) -> String {
        self.bind_address
            .parse::<IpAddr>()
            .map(|ip| SocketAddr::new(ip, self.port).to_string())
            .unwrap_or_else(|_| format!("{}:{}", self.bind_address, self.port))
    }

    /// Parse and enforce the library's loopback-only binding invariant.
    pub(crate) fn validated_socket_addr(&self) -> Result<SocketAddr> {
        let ip = self.bind_address.parse::<IpAddr>().map_err(|_| {
            Error::InvalidInput("WebDAV bind address must be a loopback IP address".to_string())
        })?;

        if !ip.is_loopback() {
            return Err(Error::InvalidInput(
                "WebDAV bind address must be loopback".to_string(),
            ));
        }

        Ok(SocketAddr::new(ip, self.port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_ip_loopback_families_are_valid() {
        for bind_address in ["127.0.0.1", "::1"] {
            let config = WebDavConfig {
                bind_address: bind_address.to_owned(),
                ..Default::default()
            };
            assert!(config.validated_socket_addr().unwrap().ip().is_loopback());
        }
    }

    #[test]
    fn hostnames_are_rejected_instead_of_resolved() {
        let config = WebDavConfig {
            bind_address: "localhost".to_owned(),
            ..Default::default()
        };
        assert!(matches!(
            config.validated_socket_addr(),
            Err(Error::InvalidInput(_))
        ));
    }
}
