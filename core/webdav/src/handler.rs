//! WebDAV HTTP method handlers.
//!
//! Each handler translates a WebDAV method into vault operations via
//! `VaultOperations`, performing decrypt-on-read and encrypt-on-write
//! transparently.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use tracing::debug;

use axiomvault_common::VaultPath;
use axiomvault_vault::{VaultOperations, VaultSession};

use crate::xml::{self, PropEntry};

/// Shared application state passed to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub session: Arc<VaultSession>,
    pub max_body_size: usize,
}

/// Top-level router: dispatches by HTTP method.
pub async fn handle_request(
    State(state): State<AppState>,
    req: axum::extract::Request,
) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let depth = req
        .headers()
        .get("Depth")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("1")
        .to_string();

    match method.as_str() {
        "OPTIONS" => handle_options(),
        "PROPFIND" => handle_propfind(&state, &depth, &path).await,
        "GET" => handle_get(&state, &path).await,
        "HEAD" => handle_head(&state, &path).await,
        "PUT" => handle_put(&state, req, &path).await,
        "MKCOL" => handle_mkcol(&state, &path).await,
        "DELETE" => handle_delete(&state, &path).await,
        "MOVE" | "COPY" | "LOCK" | "UNLOCK" => not_implemented(),
        _ => (StatusCode::METHOD_NOT_ALLOWED, "Method not allowed").into_response(),
    }
}

/// OPTIONS — return allowed methods and DAV compliance headers.
fn handle_options() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("Allow", "OPTIONS, PROPFIND, GET, HEAD, PUT, MKCOL, DELETE")
        .header("DAV", "1")
        .header("Content-Length", "0")
        .body(Body::empty())
        .unwrap()
}

/// 501 Not Implemented for unsupported methods.
fn not_implemented() -> Response {
    (StatusCode::NOT_IMPLEMENTED, "Not implemented").into_response()
}

/// PROPFIND — list directory or return metadata for a single resource.
async fn handle_propfind(state: &AppState, depth: &str, path: &str) -> Response {
    debug!("PROPFIND request");

    // Clamp "infinity" to "1" to avoid recursive listing
    let depth = if depth == "infinity" { "1" } else { depth };

    let vault_path = match normalize_vault_path(path) {
        Ok(p) => p,
        Err(status) => return error_response(status, "Invalid path"),
    };

    let ops = match VaultOperations::new(&state.session) {
        Ok(ops) => ops,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    let mut entries = Vec::new();

    // Get metadata for the requested path itself
    match ops.metadata(&vault_path).await {
        Ok((name, is_dir, size)) => {
            let tree = state.session.tree().read().await;
            let node = tree.get_node(&vault_path).ok();
            let (created, modified, etag) = node
                .map(|n| {
                    (
                        n.metadata.created_at,
                        n.metadata.modified_at,
                        n.metadata.etag.clone(),
                    )
                })
                .unwrap_or_else(|| (chrono::Utc::now(), chrono::Utc::now(), None));
            drop(tree);

            let href = if is_dir {
                ensure_trailing_slash(path)
            } else {
                path.to_string()
            };

            entries.push(PropEntry {
                href,
                display_name: if vault_path.is_root() {
                    "/".to_string()
                } else {
                    name
                },
                is_collection: is_dir,
                content_length: if is_dir { None } else { size },
                content_type: if is_dir {
                    "httpd/unix-directory".to_string()
                } else {
                    guess_mime(path)
                },
                last_modified: modified,
                created,
                etag,
            });

            // If depth=1 and this is a directory, list children
            if depth == "1" && is_dir {
                if let Ok(children) = ops.list_directory(&vault_path).await {
                    let tree = state.session.tree().read().await;
                    for (child_name, child_is_dir, child_size) in &children {
                        let child_path_str = if vault_path.is_root() {
                            format!("/{}", child_name)
                        } else {
                            format!("{}/{}", path.trim_end_matches('/'), child_name)
                        };

                        let child_vault_path = match VaultPath::parse(&child_path_str) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        let (child_created, child_modified, child_etag) = tree
                            .get_node(&child_vault_path)
                            .map(|n| {
                                (
                                    n.metadata.created_at,
                                    n.metadata.modified_at,
                                    n.metadata.etag.clone(),
                                )
                            })
                            .unwrap_or_else(|_| (chrono::Utc::now(), chrono::Utc::now(), None));

                        let child_href = if *child_is_dir {
                            ensure_trailing_slash(&child_path_str)
                        } else {
                            child_path_str.clone()
                        };

                        entries.push(PropEntry {
                            href: child_href,
                            display_name: child_name.clone(),
                            is_collection: *child_is_dir,
                            content_length: if *child_is_dir { None } else { *child_size },
                            content_type: if *child_is_dir {
                                "httpd/unix-directory".to_string()
                            } else {
                                guess_mime(&child_path_str)
                            },
                            last_modified: child_modified,
                            created: child_created,
                            etag: child_etag,
                        });
                    }
                }
            }
        }
        Err(_) => {
            return error_response(StatusCode::NOT_FOUND, "Resource not found");
        }
    }

    let body = xml::build_multistatus(&entries);

    Response::builder()
        .status(StatusCode::MULTI_STATUS)
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(Body::from(body))
        .unwrap()
}

/// GET — read and decrypt file content.
async fn handle_get(state: &AppState, path: &str) -> Response {
    debug!("GET request");

    let vault_path = match normalize_vault_path(path) {
        Ok(p) => p,
        Err(status) => return error_response(status, "Invalid path"),
    };

    let ops = match VaultOperations::new(&state.session) {
        Ok(ops) => ops,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    // Check if it's a directory — can't GET a directory
    match ops.metadata(&vault_path).await {
        Ok((_, true, _)) => {
            return error_response(StatusCode::METHOD_NOT_ALLOWED, "Cannot GET a directory");
        }
        Err(_) => return error_response(StatusCode::NOT_FOUND, "File not found"),
        _ => {}
    }

    match ops.read_file(&vault_path).await {
        Ok(content) => {
            let mut builder = Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", guess_mime(path))
                .header("Content-Length", content.len().to_string());

            // Add Last-Modified and ETag from tree metadata
            let tree = state.session.tree().read().await;
            if let Ok(node) = tree.get_node(&vault_path) {
                let modified = node
                    .metadata
                    .modified_at
                    .format("%a, %d %b %Y %H:%M:%S GMT")
                    .to_string();
                builder = builder.header("Last-Modified", modified);
                if let Some(ref etag) = node.metadata.etag {
                    builder = builder.header("ETag", format!("\"{}\"", etag));
                }
            }

            builder.body(Body::from(content)).unwrap()
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// HEAD — return metadata headers without body.
async fn handle_head(state: &AppState, path: &str) -> Response {
    debug!("HEAD request");

    let vault_path = match normalize_vault_path(path) {
        Ok(p) => p,
        Err(status) => return error_response(status, "Invalid path"),
    };

    let ops = match VaultOperations::new(&state.session) {
        Ok(ops) => ops,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    match ops.metadata(&vault_path).await {
        Ok((_, is_dir, size)) => {
            let mut builder = Response::builder().status(StatusCode::OK);

            if is_dir {
                builder = builder.header("Content-Type", "httpd/unix-directory");
            } else {
                builder = builder.header("Content-Type", guess_mime(path));
                if let Some(s) = size {
                    builder = builder.header("Content-Length", s.to_string());
                }
            }

            let tree = state.session.tree().read().await;
            if let Ok(node) = tree.get_node(&vault_path) {
                let modified = node
                    .metadata
                    .modified_at
                    .format("%a, %d %b %Y %H:%M:%S GMT")
                    .to_string();
                builder = builder.header("Last-Modified", modified);
                if let Some(ref etag) = node.metadata.etag {
                    builder = builder.header("ETag", format!("\"{}\"", etag));
                }
            }

            builder.body(Body::empty()).unwrap()
        }
        Err(_) => error_response(StatusCode::NOT_FOUND, "Not found"),
    }
}

/// PUT — create or update a file with encrypted content.
async fn handle_put(state: &AppState, req: axum::extract::Request, path: &str) -> Response {
    debug!("PUT request");

    let vault_path = match normalize_vault_path(path) {
        Ok(p) => p,
        Err(status) => return error_response(status, "Invalid path"),
    };

    // Read request body with size limit
    let body = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &e.to_string()),
    };

    if body.len() > state.max_body_size {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Request body exceeds maximum size",
        );
    }

    let ops = match VaultOperations::new(&state.session) {
        Ok(ops) => ops,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    let exists = ops.exists(&vault_path).await;

    if exists {
        match ops.update_file(&vault_path, &body).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        }
    } else {
        match ops.create_file(&vault_path, &body).await {
            Ok(()) => StatusCode::CREATED.into_response(),
            Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        }
    }
}

/// MKCOL — create a directory.
async fn handle_mkcol(state: &AppState, path: &str) -> Response {
    debug!("MKCOL request");

    let vault_path = match normalize_vault_path(path) {
        Ok(p) => p,
        Err(status) => return error_response(status, "Invalid path"),
    };

    let ops = match VaultOperations::new(&state.session) {
        Ok(ops) => ops,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    match ops.create_directory(&vault_path).await {
        Ok(()) => StatusCode::CREATED.into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Already exists") {
                error_response(StatusCode::METHOD_NOT_ALLOWED, "Directory already exists")
            } else if msg.contains("Not found") {
                error_response(StatusCode::CONFLICT, "Parent directory does not exist")
            } else {
                error_response(StatusCode::INTERNAL_SERVER_ERROR, &msg)
            }
        }
    }
}

/// DELETE — remove a file or empty directory.
async fn handle_delete(state: &AppState, path: &str) -> Response {
    debug!("DELETE request");

    let vault_path = match normalize_vault_path(path) {
        Ok(p) => p,
        Err(status) => return error_response(status, "Invalid path"),
    };

    let ops = match VaultOperations::new(&state.session) {
        Ok(ops) => ops,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    // Check if it's a file or directory
    match ops.metadata(&vault_path).await {
        Ok((_, true, _)) => match ops.delete_directory(&vault_path).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        Ok((_, false, _)) => match ops.delete_file(&vault_path).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        Err(_) => error_response(StatusCode::NOT_FOUND, "Resource not found"),
    }
}

/// Convert a URL path to a VaultPath, handling percent-decoding and normalization.
fn normalize_vault_path(path: &str) -> Result<VaultPath, StatusCode> {
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    let cleaned = decoded.trim_end_matches('/');
    let normalized = if cleaned.is_empty() { "/" } else { cleaned };

    VaultPath::parse(normalized).map_err(|_| StatusCode::BAD_REQUEST)
}

/// Ensure a path ends with `/` for collection hrefs.
fn ensure_trailing_slash(path: &str) -> String {
    if path.ends_with('/') {
        path.to_string()
    } else {
        format!("{}/", path)
    }
}

/// Simple MIME type guessing based on file extension.
fn guess_mime(path: &str) -> String {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();

    match ext.as_str() {
        "txt" | "text" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "tar" => "application/x-tar",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "wasm" => "application/wasm",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "yaml" | "yml" => "text/yaml",
        "toml" => "application/toml",
        "rs" => "text/x-rust",
        "py" => "text/x-python",
        "sh" => "text/x-shellscript",
        "doc" | "docx" => "application/msword",
        "xls" | "xlsx" => "application/vnd.ms-excel",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Build an error response with a text body.
fn error_response(status: StatusCode, message: &str) -> Response {
    (status, message.to_string()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guess_mime() {
        assert_eq!(guess_mime("/test.txt"), "text/plain");
        assert_eq!(guess_mime("/test.png"), "image/png");
        assert_eq!(guess_mime("/test.unknown"), "application/octet-stream");
        assert_eq!(guess_mime("/no-extension"), "application/octet-stream");
    }

    #[test]
    fn test_ensure_trailing_slash() {
        assert_eq!(ensure_trailing_slash("/dir"), "/dir/");
        assert_eq!(ensure_trailing_slash("/dir/"), "/dir/");
    }

    #[test]
    fn test_normalize_vault_path() {
        let p = normalize_vault_path("/test/file.txt").unwrap();
        assert_eq!(p.to_string(), "/test/file.txt");

        let p = normalize_vault_path("/").unwrap();
        assert!(p.is_root());

        let p = normalize_vault_path("/dir/").unwrap();
        assert_eq!(p.to_string(), "/dir");
    }
}
