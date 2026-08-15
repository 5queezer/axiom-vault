//! WebDAV server for AxiomVault.
//!
//! Serves decrypted vault contents over a local loopback connection as a
//! fallback when FUSE is unavailable. Files are transparently encrypted
//! on write and decrypted on read.
//!
//! # Security
//! - Binds only to loopback IP addresses
//! - Requires a random, session-scoped HTTP Basic credential
//! - All operations go through `VaultOperations` which handles encryption

pub mod auth;
pub mod config;
pub mod handler;
pub mod xml;

use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;
use tracing::debug;

use axiomvault_common::Result;
use axiomvault_vault::VaultSession;

pub use auth::{WebDavCredential, WEBDAV_USERNAME};
pub use config::WebDavConfig;
use handler::AppState;

/// WebDAV server that exposes a vault over HTTP on the loopback interface.
pub struct WebDavServer {
    session: Arc<VaultSession>,
    config: WebDavConfig,
    credential: Arc<WebDavCredential>,
}

impl WebDavServer {
    /// Create a new WebDAV server.
    pub fn new(session: Arc<VaultSession>, config: WebDavConfig) -> Self {
        Self {
            session,
            config,
            credential: Arc::new(WebDavCredential::generate()),
        }
    }

    /// Return the random credential for this server session.
    pub fn credential(&self) -> &WebDavCredential {
        &self.credential
    }

    /// Start the server. Runs until the future is cancelled or the listener fails.
    ///
    /// This is designed to be spawned as a tokio task or selected alongside
    /// a shutdown signal.
    pub async fn start(&self) -> Result<()> {
        let state = AppState {
            session: self.session.clone(),
            max_body_size: self.config.max_body_size,
            credential: self.credential.clone(),
        };

        let app = Router::new()
            .fallback(handler::handle_request)
            .with_state(state);

        let addr = self.config.validated_socket_addr()?;
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
    use base64::Engine;
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

    /// Start a WebDAV server in the background and return its URL and password.
    async fn start_test_server(session: Arc<VaultSession>) -> (String, String) {
        let port = free_port().await;
        let config = WebDavConfig {
            bind_address: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let server = WebDavServer::new(session, config);
        let url = server.url();
        let password = server.credential().password().to_owned();
        tokio::spawn(async move {
            let _ = server.start().await;
        });
        // Give the server a moment to bind
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        (url, password)
    }

    fn authenticated_client(password: &str) -> reqwest::Client {
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", WEBDAV_USERNAME, password));
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Basic {encoded}").parse().unwrap(),
        );
        reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn non_loopback_bind_addresses_are_rejected_by_start() {
        let session = create_test_session().await;

        for bind_address in ["0.0.0.0", "::", "192.168.1.10", "203.0.113.10"] {
            let server = WebDavServer::new(
                session.clone(),
                WebDavConfig {
                    bind_address: bind_address.to_owned(),
                    port: 0,
                    ..Default::default()
                },
            );

            let error = tokio::time::timeout(std::time::Duration::from_millis(100), server.start())
                .await
                .expect("non-loopback validation must happen before binding")
                .unwrap_err();

            assert!(
                matches!(error, axiomvault_common::Error::InvalidInput(_)),
                "{bind_address} returned {error:?}"
            );
        }
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

        let (base_url, password) = start_test_server(session).await;

        let client = authenticated_client(&password);
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
    async fn unauthenticated_vault_methods_are_rejected() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();
        ops.create_file(
            &axiomvault_common::VaultPath::parse("/test.txt").unwrap(),
            b"decrypted content",
        )
        .await
        .unwrap();

        let (base_url, _) = start_test_server(session.clone()).await;
        let client = reqwest::Client::new();

        for (method, path) in [
            ("PROPFIND", "/"),
            ("GET", "/test.txt"),
            ("HEAD", "/test.txt"),
            ("PUT", "/test.txt"),
            ("MKCOL", "/new-directory"),
            ("DELETE", "/test.txt"),
        ] {
            let response = client
                .request(
                    Method::from_bytes(method.as_bytes()).unwrap(),
                    format!("{base_url}{path}"),
                )
                .body("attacker-controlled")
                .send()
                .await
                .unwrap();

            assert_eq!(
                response.status(),
                401,
                "{method} must require authentication"
            );
            assert_eq!(
                response
                    .headers()
                    .get("www-authenticate")
                    .and_then(|value| value.to_str().ok()),
                Some("Basic realm=\"AxiomVault WebDAV\"")
            );
        }

        assert_eq!(
            ops.read_file(&axiomvault_common::VaultPath::parse("/test.txt").unwrap())
                .await
                .unwrap(),
            b"decrypted content"
        );
        assert!(
            !ops.exists(&axiomvault_common::VaultPath::parse("/new-directory").unwrap())
                .await
        );
    }

    #[tokio::test]
    async fn wrong_password_is_rejected() {
        let session = create_test_session().await;
        let (base_url, _) = start_test_server(session).await;

        let response = authenticated_client("wrong-password")
            .get(format!("{base_url}/test.txt"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 401);
    }

    #[tokio::test]
    async fn correct_password_allows_decrypted_get() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();
        ops.create_file(
            &axiomvault_common::VaultPath::parse("/secret.txt").unwrap(),
            b"decrypted content",
        )
        .await
        .unwrap();
        let (base_url, password) = start_test_server(session).await;

        let response = authenticated_client(&password)
            .get(format!("{base_url}/secret.txt"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(
            response.bytes().await.unwrap(),
            b"decrypted content".as_slice()
        );
    }

    #[tokio::test]
    async fn test_put_create_file() {
        let session = create_test_session().await;
        let (base_url, password) = start_test_server(session.clone()).await;

        let client = authenticated_client(&password);

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

        let (base_url, password) = start_test_server(session).await;

        let client = authenticated_client(&password);
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

        let (base_url, password) = start_test_server(session).await;

        let client = authenticated_client(&password);
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
        let (base_url, password) = start_test_server(session).await;

        let client = authenticated_client(&password);
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
        let (base_url, password) = start_test_server(session).await;

        let client = authenticated_client(&password);
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
        let (base_url, password) = start_test_server(session).await;

        let client = authenticated_client(&password);

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

        let (base_url, password) = start_test_server(session).await;

        let client = authenticated_client(&password);
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
