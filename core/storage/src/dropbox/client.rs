//! Dropbox API client.

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::Stream;
use reqwest::{header, Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::Arc;

use axiomvault_common::{Error, Result};

use super::auth::DropboxTokenManager;

/// Dropbox API base URL for metadata operations.
const API_BASE: &str = "https://api.dropboxapi.com/2";
/// Dropbox content API base URL for file content operations.
const CONTENT_BASE: &str = "https://content.dropboxapi.com/2";
/// Chunk size for upload sessions (8 MB).
const CHUNK_SIZE: usize = 8 * 1024 * 1024;

/// Dropbox file metadata from API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropboxMetadata {
    /// Discriminator tag: "file" or "folder".
    #[serde(rename = ".tag")]
    pub tag: String,
    /// File/folder name.
    pub name: String,
    /// Unique ID.
    #[serde(default)]
    pub id: String,
    /// Lowercase path.
    #[serde(default)]
    pub path_lower: Option<String>,
    /// Display path.
    #[serde(default)]
    pub path_display: Option<String>,
    /// File size in bytes (files only).
    #[serde(default)]
    pub size: Option<u64>,
    /// Server-modified timestamp (files only).
    #[serde(default)]
    pub server_modified: Option<DateTime<Utc>>,
    /// Revision hash (files only, used as etag).
    #[serde(default)]
    pub rev: Option<String>,
    /// Content hash (files only).
    #[serde(default)]
    pub content_hash: Option<String>,
}

impl DropboxMetadata {
    /// Check if this entry is a folder.
    pub fn is_folder(&self) -> bool {
        self.tag == "folder"
    }
}

/// Response from list_folder / list_folder/continue.
#[derive(Debug, Deserialize)]
pub struct ListFolderResult {
    /// Directory entries.
    pub entries: Vec<DropboxMetadata>,
    /// Cursor for pagination.
    pub cursor: String,
    /// Whether there are more entries.
    pub has_more: bool,
}

/// Response from upload session start.
#[derive(Debug, Deserialize)]
pub struct UploadSessionStart {
    /// Session ID for appending chunks.
    pub session_id: String,
}

/// Dropbox API client.
pub struct DropboxClient {
    http: Client,
    token_manager: Arc<DropboxTokenManager>,
}

impl DropboxClient {
    /// Create a new Dropbox API client.
    pub fn new(token_manager: Arc<DropboxTokenManager>) -> Self {
        Self {
            http: Client::new(),
            token_manager,
        }
    }

    /// Get an authorization header value.
    async fn auth_header(&self) -> Result<String> {
        let token = self.token_manager.get_access_token().await?;
        Ok(format!("Bearer {}", token))
    }

    /// Handle a Dropbox API error response.
    fn handle_error(status: StatusCode, body: &str) -> Error {
        if status == StatusCode::CONFLICT {
            // Dropbox returns 409 for path errors
            if body.contains("not_found") {
                return Error::NotFound(format!("Path not found: {}", body));
            }
            if body.contains("conflict") || body.contains("already_exists") {
                return Error::AlreadyExists(format!("Path conflict: {}", body));
            }
        }
        if status == StatusCode::UNAUTHORIZED {
            return Error::Authentication("Dropbox authentication failed".to_string());
        }
        Error::Storage(format!("Dropbox API error ({}): {}", status, body))
    }

    /// Get metadata for a file or folder.
    pub async fn get_metadata(&self, path: &str) -> Result<DropboxMetadata> {
        let auth = self.auth_header().await?;
        let resp = self
            .http
            .post(format!("{}/files/get_metadata", API_BASE))
            .header(header::AUTHORIZATION, &auth)
            .header(header::CONTENT_TYPE, "application/json")
            .json(&serde_json::json!({ "path": path }))
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Dropbox request failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(Self::handle_error(status, &body));
        }

        resp.json::<DropboxMetadata>()
            .await
            .map_err(|e| Error::Storage(format!("Failed to parse Dropbox response: {}", e)))
    }

    /// List folder contents with automatic pagination.
    pub async fn list_folder(&self, path: &str) -> Result<Vec<DropboxMetadata>> {
        let auth = self.auth_header().await?;

        // Dropbox uses empty string for root, not "/"
        let list_path = if path == "/" { "" } else { path };

        let resp = self
            .http
            .post(format!("{}/files/list_folder", API_BASE))
            .header(header::AUTHORIZATION, &auth)
            .header(header::CONTENT_TYPE, "application/json")
            .json(&serde_json::json!({
                "path": list_path,
                "recursive": false,
                "include_deleted": false,
            }))
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Dropbox request failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(Self::handle_error(status, &body));
        }

        let mut result: ListFolderResult = resp
            .json()
            .await
            .map_err(|e| Error::Storage(format!("Failed to parse response: {}", e)))?;

        let mut entries = result.entries;

        // Handle pagination
        while result.has_more {
            let auth = self.auth_header().await?;
            let resp = self
                .http
                .post(format!("{}/files/list_folder/continue", API_BASE))
                .header(header::AUTHORIZATION, &auth)
                .header(header::CONTENT_TYPE, "application/json")
                .json(&serde_json::json!({ "cursor": result.cursor }))
                .send()
                .await
                .map_err(|e| Error::Storage(format!("Dropbox request failed: {}", e)))?;

            let status = resp.status();
            if !status.is_success() {
                let body = resp
                    .text()
                    .await
                    .unwrap_or_else(|_| "unknown error".to_string());
                return Err(Self::handle_error(status, &body));
            }

            result = resp
                .json()
                .await
                .map_err(|e| Error::Storage(format!("Failed to parse response: {}", e)))?;

            entries.extend(result.entries);
        }

        Ok(entries)
    }

    /// Upload a file (simple upload for files up to 150 MB).
    pub async fn upload(&self, path: &str, data: Vec<u8>) -> Result<DropboxMetadata> {
        let auth = self.auth_header().await?;
        let api_arg = serde_json::json!({
            "path": path,
            "mode": "overwrite",
            "autorename": false,
            "mute": false,
        });

        let resp = self
            .http
            .post(format!("{}/files/upload", CONTENT_BASE))
            .header(header::AUTHORIZATION, &auth)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header("Dropbox-API-Arg", api_arg.to_string())
            .body(data)
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Dropbox upload failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(Self::handle_error(status, &body));
        }

        resp.json::<DropboxMetadata>()
            .await
            .map_err(|e| Error::Storage(format!("Failed to parse upload response: {}", e)))
    }

    /// Upload a large file using upload sessions (chunked).
    pub async fn upload_session(&self, path: &str, data: Vec<u8>) -> Result<DropboxMetadata> {
        let total_size = data.len();
        let chunks: Vec<&[u8]> = data.chunks(CHUNK_SIZE).collect();

        if chunks.is_empty() {
            return self.upload(path, data).await;
        }

        // Start session
        let auth = self.auth_header().await?;
        let resp = self
            .http
            .post(format!("{}/files/upload_session/start", CONTENT_BASE))
            .header(header::AUTHORIZATION, &auth)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header("Dropbox-API-Arg", "{\"close\": false}")
            .body(chunks[0].to_vec())
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Upload session start failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(Self::handle_error(status, &body));
        }

        let session: UploadSessionStart = resp
            .json()
            .await
            .map_err(|e| Error::Storage(format!("Failed to parse session response: {}", e)))?;

        let mut offset = chunks[0].len();

        // Append remaining chunks (except the last)
        for chunk in &chunks[1..chunks.len() - 1] {
            let auth = self.auth_header().await?;
            let api_arg = serde_json::json!({
                "cursor": {
                    "session_id": session.session_id,
                    "offset": offset,
                },
                "close": false,
            });

            let resp = self
                .http
                .post(format!("{}/files/upload_session/append_v2", CONTENT_BASE))
                .header(header::AUTHORIZATION, &auth)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header("Dropbox-API-Arg", api_arg.to_string())
                .body(chunk.to_vec())
                .send()
                .await
                .map_err(|e| Error::Storage(format!("Upload session append failed: {}", e)))?;

            if !resp.status().is_success() {
                let body = resp
                    .text()
                    .await
                    .unwrap_or_else(|_| "unknown error".to_string());
                return Err(Error::Storage(format!("Upload append failed: {}", body)));
            }

            offset += chunk.len();
        }

        // Finish session with last chunk
        let last_chunk = if chunks.len() > 1 {
            chunks[chunks.len() - 1].to_vec()
        } else {
            Vec::new()
        };

        let auth = self.auth_header().await?;
        let api_arg = serde_json::json!({
            "cursor": {
                "session_id": session.session_id,
                "offset": offset,
            },
            "commit": {
                "path": path,
                "mode": "overwrite",
                "autorename": false,
                "mute": false,
            },
        });

        let resp = self
            .http
            .post(format!("{}/files/upload_session/finish", CONTENT_BASE))
            .header(header::AUTHORIZATION, &auth)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header("Dropbox-API-Arg", api_arg.to_string())
            .body(last_chunk)
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Upload session finish failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(Self::handle_error(status, &body));
        }

        let _ = total_size; // used implicitly via chunks

        resp.json::<DropboxMetadata>()
            .await
            .map_err(|e| Error::Storage(format!("Failed to parse finish response: {}", e)))
    }

    /// Download a file.
    pub async fn download(&self, path: &str) -> Result<Vec<u8>> {
        let auth = self.auth_header().await?;
        let api_arg = serde_json::json!({ "path": path });

        let resp = self
            .http
            .post(format!("{}/files/download", CONTENT_BASE))
            .header(header::AUTHORIZATION, &auth)
            .header("Dropbox-API-Arg", api_arg.to_string())
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Dropbox download failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(Self::handle_error(status, &body));
        }

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| Error::Storage(format!("Failed to read download body: {}", e)))
    }

    /// Download a file as a stream.
    pub async fn download_stream(
        &self,
        path: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<Bytes, Error>> + Send>>> {
        let auth = self.auth_header().await?;
        let api_arg = serde_json::json!({ "path": path });

        let resp = self
            .http
            .post(format!("{}/files/download", CONTENT_BASE))
            .header(header::AUTHORIZATION, &auth)
            .header("Dropbox-API-Arg", api_arg.to_string())
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Dropbox download failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(Self::handle_error(status, &body));
        }

        use futures::StreamExt;
        let stream = resp
            .bytes_stream()
            .map(|r| r.map_err(|e| Error::Storage(format!("Stream error: {}", e))));

        Ok(Box::pin(stream))
    }

    /// Delete a file or folder.
    pub async fn delete(&self, path: &str) -> Result<()> {
        let auth = self.auth_header().await?;
        let resp = self
            .http
            .post(format!("{}/files/delete_v2", API_BASE))
            .header(header::AUTHORIZATION, &auth)
            .header(header::CONTENT_TYPE, "application/json")
            .json(&serde_json::json!({ "path": path }))
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Dropbox delete failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(Self::handle_error(status, &body));
        }

        Ok(())
    }

    /// Create a folder.
    pub async fn create_folder(&self, path: &str) -> Result<DropboxMetadata> {
        let auth = self.auth_header().await?;
        let resp = self
            .http
            .post(format!("{}/files/create_folder_v2", API_BASE))
            .header(header::AUTHORIZATION, &auth)
            .header(header::CONTENT_TYPE, "application/json")
            .json(&serde_json::json!({
                "path": path,
                "autorename": false,
            }))
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Dropbox create folder failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(Self::handle_error(status, &body));
        }

        // create_folder_v2 wraps the metadata in a "metadata" field
        #[derive(Deserialize)]
        struct Wrapper {
            metadata: DropboxMetadata,
        }

        let wrapper: Wrapper = resp
            .json()
            .await
            .map_err(|e| Error::Storage(format!("Failed to parse response: {}", e)))?;

        Ok(wrapper.metadata)
    }

    /// Move a file or folder.
    pub async fn move_entry(&self, from_path: &str, to_path: &str) -> Result<DropboxMetadata> {
        let auth = self.auth_header().await?;
        let resp = self
            .http
            .post(format!("{}/files/move_v2", API_BASE))
            .header(header::AUTHORIZATION, &auth)
            .header(header::CONTENT_TYPE, "application/json")
            .json(&serde_json::json!({
                "from_path": from_path,
                "to_path": to_path,
                "autorename": false,
            }))
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Dropbox move failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(Self::handle_error(status, &body));
        }

        #[derive(Deserialize)]
        struct Wrapper {
            metadata: DropboxMetadata,
        }

        let wrapper: Wrapper = resp
            .json()
            .await
            .map_err(|e| Error::Storage(format!("Failed to parse response: {}", e)))?;

        Ok(wrapper.metadata)
    }

    /// Copy a file or folder.
    pub async fn copy_entry(&self, from_path: &str, to_path: &str) -> Result<DropboxMetadata> {
        let auth = self.auth_header().await?;
        let resp = self
            .http
            .post(format!("{}/files/copy_v2", API_BASE))
            .header(header::AUTHORIZATION, &auth)
            .header(header::CONTENT_TYPE, "application/json")
            .json(&serde_json::json!({
                "from_path": from_path,
                "to_path": to_path,
                "autorename": false,
            }))
            .send()
            .await
            .map_err(|e| Error::Storage(format!("Dropbox copy failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(Self::handle_error(status, &body));
        }

        #[derive(Deserialize)]
        struct Wrapper {
            metadata: DropboxMetadata,
        }

        let wrapper: Wrapper = resp
            .json()
            .await
            .map_err(|e| Error::Storage(format!("Failed to parse response: {}", e)))?;

        Ok(wrapper.metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dropbox_metadata_is_folder() {
        let folder = DropboxMetadata {
            tag: "folder".to_string(),
            name: "test".to_string(),
            id: "id:abc".to_string(),
            path_lower: Some("/test".to_string()),
            path_display: Some("/test".to_string()),
            size: None,
            server_modified: None,
            rev: None,
            content_hash: None,
        };
        assert!(folder.is_folder());

        let file = DropboxMetadata {
            tag: "file".to_string(),
            name: "test.txt".to_string(),
            id: "id:def".to_string(),
            path_lower: Some("/test.txt".to_string()),
            path_display: Some("/test.txt".to_string()),
            size: Some(1024),
            server_modified: Some(Utc::now()),
            rev: Some("rev123".to_string()),
            content_hash: None,
        };
        assert!(!file.is_folder());
    }

    #[test]
    fn test_dropbox_metadata_serialization() {
        let meta = DropboxMetadata {
            tag: "file".to_string(),
            name: "test.txt".to_string(),
            id: "id:abc".to_string(),
            path_lower: Some("/test.txt".to_string()),
            path_display: Some("/test.txt".to_string()),
            size: Some(100),
            server_modified: Some(Utc::now()),
            rev: Some("rev1".to_string()),
            content_hash: Some("hash1".to_string()),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: DropboxMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test.txt");
        assert_eq!(deserialized.tag, "file");
    }
}
