//! WebDAV server for AxiomVault.
//!
//! Serves decrypted vault contents over a local loopback connection as a
//! fallback when FUSE is unavailable. Files are transparently encrypted
//! on write and decrypted on read.
//!
//! # Security
//! - Binds only to `127.0.0.1` (never `0.0.0.0`)
//! - No authentication layer (vault is already password-protected)
//! - All operations go through `VaultOperations` which handles encryption

pub mod config;
pub mod handler;
pub mod xml;

use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;
use tracing::debug;

use axiomvault_common::Result;
use axiomvault_vault::VaultSession;

pub use config::WebDavConfig;
use handler::AppState;

/// WebDAV server that exposes a vault over HTTP on the loopback interface.
pub struct WebDavServer {
    session: Arc<VaultSession>,
    config: WebDavConfig,
}

impl WebDavServer {
    /// Create a new WebDAV server.
    pub fn new(session: Arc<VaultSession>, config: WebDavConfig) -> Self {
        Self { session, config }
    }

    /// Start the server. Runs until the future is cancelled or the listener fails.
    ///
    /// This is designed to be spawned as a tokio task or selected alongside
    /// a shutdown signal.
    pub async fn start(&self) -> Result<()> {
        let state = AppState {
            session: self.session.clone(),
            max_body_size: self.config.max_body_size,
        };

        let app = Router::new()
            .fallback(handler::handle_request)
            .with_state(state);

        let addr = self.config.socket_addr();
        debug!("Binding WebDAV server");

        let listener = TcpListener::bind(&addr).await?;

        axum::serve(listener, app).await.map_err(|e| {
            axiomvault_common::Error::Io(std::io::Error::other(format!(
                "WebDAV server error: {}",
                e
            )))
        })
    }

    /// Return the URL the server will listen on (before starting).
    pub fn url(&self) -> String {
        format!("http://{}", self.config.socket_addr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiomvault_common::VaultId;
    use axiomvault_crypto::KdfParams;
    use axiomvault_storage::{MemoryProvider, StorageProvider};
    use axiomvault_vault::{VaultConfig, VaultOperations, VaultTree};
    use http::Method;

    /// Helper to create a test session with an in-memory storage provider.
    async fn create_test_session() -> Arc<VaultSession> {
        let id = VaultId::new("test").unwrap();
        let password = b"test-password";
        let params = KdfParams::moderate();
        let creation =
            VaultConfig::new(id, password, "memory", serde_json::Value::Null, params).unwrap();

        let provider = Arc::new(MemoryProvider::new());

        provider
            .create_dir(&axiomvault_common::VaultPath::parse("/d").unwrap())
            .await
            .unwrap();

        provider
            .create_dir(&axiomvault_common::VaultPath::parse("/m").unwrap())
            .await
            .unwrap();

        let session =
            VaultSession::unlock(creation.config, password, provider, VaultTree::new()).unwrap();
        Arc::new(session)
    }

    /// Find a free port for testing.
    async fn free_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    }

    /// Start a WebDAV server in the background and return its base URL.
    async fn start_test_server(session: Arc<VaultSession>) -> String {
        let port = free_port().await;
        let config = WebDavConfig {
            bind_address: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let server = WebDavServer::new(session, config);
        let url = server.url();
        tokio::spawn(async move {
            let _ = server.start().await;
        });
        // Give the server a moment to bind
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        url
    }

    #[tokio::test]
    async fn test_propfind_root() {
        let session = create_test_session().await;

        // Create a file so root is non-empty
        let ops = VaultOperations::new(&session).unwrap();
        ops.create_file(
            &axiomvault_common::VaultPath::parse("/hello.txt").unwrap(),
            b"Hello WebDAV",
        )
        .await
        .unwrap();

        let base_url = start_test_server(session).await;

        let client = reqwest::Client::new();
        let resp = client
            .request(Method::from_bytes(b"PROPFIND").unwrap(), &base_url)
            .header("Depth", "1")
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 207);
        let body = resp.text().await.unwrap();
        assert!(body.contains("<D:multistatus"));
        assert!(body.contains("hello.txt"));
    }

    #[tokio::test]
    async fn test_get_file() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();
        ops.create_file(
            &axiomvault_common::VaultPath::parse("/test.txt").unwrap(),
            b"decrypted content",
        )
        .await
        .unwrap();

        let base_url = start_test_server(session).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/test.txt", base_url))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body = resp.bytes().await.unwrap();
        assert_eq!(&body[..], b"decrypted content");
    }

    #[tokio::test]
    async fn test_put_create_file() {
        let session = create_test_session().await;
        let base_url = start_test_server(session.clone()).await;

        let client = reqwest::Client::new();

        // PUT a new file
        let resp = client
            .put(format!("{}/newfile.txt", base_url))
            .body("new content")
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 201);

        // Verify we can read it back
        let resp = client
            .get(format!("{}/newfile.txt", base_url))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        assert_eq!(resp.bytes().await.unwrap().as_ref(), b"new content");
    }

    #[tokio::test]
    async fn test_put_update_file() {
        let session = create_test_session().await;

        let ops = VaultOperations::new(&session).unwrap();
        ops.create_file(
            &axiomvault_common::VaultPath::parse("/existing.txt").unwrap(),
            b"old content",
        )
        .await
        .unwrap();

        let base_url = start_test_server(session).await;

        let client = reqwest::Client::new();
        let resp = client
            .put(format!("{}/existing.txt", base_url))
            .body("updated content")
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 204);

        let resp = client
            .get(format!("{}/existing.txt", base_url))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        assert_eq!(resp.bytes().await.unwrap().as_ref(), b"updated content");
    }

    #[tokio::test]
    async fn test_delete_file() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();
        ops.create_file(
            &axiomvault_common::VaultPath::parse("/todelete.txt").unwrap(),
            b"delete me",
        )
        .await
        .unwrap();

        let base_url = start_test_server(session).await;

        let client = reqwest::Client::new();
        let resp = client
            .delete(format!("{}/todelete.txt", base_url))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 204);

        // Verify it's gone
        let resp = client
            .get(format!("{}/todelete.txt", base_url))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn test_mkcol_create_directory() {
        let session = create_test_session().await;
        let base_url = start_test_server(session).await;

        let client = reqwest::Client::new();
        let resp = client
            .request(
                Method::from_bytes(b"MKCOL").unwrap(),
                format!("{}/newdir", base_url),
            )
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 201);

        // Verify directory exists via PROPFIND
        let resp = client
            .request(
                Method::from_bytes(b"PROPFIND").unwrap(),
                format!("{}/newdir", base_url),
            )
            .header("Depth", "0")
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 207);
        let body = resp.text().await.unwrap();
        assert!(body.contains("<D:collection/>"));
    }

    #[tokio::test]
    async fn test_options() {
        let session = create_test_session().await;
        let base_url = start_test_server(session).await;

        let client = reqwest::Client::new();
        let resp = client
            .request(Method::OPTIONS, &base_url)
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let allow = resp.headers().get("allow").unwrap().to_str().unwrap();
        assert!(allow.contains("PROPFIND"));
        assert!(allow.contains("GET"));
        assert!(allow.contains("PUT"));
        let dav = resp.headers().get("dav").unwrap().to_str().unwrap();
        assert_eq!(dav, "1");
    }

    #[tokio::test]
    async fn test_unsupported_methods_return_501() {
        let session = create_test_session().await;
        let base_url = start_test_server(session).await;

        let client = reqwest::Client::new();

        for method_name in &["MOVE", "COPY", "LOCK", "UNLOCK"] {
            let resp = client
                .request(
                    Method::from_bytes(method_name.as_bytes()).unwrap(),
                    format!("{}/test", base_url),
                )
                .send()
                .await
                .unwrap();

            assert_eq!(resp.status(), 501, "{} should return 501", method_name,);
        }
    }

    #[tokio::test]
    async fn test_head_file() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();
        ops.create_file(
            &axiomvault_common::VaultPath::parse("/headtest.txt").unwrap(),
            b"head content",
        )
        .await
        .unwrap();

        let base_url = start_test_server(session).await;

        let client = reqwest::Client::new();
        let resp = client
            .head(format!("{}/headtest.txt", base_url))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "text/plain");
    }
}
