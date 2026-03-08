//! Vault health check and integrity verification.
//!
//! Provides diagnostics for vault structure, tree index integrity,
//! orphaned files, and missing files.

use std::collections::HashSet;

use serde::Serialize;
use tracing::{debug, warn};

use crate::config::{VaultConfig, VaultVersion, DATA_DIRNAME, META_DIRNAME, TREE_FILENAME};
use crate::tree::{NodeType, TreeNode, VaultTree};
use axiomvault_common::{Error, Result, VaultPath};
use axiomvault_crypto::{decrypt, MasterKey};
use axiomvault_storage::StorageProvider;

/// Context tag for tree index key derivation (must match session.rs).
const TREE_KEY_CONTEXT: &[u8] = b"vault_tree_index_v1";

/// Severity level for a diagnostic result.
#[derive(Debug, Clone, Serialize)]
pub enum Severity {
    /// Informational finding, no action needed.
    Info,
    /// Potential problem that may need attention.
    Warning,
    /// Definite problem that affects vault integrity.
    Error,
}

/// A single diagnostic finding from a health check.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticResult {
    /// Name of the check that produced this result.
    pub check_name: String,
    /// Severity of the finding.
    pub severity: Severity,
    /// Human-readable description of the finding.
    pub message: String,
    /// Whether this issue can be automatically fixed.
    pub auto_fixable: bool,
}

/// Complete health report for a vault.
#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    /// Individual diagnostic results.
    pub results: Vec<DiagnosticResult>,
    /// Path or identifier for the vault that was checked.
    pub vault_path: String,
}

impl HealthReport {
    /// Returns `true` if any diagnostic result has `Severity::Error`.
    pub fn has_errors(&self) -> bool {
        self.results
            .iter()
            .any(|r| matches!(r.severity, Severity::Error))
    }

    /// Serialize the report to a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|e| {
            format!(r#"{{"error": "Failed to serialize health report: {}"}}"#, e)
        })
    }
}

/// Run all health checks on a vault.
///
/// This checks configuration validity, tree index integrity,
/// orphaned files in storage, and missing files referenced by the tree.
pub async fn check_vault_health(
    provider: &dyn StorageProvider,
    config: &VaultConfig,
    master_key: &MasterKey,
    vault_path: &str,
) -> Result<HealthReport> {
    let mut results = Vec::new();

    check_config(config, &mut results);
    check_tree_index(provider, master_key, &mut results).await;

    // Only run cross-referencing checks if the tree loaded successfully.
    let tree_path = VaultPath::parse(META_DIRNAME)?.join(TREE_FILENAME)?;
    if provider.exists(&tree_path).await.unwrap_or(false) {
        if let Ok(tree) = load_tree(provider, master_key).await {
            let mut tree_encrypted_names = HashSet::new();
            collect_file_encrypted_names(tree.root(), &mut tree_encrypted_names);

            check_orphaned_files(provider, &tree_encrypted_names, &mut results).await;
            check_missing_files(provider, &tree_encrypted_names, &mut results).await;
        }
    }

    Ok(HealthReport {
        results,
        vault_path: vault_path.to_string(),
    })
}

/// Validate the vault configuration.
fn check_config(config: &VaultConfig, results: &mut Vec<DiagnosticResult>) {
    debug!("Running config validation check");

    if !config.version.is_compatible() {
        results.push(DiagnosticResult {
            check_name: "config_version".to_string(),
            severity: Severity::Error,
            message: format!(
                "Incompatible vault version: {}.{} (current: {}.{})",
                config.version.major,
                config.version.minor,
                VaultVersion::CURRENT.major,
                VaultVersion::CURRENT.minor,
            ),
            auto_fixable: false,
        });
    } else {
        results.push(DiagnosticResult {
            check_name: "config_version".to_string(),
            severity: Severity::Info,
            message: format!(
                "Vault version {}.{} is compatible",
                config.version.major, config.version.minor,
            ),
            auto_fixable: false,
        });
    }

    if config.key_verification.is_empty() {
        results.push(DiagnosticResult {
            check_name: "config_key_verification".to_string(),
            severity: Severity::Error,
            message: "Key verification data is missing".to_string(),
            auto_fixable: false,
        });
    }

    if config.provider_type.is_empty() {
        results.push(DiagnosticResult {
            check_name: "config_provider".to_string(),
            severity: Severity::Warning,
            message: "Provider type is empty".to_string(),
            auto_fixable: false,
        });
    }
}

/// Check the tree index: exists, decryptable, and parseable.
async fn check_tree_index(
    provider: &dyn StorageProvider,
    master_key: &MasterKey,
    results: &mut Vec<DiagnosticResult>,
) {
    debug!("Running tree index check");

    let tree_path = match VaultPath::parse(META_DIRNAME).and_then(|p| p.join(TREE_FILENAME)) {
        Ok(p) => p,
        Err(e) => {
            results.push(DiagnosticResult {
                check_name: "tree_index".to_string(),
                severity: Severity::Error,
                message: format!("Failed to construct tree path: {}", e),
                auto_fixable: false,
            });
            return;
        }
    };

    match provider.exists(&tree_path).await {
        Ok(true) => {}
        Ok(false) => {
            results.push(DiagnosticResult {
                check_name: "tree_index".to_string(),
                severity: Severity::Warning,
                message: "Tree index file does not exist (vault may be empty)".to_string(),
                auto_fixable: false,
            });
            return;
        }
        Err(e) => {
            results.push(DiagnosticResult {
                check_name: "tree_index".to_string(),
                severity: Severity::Error,
                message: format!("Failed to check tree index existence: {}", e),
                auto_fixable: false,
            });
            return;
        }
    }

    match load_tree(provider, master_key).await {
        Ok(tree) => {
            let file_count = tree.count_files();
            results.push(DiagnosticResult {
                check_name: "tree_index".to_string(),
                severity: Severity::Info,
                message: format!(
                    "Tree index is valid ({} files, {} total bytes)",
                    file_count,
                    tree.total_size(),
                ),
                auto_fixable: false,
            });
        }
        Err(e) => {
            results.push(DiagnosticResult {
                check_name: "tree_index".to_string(),
                severity: Severity::Error,
                message: format!("Tree index is corrupted or cannot be decrypted: {}", e),
                auto_fixable: false,
            });
        }
    }
}

/// Check for orphaned files in `d/` that are not referenced by the tree.
async fn check_orphaned_files(
    provider: &dyn StorageProvider,
    tree_encrypted_names: &HashSet<String>,
    results: &mut Vec<DiagnosticResult>,
) {
    debug!("Running orphaned files check");

    let data_path = match VaultPath::parse(DATA_DIRNAME) {
        Ok(p) => p,
        Err(_) => return,
    };

    let storage_files = match provider.list(&data_path).await {
        Ok(entries) => entries,
        Err(e) => {
            results.push(DiagnosticResult {
                check_name: "orphaned_files".to_string(),
                severity: Severity::Warning,
                message: format!("Failed to list data directory: {}", e),
                auto_fixable: false,
            });
            return;
        }
    };

    let mut orphan_count = 0;
    for entry in &storage_files {
        if entry.is_directory {
            continue;
        }
        if !tree_encrypted_names.contains(&entry.name) {
            warn!(file = %entry.name, "Orphaned file found in data directory");
            orphan_count += 1;
        }
    }

    if orphan_count > 0 {
        results.push(DiagnosticResult {
            check_name: "orphaned_files".to_string(),
            severity: Severity::Warning,
            message: format!(
                "{} orphaned file(s) found in data directory (not referenced by tree)",
                orphan_count
            ),
            auto_fixable: true,
        });
    } else {
        results.push(DiagnosticResult {
            check_name: "orphaned_files".to_string(),
            severity: Severity::Info,
            message: "No orphaned files found".to_string(),
            auto_fixable: false,
        });
    }
}

/// Check for files referenced in the tree that are missing from `d/`.
async fn check_missing_files(
    provider: &dyn StorageProvider,
    tree_encrypted_names: &HashSet<String>,
    results: &mut Vec<DiagnosticResult>,
) {
    debug!("Running missing files check");

    let mut missing_count = 0;
    for encrypted_name in tree_encrypted_names {
        let file_path = match VaultPath::parse(DATA_DIRNAME).and_then(|p| p.join(encrypted_name)) {
            Ok(p) => p,
            Err(_) => continue,
        };

        match provider.exists(&file_path).await {
            Ok(true) => {}
            Ok(false) => {
                warn!(file = %encrypted_name, "Missing file referenced by tree");
                missing_count += 1;
            }
            Err(_) => {
                missing_count += 1;
            }
        }
    }

    if missing_count > 0 {
        results.push(DiagnosticResult {
            check_name: "missing_files".to_string(),
            severity: Severity::Error,
            message: format!(
                "{} file(s) referenced by tree are missing from data directory",
                missing_count
            ),
            auto_fixable: false,
        });
    } else {
        results.push(DiagnosticResult {
            check_name: "missing_files".to_string(),
            severity: Severity::Info,
            message: "All tree-referenced files exist in data directory".to_string(),
            auto_fixable: false,
        });
    }
}

/// Load and decrypt the vault tree from storage.
async fn load_tree(provider: &dyn StorageProvider, master_key: &MasterKey) -> Result<VaultTree> {
    let tree_path = VaultPath::parse(META_DIRNAME)?.join(TREE_FILENAME)?;

    if !provider.exists(&tree_path).await? {
        return Ok(VaultTree::new());
    }

    let encrypted_bytes = provider.download(&tree_path).await?;

    let tree_key = master_key.derive_file_key(TREE_KEY_CONTEXT);
    let tree_bytes = decrypt(tree_key.as_bytes(), &encrypted_bytes).map_err(|e| {
        Error::Crypto(format!(
            "Failed to decrypt tree index (wrong password or corrupted vault): {}",
            e
        ))
    })?;

    let tree_json = String::from_utf8(tree_bytes)
        .map_err(|e| Error::Serialization(format!("Invalid UTF-8 in tree data: {}", e)))?;

    VaultTree::from_json(&tree_json)
}

/// Recursively collect all encrypted file names from the tree.
fn collect_file_encrypted_names(node: &TreeNode, names: &mut HashSet<String>) {
    if node.metadata.node_type == NodeType::File {
        names.insert(node.metadata.encrypted_name.clone());
    }
    for child in node.children.values() {
        collect_file_encrypted_names(child, names);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::VaultConfig;
    use axiomvault_common::VaultId;
    use axiomvault_crypto::KdfParams;
    use axiomvault_storage::MemoryProvider;
    use std::sync::Arc;

    async fn setup_vault() -> (Arc<MemoryProvider>, VaultConfig, MasterKey) {
        let id = VaultId::new("test-health").unwrap();
        let password = b"test-password";
        let params = KdfParams::moderate();
        let config =
            VaultConfig::new(id, password, "memory", serde_json::Value::Null, params).unwrap();

        let master_key = config.verify_password(password).unwrap().unwrap();

        let provider = Arc::new(MemoryProvider::new());

        provider
            .create_dir(&VaultPath::parse("d").unwrap())
            .await
            .unwrap();
        provider
            .create_dir(&VaultPath::parse("m").unwrap())
            .await
            .unwrap();

        (provider, config, master_key)
    }

    #[tokio::test]
    async fn test_health_check_empty_vault() {
        let (provider, config, master_key) = setup_vault().await;

        let report = check_vault_health(provider.as_ref(), &config, &master_key, "/tmp/test")
            .await
            .unwrap();

        assert!(!report.has_errors());
        assert!(!report.results.is_empty());
    }

    #[tokio::test]
    async fn test_health_check_with_tree() {
        let (provider, config, master_key) = setup_vault().await;

        let mut tree = VaultTree::new();
        tree.create_file(&VaultPath::parse("/test.txt").unwrap(), "enc_file", 100)
            .unwrap();

        let tree_json = tree.to_json().unwrap();
        let tree_key = master_key.derive_file_key(b"vault_tree_index_v1");
        let encrypted =
            axiomvault_crypto::encrypt(tree_key.as_bytes(), tree_json.as_bytes()).unwrap();
        let tree_path = VaultPath::parse("m").unwrap().join("tree.json").unwrap();
        provider.upload(&tree_path, encrypted).await.unwrap();

        let file_path = VaultPath::parse("d").unwrap().join("enc_file").unwrap();
        provider.upload(&file_path, vec![0u8; 100]).await.unwrap();

        let report = check_vault_health(provider.as_ref(), &config, &master_key, "/tmp/test")
            .await
            .unwrap();

        assert!(!report.has_errors());
    }

    #[tokio::test]
    async fn test_health_check_missing_file() {
        let (provider, config, master_key) = setup_vault().await;

        let mut tree = VaultTree::new();
        tree.create_file(&VaultPath::parse("/ghost.txt").unwrap(), "ghost_enc", 50)
            .unwrap();

        let tree_json = tree.to_json().unwrap();
        let tree_key = master_key.derive_file_key(b"vault_tree_index_v1");
        let encrypted =
            axiomvault_crypto::encrypt(tree_key.as_bytes(), tree_json.as_bytes()).unwrap();
        let tree_path = VaultPath::parse("m").unwrap().join("tree.json").unwrap();
        provider.upload(&tree_path, encrypted).await.unwrap();

        let report = check_vault_health(provider.as_ref(), &config, &master_key, "/tmp/test")
            .await
            .unwrap();

        assert!(report.has_errors());
        assert!(report
            .results
            .iter()
            .any(|r| r.check_name == "missing_files" && matches!(r.severity, Severity::Error)));
    }

    #[tokio::test]
    async fn test_health_check_orphaned_file() {
        let (provider, config, master_key) = setup_vault().await;

        let tree = VaultTree::new();
        let tree_json = tree.to_json().unwrap();
        let tree_key = master_key.derive_file_key(b"vault_tree_index_v1");
        let encrypted =
            axiomvault_crypto::encrypt(tree_key.as_bytes(), tree_json.as_bytes()).unwrap();
        let tree_path = VaultPath::parse("m").unwrap().join("tree.json").unwrap();
        provider.upload(&tree_path, encrypted).await.unwrap();

        let orphan_path = VaultPath::parse("d").unwrap().join("orphan_enc").unwrap();
        provider.upload(&orphan_path, vec![1u8; 50]).await.unwrap();

        let report = check_vault_health(provider.as_ref(), &config, &master_key, "/tmp/test")
            .await
            .unwrap();

        assert!(!report.has_errors());
        assert!(report
            .results
            .iter()
            .any(|r| r.check_name == "orphaned_files" && matches!(r.severity, Severity::Warning)));
    }

    #[tokio::test]
    async fn test_health_check_incompatible_version() {
        let (provider, mut config, master_key) = setup_vault().await;

        config.version = VaultVersion {
            major: 99,
            minor: 0,
        };

        let report = check_vault_health(provider.as_ref(), &config, &master_key, "/tmp/test")
            .await
            .unwrap();

        assert!(report.has_errors());
        assert!(report
            .results
            .iter()
            .any(|r| r.check_name == "config_version" && matches!(r.severity, Severity::Error)));
    }

    #[test]
    fn test_health_report_json() {
        let report = HealthReport {
            results: vec![DiagnosticResult {
                check_name: "test".to_string(),
                severity: Severity::Info,
                message: "All good".to_string(),
                auto_fixable: false,
            }],
            vault_path: "/tmp/test".to_string(),
        };

        let json = report.to_json();
        assert!(json.contains("test"));
        assert!(json.contains("All good"));
    }
}
