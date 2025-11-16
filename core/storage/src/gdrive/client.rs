//! Google Drive API client.

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use reqwest::{header, Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::pin::Pin;

use axiomvault_common::{Error, Result};

use super::auth::TokenManager;

/// Google Drive API base URL.
const DRIVE_API_BASE: &str = "https://www.googleapis.com/drive/v3";
/// Google Drive upload API base URL.
const DRIVE_UPLOAD_BASE: &str = "https://www.googleapis.com/upload/drive/v3";

/// Chunk size for resumable uploads (256KB minimum, must be multiple of 256KB).
const CHUNK_SIZE: usize = 256 * 1024; // 256KB

/// Google Drive file metadata from API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFile {
    /// File ID.
    pub id: String,
    /// File name.
    pub name: String,
    /// MIME type.
    pub mime_type: String,
    /// File size in bytes (only for files, not folders).
    #[serde(default)]
    pub size: Option<String>,
    /// Created time.
    #[serde(default)]
    pub created_time: Option<DateTime<Utc>>,
    /// Modified time.
    #[serde(default)]
    pub modified_time: Option<DateTime<Utc>>,
    /// Parent folder IDs.
    #[serde(default)]
    pub parents: Vec<String>,
    /// MD5 checksum (only for files).
    #[serde(default)]
    pub md5_checksum: Option<String>,
    /// Trashed status.
    #[serde(default)]
    pub trashed: bool,
}

impl DriveFile {
    /// Check if this is a folder.
    pub fn is_folder(&self) -> bool {
        self.mime_type == "application/vnd.google-apps.folder"
    }

    /// Get size as u64.
    pub fn size_bytes(&self) -> Option<u64> {
        self.size.as_ref().and_then(|s| s.parse().ok())
    }
}

/// Response from listing files.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileListResponse {
    files: Vec<DriveFile>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// Google Drive API client.
pub struct DriveClient {
    http: Client,
    token_manager: std::sync::Arc<TokenManager>,
}

impl DriveClient {
    /// Create a new Drive client.
    pub fn new(token_manager: std::sync::Arc<TokenManager>) -> Self {
        let http = Client::builder()
            .user_agent("AxiomVault/0.1")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            http,
            token_manager,
        }
    }

    /// Get authorization header.
    async fn auth_header(&self) -> Result<String> {
        let token = self.token_manager.get_access_token().await?;
        Ok(format!("Bearer {}", token))
    }

    /// Get file metadata by ID.
    pub async fn get_file(&self, file_id: &str) -> Result<DriveFile> {
        let url = format!("{}/files/{}", DRIVE_API_BASE, file_id);
        let auth = self.auth_header().await?;

        let response = self
            .http
            .get(&url)
            .header(header::AUTHORIZATION, auth)
            .query(&[(
                "fields",
                "id,name,mimeType,size,createdTime,modifiedTime,parents,md5Checksum,trashed",
            )])
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to get file: {}", e)))?;

        self.handle_response(response).await
    }

    /// Create a folder.
    pub async fn create_folder(&self, name: &str, parent_id: Option<&str>) -> Result<DriveFile> {
        let url = format!("{}/files", DRIVE_API_BASE);
        let auth = self.auth_header().await?;

        let mut metadata = serde_json::json!({
            "name": name,
            "mimeType": "application/vnd.google-apps.folder"
        });

        if let Some(parent) = parent_id {
            metadata["parents"] = serde_json::json!([parent]);
        }

        let response = self
            .http
            .post(&url)
            .header(header::AUTHORIZATION, auth)
            .header(header::CONTENT_TYPE, "application/json")
            .query(&[(
                "fields",
                "id,name,mimeType,size,createdTime,modifiedTime,parents,md5Checksum,trashed",
            )])
            .json(&metadata)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to create folder: {}", e)))?;

        self.handle_response(response).await
    }

    /// List files in a folder.
    pub async fn list_folder(&self, folder_id: &str) -> Result<Vec<DriveFile>> {
        let mut all_files = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let url = format!("{}/files", DRIVE_API_BASE);
            let auth = self.auth_header().await?;

            let query = format!("'{}' in parents and trashed = false", folder_id);

            let mut request = self
                .http
                .get(&url)
                .header(header::AUTHORIZATION, auth)
                .query(&[
                    ("q", query.as_str()),
                    ("fields", "files(id,name,mimeType,size,createdTime,modifiedTime,parents,md5Checksum,trashed),nextPageToken"),
                    ("pageSize", "1000"),
                ]);

            if let Some(token) = &page_token {
                request = request.query(&[("pageToken", token.as_str())]);
            }

            let response = request
                .send()
                .await
                .map_err(|e| Error::Network(format!("Failed to list folder: {}", e)))?;

            let list_response: FileListResponse = self.handle_response(response).await?;
            all_files.extend(list_response.files);

            match list_response.next_page_token {
                Some(token) => page_token = Some(token),
                None => break,
            }
        }

        Ok(all_files)
    }

    /// Find a file by name in a folder.
    pub async fn find_file(&self, name: &str, parent_id: &str) -> Result<Option<DriveFile>> {
        let url = format!("{}/files", DRIVE_API_BASE);
        let auth = self.auth_header().await?;

        let query = format!(
            "name = '{}' and '{}' in parents and trashed = false",
            name.replace('\'', "\\'"),
            parent_id
        );

        let response = self
            .http
            .get(&url)
            .header(header::AUTHORIZATION, auth)
            .query(&[
                ("q", query.as_str()),
                ("fields", "files(id,name,mimeType,size,createdTime,modifiedTime,parents,md5Checksum,trashed)"),
                ("pageSize", "1"),
            ])
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to find file: {}", e)))?;

        let list_response: FileListResponse = self.handle_response(response).await?;
        Ok(list_response.files.into_iter().next())
    }

    /// Upload a small file (< 5MB).
    pub async fn upload_simple(
        &self,
        name: &str,
        parent_id: &str,
        data: Vec<u8>,
    ) -> Result<DriveFile> {
        let url = format!("{}/files?uploadType=multipart", DRIVE_UPLOAD_BASE);
        let auth = self.auth_header().await?;

        let metadata = serde_json::json!({
            "name": name,
            "parents": [parent_id]
        });

        let metadata_json = serde_json::to_string(&metadata)
            .map_err(|e| Error::InvalidInput(format!("Failed to serialize metadata: {}", e)))?;

        // Build multipart request
        let boundary = "AxiomVaultBoundary";
        let mut body = Vec::new();

        // Metadata part
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
        body.extend_from_slice(metadata_json.as_bytes());
        body.extend_from_slice(b"\r\n");

        // Data part
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(&data);
        body.extend_from_slice(b"\r\n");

        // End boundary
        body.extend_from_slice(format!("--{}--", boundary).as_bytes());

        let response = self
            .http
            .post(&url)
            .header(header::AUTHORIZATION, auth)
            .header(
                header::CONTENT_TYPE,
                format!("multipart/related; boundary={}", boundary),
            )
            .query(&[(
                "fields",
                "id,name,mimeType,size,createdTime,modifiedTime,parents,md5Checksum,trashed",
            )])
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to upload file: {}", e)))?;

        self.handle_response(response).await
    }

    /// Update an existing file.
    pub async fn update_file(&self, file_id: &str, data: Vec<u8>) -> Result<DriveFile> {
        let url = format!("{}/files/{}?uploadType=media", DRIVE_UPLOAD_BASE, file_id);
        let auth = self.auth_header().await?;

        let response = self
            .http
            .patch(&url)
            .header(header::AUTHORIZATION, auth)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .query(&[(
                "fields",
                "id,name,mimeType,size,createdTime,modifiedTime,parents,md5Checksum,trashed",
            )])
            .body(data)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to update file: {}", e)))?;

        self.handle_response(response).await
    }

    /// Start a resumable upload session.
    pub async fn start_resumable_upload(
        &self,
        name: &str,
        parent_id: &str,
        total_size: u64,
    ) -> Result<String> {
        let url = format!("{}/files?uploadType=resumable", DRIVE_UPLOAD_BASE);
        let auth = self.auth_header().await?;

        let metadata = serde_json::json!({
            "name": name,
            "parents": [parent_id]
        });

        let response = self
            .http
            .post(&url)
            .header(header::AUTHORIZATION, auth)
            .header(header::CONTENT_TYPE, "application/json")
            .header("X-Upload-Content-Length", total_size.to_string())
            .json(&metadata)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to start resumable upload: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Network(format!(
                "Failed to start resumable upload: {} - {}",
                status, body
            )));
        }

        // Extract upload URI from Location header
        let upload_uri = response
            .headers()
            .get(header::LOCATION)
            .ok_or_else(|| Error::Network("No upload URI in response".to_string()))?
            .to_str()
            .map_err(|e| Error::Network(format!("Invalid upload URI: {}", e)))?
            .to_string();

        Ok(upload_uri)
    }

    /// Upload a chunk to a resumable upload session.
    pub async fn upload_chunk(
        &self,
        upload_uri: &str,
        data: &[u8],
        start_byte: u64,
        total_size: u64,
    ) -> Result<Option<DriveFile>> {
        let end_byte = start_byte + data.len() as u64 - 1;
        let content_range = format!("bytes {}-{}/{}", start_byte, end_byte, total_size);

        let response = self
            .http
            .put(upload_uri)
            .header(header::CONTENT_LENGTH, data.len().to_string())
            .header(header::CONTENT_RANGE, content_range)
            .body(data.to_vec())
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to upload chunk: {}", e)))?;

        let status = response.status();

        if status == StatusCode::OK || status == StatusCode::CREATED {
            // Upload complete
            let file: DriveFile = response
                .json()
                .await
                .map_err(|e| Error::Network(format!("Failed to parse upload response: {}", e)))?;
            Ok(Some(file))
        } else if status == StatusCode::PERMANENT_REDIRECT
            || status == StatusCode::from_u16(308).unwrap()
        {
            // More chunks needed (308 Resume Incomplete)
            Ok(None)
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(Error::Network(format!(
                "Chunk upload failed: {} - {}",
                status, body
            )))
        }
    }

    /// Upload a large file using resumable upload with streaming.
    pub async fn upload_resumable(
        &self,
        name: &str,
        parent_id: &str,
        mut stream: Pin<Box<dyn Stream<Item = Result<Vec<u8>>> + Send>>,
        total_size: u64,
    ) -> Result<DriveFile> {
        let upload_uri = self
            .start_resumable_upload(name, parent_id, total_size)
            .await?;

        let mut bytes_uploaded = 0u64;
        let mut buffer = Vec::with_capacity(CHUNK_SIZE);

        while let Some(chunk) = stream.next().await {
            let data = chunk?;
            buffer.extend_from_slice(&data);

            // Upload when buffer is full or stream is done
            while buffer.len() >= CHUNK_SIZE {
                let chunk_to_upload: Vec<u8> = buffer.drain(..CHUNK_SIZE).collect();
                let result = self
                    .upload_chunk(&upload_uri, &chunk_to_upload, bytes_uploaded, total_size)
                    .await?;

                bytes_uploaded += chunk_to_upload.len() as u64;

                if let Some(file) = result {
                    return Ok(file);
                }
            }
        }

        // Upload remaining bytes
        if !buffer.is_empty() {
            let result = self
                .upload_chunk(&upload_uri, &buffer, bytes_uploaded, total_size)
                .await?;

            if let Some(file) = result {
                return Ok(file);
            }
        }

        Err(Error::Network("Upload did not complete".to_string()))
    }

    /// Download file content.
    pub async fn download(&self, file_id: &str) -> Result<Vec<u8>> {
        let url = format!("{}/files/{}", DRIVE_API_BASE, file_id);
        let auth = self.auth_header().await?;

        let response = self
            .http
            .get(&url)
            .header(header::AUTHORIZATION, auth)
            .query(&[("alt", "media")])
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to download file: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Network(format!(
                "Download failed: {} - {}",
                status, body
            )));
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
        file_id: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        let url = format!("{}/files/{}", DRIVE_API_BASE, file_id);
        let auth = self.auth_header().await?;

        let response = self
            .http
            .get(&url)
            .header(header::AUTHORIZATION, auth)
            .query(&[("alt", "media")])
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to start download: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Network(format!(
                "Download failed: {} - {}",
                status, body
            )));
        }

        let stream = response
            .bytes_stream()
            .map(|result| result.map_err(|e| Error::Network(format!("Stream read error: {}", e))));

        Ok(Box::pin(stream))
    }

    /// Delete a file.
    pub async fn delete(&self, file_id: &str) -> Result<()> {
        let url = format!("{}/files/{}", DRIVE_API_BASE, file_id);
        let auth = self.auth_header().await?;

        let response = self
            .http
            .delete(&url)
            .header(header::AUTHORIZATION, auth)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to delete file: {}", e)))?;

        if response.status() == StatusCode::NO_CONTENT || response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            Err(Error::Network(format!(
                "Delete failed: {} - {}",
                status, body
            )))
        }
    }

    /// Move/rename a file.
    pub async fn move_file(
        &self,
        file_id: &str,
        new_name: Option<&str>,
        new_parent: Option<&str>,
        current_parent: Option<&str>,
    ) -> Result<DriveFile> {
        let url = format!("{}/files/{}", DRIVE_API_BASE, file_id);
        let auth = self.auth_header().await?;

        let mut metadata = serde_json::json!({});
        if let Some(name) = new_name {
            metadata["name"] = serde_json::json!(name);
        }

        let mut request = self
            .http
            .patch(&url)
            .header(header::AUTHORIZATION, auth)
            .header(header::CONTENT_TYPE, "application/json")
            .query(&[(
                "fields",
                "id,name,mimeType,size,createdTime,modifiedTime,parents,md5Checksum,trashed",
            )]);

        // Handle parent change
        if let Some(new_parent_id) = new_parent {
            request = request.query(&[("addParents", new_parent_id)]);
            if let Some(old_parent_id) = current_parent {
                request = request.query(&[("removeParents", old_parent_id)]);
            }
        }

        let response = request
            .json(&metadata)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to move file: {}", e)))?;

        self.handle_response(response).await
    }

    /// Copy a file.
    pub async fn copy_file(
        &self,
        file_id: &str,
        new_name: &str,
        parent_id: &str,
    ) -> Result<DriveFile> {
        let url = format!("{}/files/{}/copy", DRIVE_API_BASE, file_id);
        let auth = self.auth_header().await?;

        let metadata = serde_json::json!({
            "name": new_name,
            "parents": [parent_id]
        });

        let response = self
            .http
            .post(&url)
            .header(header::AUTHORIZATION, auth)
            .header(header::CONTENT_TYPE, "application/json")
            .query(&[(
                "fields",
                "id,name,mimeType,size,createdTime,modifiedTime,parents,md5Checksum,trashed",
            )])
            .json(&metadata)
            .send()
            .await
            .map_err(|e| Error::Network(format!("Failed to copy file: {}", e)))?;

        self.handle_response(response).await
    }

    /// Handle API response with error checking.
    async fn handle_response<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();

        if status.is_success() {
            response
                .json()
                .await
                .map_err(|e| Error::Network(format!("Failed to parse response: {}", e)))
        } else if status == StatusCode::NOT_FOUND {
            Err(Error::NotFound("Resource not found".to_string()))
        } else if status == StatusCode::UNAUTHORIZED {
            Err(Error::Authentication(
                "Invalid or expired token".to_string(),
            ))
        } else if status == StatusCode::FORBIDDEN {
            Err(Error::PermissionDenied("Access denied".to_string()))
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(Error::Network(format!("API error: {} - {}", status, body)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drive_file_is_folder() {
        let folder = DriveFile {
            id: "1".to_string(),
            name: "folder".to_string(),
            mime_type: "application/vnd.google-apps.folder".to_string(),
            size: None,
            created_time: None,
            modified_time: None,
            parents: vec![],
            md5_checksum: None,
            trashed: false,
        };

        assert!(folder.is_folder());

        let file = DriveFile {
            id: "2".to_string(),
            name: "file.txt".to_string(),
            mime_type: "text/plain".to_string(),
            size: Some("1024".to_string()),
            created_time: None,
            modified_time: None,
            parents: vec![],
            md5_checksum: None,
            trashed: false,
        };

        assert!(!file.is_folder());
    }

    #[test]
    fn test_drive_file_size_bytes() {
        let file = DriveFile {
            id: "1".to_string(),
            name: "file.txt".to_string(),
            mime_type: "text/plain".to_string(),
            size: Some("12345".to_string()),
            created_time: None,
            modified_time: None,
            parents: vec![],
            md5_checksum: None,
            trashed: false,
        };

        assert_eq!(file.size_bytes(), Some(12345));

        let folder = DriveFile {
            id: "2".to_string(),
            name: "folder".to_string(),
            mime_type: "application/vnd.google-apps.folder".to_string(),
            size: None,
            created_time: None,
            modified_time: None,
            parents: vec![],
            md5_checksum: None,
            trashed: false,
        };

        assert_eq!(folder.size_bytes(), None);
    }

    #[test]
    fn test_drive_file_serialization() {
        let file = DriveFile {
            id: "abc123".to_string(),
            name: "test.txt".to_string(),
            mime_type: "text/plain".to_string(),
            size: Some("100".to_string()),
            created_time: Some(Utc::now()),
            modified_time: Some(Utc::now()),
            parents: vec!["root".to_string()],
            md5_checksum: Some("md5hash".to_string()),
            trashed: false,
        };

        let json = serde_json::to_string(&file).unwrap();
        let deserialized: DriveFile = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, file.id);
        assert_eq!(deserialized.name, file.name);
        assert_eq!(deserialized.mime_type, file.mime_type);
    }
}
