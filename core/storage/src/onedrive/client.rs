//! Microsoft Graph API client for OneDrive.

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use reqwest::{header, Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::Arc;

use axiomvault_common::{Error, Result};

use super::auth::OneDriveTokenManager;
use crate::http_client;

/// Microsoft Graph API base URL for OneDrive.
const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0/me/drive";

/// Chunk size for resumable uploads (must be multiple of 320KB).
const CHUNK_SIZE: usize = 10 * 320 * 1024; // 3.2 MB

/// OneDrive drive item from Microsoft Graph API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveItem {
    /// Item ID.
    #[serde(default)]
    pub id: String,
    /// Item name.
    #[serde(default)]
    pub name: String,
    /// File size in bytes.
    #[serde(default)]
    pub size: Option<u64>,
    /// Last modified date/time.
    #[serde(default)]
    pub last_modified_date_time: Option<DateTime<Utc>>,
    /// ETag for concurrency.
    #[serde(default, rename = "eTag")]
    pub etag: Option<String>,
    /// File facet (present if item is a file).
    #[serde(default)]
    pub file: Option<FileFacet>,
    /// Folder facet (present if item is a folder).
    #[serde(default)]
    pub folder: Option<FolderFacet>,
    /// Parent reference.
    #[serde(default)]
    pub parent_reference: Option<ParentReference>,
}

/// File facet metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFacet {
    /// MIME type.
    #[serde(default)]
    pub mime_type: Option<String>,
    /// File hashes.
    #[serde(default)]
    pub hashes: Option<FileHashes>,
}

/// File hash values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHashes {
    /// SHA1 hash.
    #[serde(default)]
    pub sha1_hash: Option<String>,
    /// QuickXor hash.
    #[serde(default)]
    pub quick_xor_hash: Option<String>,
}

/// Folder facet metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderFacet {
    /// Number of children.
    #[serde(default)]
    pub child_count: u64,
}

/// Parent reference info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentReference {
    /// Parent item ID.
    #[serde(default)]
    pub id: Option<String>,
    /// Path to parent.
    #[serde(default)]
    pub path: Option<String>,
}

impl DriveItem {
    /// Check if this item is a folder.
    pub fn is_folder(&self) -> bool {
        self.folder.is_some()
    }
}

/// Response from listing children.
#[derive(Debug, Deserialize)]
struct ChildrenResponse {
    value: Vec<DriveItem>,
    #[serde(default, rename = "@odata.nextLink")]
    next_link: Option<String>,
}

/// Response from creating an upload session.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadSession {
    upload_url: String,
}

/// Microsoft Graph API client for OneDrive.
pub struct OneDriveClient {
    http: Client,
    token_manager: Arc<OneDriveTokenManager>,
}

impl OneDriveClient {
    /// Create a new OneDrive client.
    pub fn new(token_manager: Arc<OneDriveTokenManager>) -> axiomvault_common::Result<Self> {
        Ok(Self {
            http: http_client::build_http_client()?,
            token_manager,
        })
    }

    /// Get authorization header.
    async fn auth_header(&self) -> Result<String> {
        let token = self.token_manager.get_access_token().await?;
        Ok(http_client::bearer_header(&token))
    }

    /// Encode a path for use in the Graph API URL.
    /// Uses the `root:{path}:` colon syntax.
    fn encode_path(path: &str) -> String {
        if path == "/" || path.is_empty() {
            "root".to_string()
        } else {
            let clean = if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/{}", path)
            };
            // Percent-encode special characters in path segments
            let encoded: String = clean
                .split('/')
                .map(|segment| {
                    percent_encoding::utf8_percent_encode(
                        segment,
                        percent_encoding::NON_ALPHANUMERIC,
                    )
                    .to_string()
                })
                .collect::<Vec<_>>()
                .join("/");
            format!("root:{}", encoded)
        }
    }

    /// Get item metadata by path.
    pub async fn get_metadata(&self, path: &str) -> Result<DriveItem> {
        let encoded = Self::encode_path(path);
        let url = format!("{}/{}", GRAPH_BASE, encoded);
        let auth = self.auth_header().await?;

        let response = self
            .http
            .get(&url)
            .header(header::AUTHORIZATION, auth)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to get metadata: {}", e)))?;

        self.handle_response(response).await
    }

    /// List children of a folder by path.
    pub async fn list_children(&self, path: &str) -> Result<Vec<DriveItem>> {
        let encoded = Self::encode_path(path);
        let url = if encoded == "root" {
            format!("{}/root/children", GRAPH_BASE)
        } else {
            format!("{}/:/{}/children", GRAPH_BASE, &encoded[5..]) // strip "root:" prefix
        };

        let mut all_items = Vec::new();
        let mut next_url = Some(url);

        while let Some(url) = next_url.take() {
            let auth = self.auth_header().await?;

            let response = self
                .http
                .get(&url)
                .header(header::AUTHORIZATION, auth)
                .send()
                .await
                .map_err(|e| Error::Network(format!("Failed to list children: {}", e)))?;

            let children: ChildrenResponse = self.handle_response(response).await?;
            all_items.extend(children.value);
            next_url = children.next_link;
        }

        Ok(all_items)
    }

    /// Upload a file (simple upload for files <= 4MB).
    pub async fn upload(&self, path: &str, data: Vec<u8>) -> Result<DriveItem> {
        let encoded = Self::encode_path(path);
        let url = format!("{}/:/{}/content", GRAPH_BASE, &encoded[5..]);
        let auth = self.auth_header().await?;

        let response = self
            .http
            .put(&url)
            .header(header::AUTHORIZATION, auth)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(data)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to upload file: {}", e)))?;

        self.handle_response(response).await
    }

    /// Upload a large file using an upload session.
    pub async fn upload_session(&self, path: &str, data: Vec<u8>) -> Result<DriveItem> {
        let encoded = Self::encode_path(path);
        let url = format!("{}/:/{}/createUploadSession", GRAPH_BASE, &encoded[5..]);
        let auth = self.auth_header().await?;

        let body = serde_json::json!({
            "item": {
                "@microsoft.graph.conflictBehavior": "replace"
            }
        });

        let response = self
            .http
            .post(&url)
            .header(header::AUTHORIZATION, auth)
            .header(header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to create upload session: {}", e)))?;

        let session: UploadSession = self.handle_response(response).await?;
        let total_size = data.len();
        let mut offset = 0;

        loop {
            let end = std::cmp::min(offset + CHUNK_SIZE, total_size);
            let chunk = &data[offset..end];
            let content_range = format!("bytes {}-{}/{}", offset, end - 1, total_size);

            let response = self
                .http
                .put(&session.upload_url)
                .header(header::CONTENT_LENGTH, chunk.len().to_string())
                .header(header::CONTENT_RANGE, content_range)
                .body(chunk.to_vec())
                .send()
                .await
                .map_err(|e| Error::Network(format!("Failed to upload chunk: {}", e)))?;

            let status = response.status();
            if status == StatusCode::OK || status == StatusCode::CREATED {
                return response.json().await.map_err(|e| {
                    Error::Network(format!("Failed to parse upload response: {}", e))
                });
            } else if status == StatusCode::ACCEPTED {
                // More chunks needed
                offset = end;
            } else {
                let body = response.text().await.unwrap_or_default();
                return Err(Error::Network(format!(
                    "Upload chunk failed: {} - {}",
                    status, body
                )));
            }
        }
    }

    /// Download file content by path.
    pub async fn download(&self, path: &str) -> Result<Vec<u8>> {
        let encoded = Self::encode_path(path);
        let url = format!("{}/:/{}/content", GRAPH_BASE, &encoded[5..]);
        let auth = self.auth_header().await?;

        let response = self
            .http
            .get(&url)
            .header(header::AUTHORIZATION, auth)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to download file: {}", e)))?;

        if !response.status().is_success() {
            return self.handle_error(response).await;
        }

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| Error::Network(format!("Failed to read download response: {}", e)))
    }

    /// Download file as a stream.
    pub async fn download_stream(
        &self,
        path: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        let encoded = Self::encode_path(path);
        let url = format!("{}/:/{}/content", GRAPH_BASE, &encoded[5..]);
        let auth = self.auth_header().await?;

        let response = self
            .http
            .get(&url)
            .header(header::AUTHORIZATION, auth)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to start download: {}", e)))?;

        if !response.status().is_success() {
            return self.handle_error(response).await;
        }

        let stream = response
            .bytes_stream()
            .map(|result| result.map_err(|e| Error::Network(format!("Stream read error: {}", e))));

        Ok(Box::pin(stream))
    }

    /// Delete an item by path.
    pub async fn delete(&self, path: &str) -> Result<()> {
        let encoded = Self::encode_path(path);
        let url = format!("{}/{}", GRAPH_BASE, encoded);
        let auth = self.auth_header().await?;

        let response = self
            .http
            .delete(&url)
            .header(header::AUTHORIZATION, auth)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to delete item: {}", e)))?;

        if response.status() == StatusCode::NO_CONTENT || response.status().is_success() {
            Ok(())
        } else {
            self.handle_error(response).await
        }
    }

    /// Create a folder by path.
    pub async fn create_folder(&self, parent_path: &str, name: &str) -> Result<DriveItem> {
        let encoded = Self::encode_path(parent_path);
        let url = if encoded == "root" {
            format!("{}/root/children", GRAPH_BASE)
        } else {
            format!("{}/:/{}/children", GRAPH_BASE, &encoded[5..])
        };
        let auth = self.auth_header().await?;

        let body = serde_json::json!({
            "name": name,
            "folder": {},
            "@microsoft.graph.conflictBehavior": "fail"
        });

        let response = self
            .http
            .post(&url)
            .header(header::AUTHORIZATION, auth)
            .header(header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to create folder: {}", e)))?;

        self.handle_response(response).await
    }

    /// Move or rename an item.
    pub async fn move_item(
        &self,
        from_path: &str,
        to_parent_path: &str,
        new_name: &str,
    ) -> Result<DriveItem> {
        // First get the source item to obtain its ID
        let item = self.get_metadata(from_path).await?;
        let url = format!("{}/items/{}", GRAPH_BASE, item.id);
        let auth = self.auth_header().await?;

        // Get the destination parent item to obtain its ID
        let parent = self.get_metadata(to_parent_path).await?;

        let body = serde_json::json!({
            "parentReference": {
                "id": parent.id
            },
            "name": new_name
        });

        let response = self
            .http
            .patch(&url)
            .header(header::AUTHORIZATION, auth)
            .header(header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to move item: {}", e)))?;

        self.handle_response(response).await
    }

    /// Copy an item.
    pub async fn copy_item(
        &self,
        from_path: &str,
        to_parent_path: &str,
        new_name: &str,
    ) -> Result<DriveItem> {
        // Get the source item ID
        let item = self.get_metadata(from_path).await?;
        let url = format!("{}/items/{}/copy", GRAPH_BASE, item.id);
        let auth = self.auth_header().await?;

        // Get the destination parent ID
        let parent = self.get_metadata(to_parent_path).await?;

        let body = serde_json::json!({
            "parentReference": {
                "id": parent.id
            },
            "name": new_name
        });

        let response = self
            .http
            .post(&url)
            .header(header::AUTHORIZATION, auth)
            .header(header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to copy item: {}", e)))?;

        let status = response.status();
        if status == StatusCode::ACCEPTED {
            // Copy is async; poll the monitor URL or return the source metadata
            // For simplicity, return the source item metadata with updated name
            let mut copied = item;
            copied.name = new_name.to_string();
            copied.id = String::new(); // ID is unknown until copy completes
            Ok(copied)
        } else if status.is_success() {
            self.handle_response_raw(response).await
        } else {
            self.handle_error(response).await
        }
    }

    /// Handle API response with error checking.
    async fn handle_response<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T> {
        http_client::handle_json_response(response).await
    }

    /// Handle the raw response body (same as handle_response but avoids move).
    async fn handle_response_raw<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T> {
        response
            .json()
            .await
            .map_err(|e| Error::Network(format!("Failed to parse response: {}", e)))
    }

    /// Convert an error response into an appropriate error.
    async fn handle_error<T>(&self, response: reqwest::Response) -> Result<T> {
        http_client::handle_error_response(response).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_path_root() {
        assert_eq!(OneDriveClient::encode_path("/"), "root");
        assert_eq!(OneDriveClient::encode_path(""), "root");
    }

    #[test]
    fn test_encode_path_file() {
        let encoded = OneDriveClient::encode_path("/documents/test.txt");
        assert!(encoded.starts_with("root:"));
        assert!(encoded.contains("documents"));
        assert!(encoded.contains("test"));
    }

    #[test]
    fn test_drive_item_is_folder() {
        let folder = DriveItem {
            id: "1".to_string(),
            name: "folder".to_string(),
            size: None,
            last_modified_date_time: None,
            etag: None,
            file: None,
            folder: Some(FolderFacet { child_count: 0 }),
            parent_reference: None,
        };
        assert!(folder.is_folder());

        let file = DriveItem {
            id: "2".to_string(),
            name: "file.txt".to_string(),
            size: Some(1024),
            last_modified_date_time: None,
            etag: None,
            file: Some(FileFacet {
                mime_type: Some("text/plain".to_string()),
                hashes: None,
            }),
            folder: None,
            parent_reference: None,
        };
        assert!(!file.is_folder());
    }

    #[test]
    fn test_drive_item_serialization() {
        let item = DriveItem {
            id: "abc123".to_string(),
            name: "test.txt".to_string(),
            size: Some(100),
            last_modified_date_time: Some(Utc::now()),
            etag: Some("etag123".to_string()),
            file: Some(FileFacet {
                mime_type: Some("text/plain".to_string()),
                hashes: None,
            }),
            folder: None,
            parent_reference: None,
        };

        let json = serde_json::to_string(&item).unwrap();
        let deserialized: DriveItem = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, item.id);
        assert_eq!(deserialized.name, item.name);
        assert_eq!(deserialized.size, item.size);
    }
}
