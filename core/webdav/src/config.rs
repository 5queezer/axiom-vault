//! WebDAV server configuration.

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
        format!("{}:{}", self.bind_address, self.port)
    }
}
