//! Vault health check and integrity verification.
//!
//! Provides diagnostics for vault structure, tree index integrity,
//! orphaned files, and missing files. Uses the unified health types
//! from [`axiomvault_common::health`].

use std::collections::HashSet;

use tracing::{debug, warn};

use crate::config::{
    VaultConfig, VaultVersion, CONFIG_FILENAME, DATA_DIRNAME, META_DIRNAME, TREE_FILENAME,
};
use crate::tree::{NodeType, TreeNode, VaultTree};
use axiomvault_common::health::{DiagnosticResult, HealthReport, Severity};
use axiomvault_common::{Error, Result, VaultPath};
use axiomvault_crypto::{decrypt, MasterKey};
use axiomvault_storage::StorageProvider;

/// Context tag for tree index key derivation (must match session.rs).
const TREE_KEY_CONTEXT: &[u8] = b"vault_tree_index_v1";

/// Run a shallow health check that does not require a password.
///
/// Checks directory structure, vault.config existence and parsing,
/// and basic file counts. Useful when the vault cannot be unlocked.
pub async fn check_vault_structure(
    provider: &dyn StorageProvider,
    vault_path: &str,
) -> Result<HealthReport> {
    let mut results = Vec::new();

    // Check that vault.config exists and is parseable.
    // vault.config lives at the vault root, not under the metadata directory.
    let config_path = VaultPath::parse(CONFIG_FILENAME)?;
    match provider.exists(&config_path).await {
        Ok(true) => match provider.download(&config_path).await {
            Ok(data) => match VaultConfig::from_bytes(&data) {
                Ok(config) => {
                    check_config(&config, &mut results);
                }
                Err(e) => {
                    results.push(DiagnosticResult {
                        check_name: "config_parse".to_string(),
                        severity: Severity::Error,
                        message: format!("vault.config is corrupted: {}", e),
                        auto_fixable: false,
                    });
                }
            },
            Err(e) => {
                results.push(DiagnosticResult {
                    check_name: "config_read".to_string(),
                    severity: Severity::Error,
                    message: format!("Failed to read vault.config: {}", e),
                    auto_fixable: false,
                });
            }
        },
        Ok(false) => {
            results.push(DiagnosticResult {
                check_name: "config_exists".to_string(),
                severity: Severity::Error,
                message: "vault.config not found — this may not be a valid vault".to_string(),
                auto_fixable: false,
            });
        }
        Err(e) => {
            results.push(DiagnosticResult {
                check_name: "config_exists".to_string(),
                severity: Severity::Error,
                message: format!("Failed to check vault.config: {}", e),
                auto_fixable: false,
            });
        }
    }

    // Check meta directory
    let meta_path = VaultPath::parse(META_DIRNAME)?;
    match provider.exists(&meta_path).await {
        Ok(true) => {
            results.push(DiagnosticResult {
                check_name: "meta_dir".to_string(),
                severity: Severity::Info,
                message: "Metadata directory exists".to_string(),
                auto_fixable: false,
            });
        }
        Ok(false) => {
            results.push(DiagnosticResult {
                check_name: "meta_dir".to_string(),
                severity: Severity::Error,
                message: "Metadata directory missing".to_string(),
                auto_fixable: false,
            });
        }
        _ => {}
    }

    // Check data directory and count files
    let data_path = VaultPath::parse(DATA_DIRNAME)?;
    match provider.list(&data_path).await {
        Ok(entries) => {
            let file_count = entries.len();
            results.push(DiagnosticResult {
                check_name: "data_dir".to_string(),
                severity: Severity::Info,
                message: format!("Data directory contains {} encrypted file(s)", file_count),
                auto_fixable: false,
            });
        }
        Err(_) => {
            results.push(DiagnosticResult {
                check_name: "data_dir".to_string(),
                severity: Severity::Warning,
                message: "Data directory missing or empty".to_string(),
                auto_fixable: false,
            });
        }
    }

    // Check tree.json exists (without decrypting)
    let tree_path = VaultPath::parse(META_DIRNAME)?.join(TREE_FILENAME)?;
    match provider.exists(&tree_path).await {
        Ok(true) => {
            results.push(DiagnosticResult {
                check_name: "tree_exists".to_string(),
                severity: Severity::Info,
                message: "Tree index file exists".to_string(),
                auto_fixable: false,
            });
        }
        Ok(false) => {
            results.push(DiagnosticResult {
                check_name: "tree_exists".to_string(),
                severity: Severity::Warning,
                message: "Tree index file missing (vault may be empty)".to_string(),
                auto_fixable: false,
            });
        }
        _ => {}
    }

    Ok(HealthReport::new(vault_path, results))
}

/// Run all health checks on a vault.
///
/// This checks configuration validity, tree index integrity,
/// orphaned files in storage, and missing files referenced by the tree.
/// Requires vault password to decrypt and verify contents.
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

    Ok(HealthReport::new(vault_path, results))
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
    let mut tree_bytes = decrypt(tree_key.as_bytes(), &encrypted_bytes).map_err(|e| {
        Error::Crypto(format!(
            "Failed to decrypt tree index (wrong password or corrupted vault): {}",
            e
        ))
    })?;

    let tree_json = String::from_utf8(tree_bytes.clone())
        .map_err(|e| Error::Serialization(format!("Invalid UTF-8 in tree data: {}", e)));

    // Best-effort: wipe decrypted tree bytes (contains vault structure metadata).
    use zeroize::Zeroize;
    tree_bytes.zeroize();

    let tree_json = tree_json?;

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
        let creation =
            VaultConfig::new(id, password, "memory", serde_json::Value::Null, params).unwrap();

        let provider = Arc::new(MemoryProvider::new());

        provider
            .create_dir(&VaultPath::parse("d").unwrap())
            .await
            .unwrap();
        provider
            .create_dir(&VaultPath::parse("m").unwrap())
            .await
            .unwrap();

        (provider, creation.config, creation.master_key)
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
        let report = HealthReport::new(
            "/tmp/test",
            vec![DiagnosticResult {
                check_name: "test".to_string(),
                severity: Severity::Info,
                message: "All good".to_string(),
                auto_fixable: false,
            }],
        );

        let json = report.to_json().unwrap();
        assert!(json.contains("test"));
        assert!(json.contains("All good"));
    }

    #[tokio::test]
    async fn test_health_report_status_computed() {
        let (provider, config, master_key) = setup_vault().await;

        let report = check_vault_health(provider.as_ref(), &config, &master_key, "/tmp/test")
            .await
            .unwrap();

        // Empty vault has a warning ("tree index does not exist") so status is Degraded
        assert!(!report.has_errors());
        assert!(report.has_warnings());
        assert_eq!(report.status, axiomvault_common::HealthStatus::Degraded);
    }
}
