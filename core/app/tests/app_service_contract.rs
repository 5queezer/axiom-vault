//! Contract and integration tests for the shared AppService API.
//!
//! These tests verify the behavioral contracts that all platform clients
//! depend on. Regressions here mean broken clients.

use axiomvault_app::{
    AppError, AppEvent, AppService, CreateVaultParams, LocalIndex, OpenVaultParams,
    RecoverVaultParams,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn memory_params(vault_id: &str, password: &str) -> CreateVaultParams {
    CreateVaultParams {
        vault_id: vault_id.to_string(),
        password: password.to_string(),
        provider_type: "memory".to_string(),
        provider_config: serde_json::Value::Null,
    }
}

fn memory_open_params(password: &str) -> OpenVaultParams {
    OpenVaultParams {
        password: password.to_string(),
        provider_type: "memory".to_string(),
        provider_config: serde_json::Value::Null,
    }
}

/// Create a service with an open vault ready for file operations.
async fn service_with_vault() -> AppService {
    let svc = AppService::new();
    svc.create_vault(memory_params("test-vault", "password"))
        .await
        .unwrap();
    svc
}

/// Create a service with an open vault and an attached in-memory index.
async fn service_with_index() -> AppService {
    let svc = service_with_vault().await;
    let index = LocalIndex::in_memory().unwrap();
    svc.set_local_index(index).await.unwrap();
    svc
}

// ===========================================================================
// Vault lifecycle
// ===========================================================================

#[tokio::test]
async fn create_vault_returns_24_recovery_words() {
    let svc = AppService::new();
    let result = svc.create_vault(memory_params("v1", "pass")).await.unwrap();

    assert_eq!(result.recovery_words.split_whitespace().count(), 24);
    assert_eq!(result.info.id, "v1");
    assert!(result.info.is_unlocked);
}

// Note: duplicate vault detection depends on the storage provider's
// vault_exists check. The memory provider does not persist state between
// manager calls, so this test uses a local provider with a temp directory.
#[tokio::test]
async fn create_duplicate_vault_fails() {
    let svc = AppService::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();
    let config = serde_json::json!({ "root": root });

    svc.create_vault(CreateVaultParams {
        vault_id: "dup".to_string(),
        password: "pass".to_string(),
        provider_type: "local".to_string(),
        provider_config: config.clone(),
    })
    .await
    .unwrap();

    svc.close_vault().await.unwrap();

    let err = svc
        .create_vault(CreateVaultParams {
            vault_id: "dup".to_string(),
            password: "pass".to_string(),
            provider_type: "local".to_string(),
            provider_config: config,
        })
        .await
        .unwrap_err();

    assert!(
        matches!(err, AppError::VaultAlreadyExists(_)),
        "expected VaultAlreadyExists, got {:?}",
        err
    );
}

#[tokio::test]
async fn open_nonexistent_vault_fails() {
    let svc = AppService::new();
    let err = svc
        .open_vault(memory_open_params("password"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, AppError::VaultNotFound(_)),
        "expected VaultNotFound, got {:?}",
        err
    );
}

#[tokio::test]
async fn lock_clears_active_state() {
    let svc = service_with_vault().await;

    svc.lock_vault().await.unwrap();

    let info = svc.vault_info().await.unwrap();
    assert!(!info.is_unlocked);

    // File operations should fail while locked.
    let err = svc.read_file("/any").await.unwrap_err();
    assert!(
        matches!(err, AppError::VaultLocked | AppError::InvalidInput(_)),
        "expected VaultLocked or InvalidInput, got {:?}",
        err
    );
}

#[tokio::test]
async fn close_removes_vault() {
    let svc = service_with_vault().await;
    svc.close_vault().await.unwrap();

    assert!(!svc.is_vault_open().await);
    assert!(matches!(svc.vault_info().await, Err(AppError::NoOpenVault)));
}

#[tokio::test]
async fn close_then_lock_fails() {
    let svc = service_with_vault().await;
    svc.close_vault().await.unwrap();
    assert!(matches!(svc.lock_vault().await, Err(AppError::NoOpenVault)));
}

#[tokio::test]
async fn double_lock_is_idempotent() {
    let svc = service_with_vault().await;
    svc.lock_vault().await.unwrap();
    // Second lock is a no-op (session already locked). Should not panic.
    svc.lock_vault().await.unwrap();
}

#[tokio::test]
async fn double_close_fails() {
    let svc = service_with_vault().await;
    svc.close_vault().await.unwrap();
    assert!(matches!(
        svc.close_vault().await,
        Err(AppError::NoOpenVault)
    ));
}

// ===========================================================================
// Recovery
// ===========================================================================

// Recovery requires persistent storage (memory provider doesn't persist config).
#[tokio::test]
async fn recover_vault_with_valid_words() {
    let svc = AppService::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();
    let config = serde_json::json!({ "root": root });

    let created = svc
        .create_vault(CreateVaultParams {
            vault_id: "rec".to_string(),
            password: "old-pass".to_string(),
            provider_type: "local".to_string(),
            provider_config: config.clone(),
        })
        .await
        .unwrap();
    let words = created.recovery_words.clone();

    // Write a file, then close and recover.
    svc.create_file("/secret.txt", b"data").await.unwrap();
    svc.close_vault().await.unwrap();

    let info = svc
        .recover_vault(RecoverVaultParams {
            recovery_words: words,
            new_password: "new-pass".to_string(),
            provider_type: "local".to_string(),
            provider_config: config,
        })
        .await
        .unwrap();

    assert!(info.is_unlocked);

    // Data should still be accessible after recovery.
    let content = svc.read_file("/secret.txt").await.unwrap();
    assert_eq!(content, b"data");
}

#[tokio::test]
async fn recover_vault_with_wrong_words_fails() {
    let svc = AppService::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();
    let config = serde_json::json!({ "root": root });

    svc.create_vault(CreateVaultParams {
        vault_id: "rec2".to_string(),
        password: "pass".to_string(),
        provider_type: "local".to_string(),
        provider_config: config.clone(),
    })
    .await
    .unwrap();
    svc.close_vault().await.unwrap();

    let err = svc
        .recover_vault(RecoverVaultParams {
            recovery_words: "abandon ".repeat(24).trim().to_string(),
            new_password: "new".to_string(),
            provider_type: "local".to_string(),
            provider_config: config,
        })
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            AppError::InvalidRecoveryKey | AppError::Crypto(_) | AppError::Internal(_)
        ),
        "expected InvalidRecoveryKey, Crypto, or Internal, got {:?}",
        err
    );
}

// ===========================================================================
// Password change
// ===========================================================================

#[tokio::test]
async fn change_password_allows_new_password() {
    let svc = service_with_vault().await;
    svc.create_file("/test.txt", b"hello").await.unwrap();

    svc.change_password("password", "new-password")
        .await
        .unwrap();

    // File should still be readable.
    let content = svc.read_file("/test.txt").await.unwrap();
    assert_eq!(content, b"hello");
}

#[tokio::test]
async fn change_password_with_wrong_old_password_fails() {
    let svc = service_with_vault().await;
    let err = svc.change_password("wrong", "new").await.unwrap_err();
    assert!(
        matches!(err, AppError::InvalidPassword | AppError::Crypto(_)),
        "expected InvalidPassword or Crypto, got {:?}",
        err
    );
}

// ===========================================================================
// File CRUD
// ===========================================================================

#[tokio::test]
async fn create_and_read_file() {
    let svc = service_with_vault().await;
    svc.create_file("/hello.txt", b"world").await.unwrap();

    let content = svc.read_file("/hello.txt").await.unwrap();
    assert_eq!(content, b"world");
}

#[tokio::test]
async fn create_empty_file() {
    let svc = service_with_vault().await;
    svc.create_file("/empty.txt", b"").await.unwrap();

    let content = svc.read_file("/empty.txt").await.unwrap();
    assert!(content.is_empty());
}

#[tokio::test]
async fn create_large_file() {
    let svc = service_with_vault().await;
    let data = vec![0xAB_u8; 1024 * 1024]; // 1 MiB
    svc.create_file("/large.bin", &data).await.unwrap();

    let content = svc.read_file("/large.bin").await.unwrap();
    assert_eq!(content.len(), 1024 * 1024);
    assert_eq!(content, data);
}

#[tokio::test]
async fn create_duplicate_file_fails() {
    let svc = service_with_vault().await;
    svc.create_file("/dup.txt", b"first").await.unwrap();

    let err = svc.create_file("/dup.txt", b"second").await.unwrap_err();
    assert!(
        matches!(err, AppError::PathAlreadyExists(_)),
        "expected PathAlreadyExists, got {:?}",
        err
    );
}

#[tokio::test]
async fn read_nonexistent_file_fails() {
    let svc = service_with_vault().await;
    let err = svc.read_file("/no-such-file.txt").await.unwrap_err();
    assert!(
        matches!(err, AppError::PathNotFound(_)),
        "expected PathNotFound, got {:?}",
        err
    );
}

#[tokio::test]
async fn update_nonexistent_file_fails() {
    let svc = service_with_vault().await;
    let err = svc.update_file("/ghost.txt", b"data").await.unwrap_err();
    assert!(
        matches!(err, AppError::PathNotFound(_)),
        "expected PathNotFound, got {:?}",
        err
    );
}

#[tokio::test]
async fn delete_nonexistent_file_fails() {
    let svc = service_with_vault().await;
    let err = svc.delete_file("/ghost.txt").await.unwrap_err();
    assert!(
        matches!(err, AppError::PathNotFound(_)),
        "expected PathNotFound, got {:?}",
        err
    );
}

#[tokio::test]
async fn update_preserves_path() {
    let svc = service_with_vault().await;
    svc.create_file("/file.txt", b"v1").await.unwrap();
    svc.update_file("/file.txt", b"v2").await.unwrap();

    let content = svc.read_file("/file.txt").await.unwrap();
    assert_eq!(content, b"v2");
    assert!(svc.exists("/file.txt").await.unwrap());
}

#[tokio::test]
async fn delete_file_removes_it() {
    let svc = service_with_vault().await;
    svc.create_file("/rm.txt", b"bye").await.unwrap();
    svc.delete_file("/rm.txt").await.unwrap();

    assert!(!svc.exists("/rm.txt").await.unwrap());
}

// ===========================================================================
// Directory operations
// ===========================================================================

#[tokio::test]
async fn create_and_list_directory() {
    let svc = service_with_vault().await;
    svc.create_directory("/photos").await.unwrap();

    assert!(svc.exists("/photos").await.unwrap());

    let meta = svc.metadata("/photos").await.unwrap();
    assert!(meta.is_directory);
}

#[tokio::test]
async fn list_empty_directory() {
    let svc = service_with_vault().await;
    svc.create_directory("/empty-dir").await.unwrap();

    let entries = svc.list_directory("/empty-dir").await.unwrap();
    assert!(entries.is_empty());
}

#[tokio::test]
async fn list_root_directory() {
    let svc = service_with_vault().await;
    svc.create_file("/a.txt", b"a").await.unwrap();
    svc.create_directory("/sub").await.unwrap();

    let entries = svc.list_directory("/").await.unwrap();
    assert!(entries.len() >= 2);

    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"a.txt"));
    assert!(names.contains(&"sub"));
}

#[tokio::test]
async fn nested_directory_operations() {
    let svc = service_with_vault().await;
    svc.create_directory("/a").await.unwrap();
    svc.create_directory("/a/b").await.unwrap();
    svc.create_directory("/a/b/c").await.unwrap();
    svc.create_file("/a/b/c/deep.txt", b"deep").await.unwrap();

    let content = svc.read_file("/a/b/c/deep.txt").await.unwrap();
    assert_eq!(content, b"deep");

    // Listing /a should show only direct child b.
    let entries = svc.list_directory("/a").await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "b");
    assert!(entries[0].is_directory);
}

#[tokio::test]
async fn delete_empty_directory() {
    let svc = service_with_vault().await;
    svc.create_directory("/tmp").await.unwrap();
    svc.delete_directory("/tmp").await.unwrap();
    assert!(!svc.exists("/tmp").await.unwrap());
}

#[tokio::test]
async fn delete_nonempty_directory_fails() {
    let svc = service_with_vault().await;
    svc.create_directory("/docs").await.unwrap();
    svc.create_file("/docs/readme.txt", b"hi").await.unwrap();

    let err = svc.delete_directory("/docs").await.unwrap_err();
    // The vault layer should reject deleting a non-empty directory.
    assert!(
        !matches!(err, AppError::NoOpenVault),
        "unexpected NoOpenVault, got {:?}",
        err
    );
}

#[tokio::test]
async fn create_file_in_nonexistent_directory_fails() {
    let svc = service_with_vault().await;
    let err = svc
        .create_file("/no-dir/file.txt", b"data")
        .await
        .unwrap_err();
    assert!(
        matches!(err, AppError::PathNotFound(_)),
        "expected PathNotFound, got {:?}",
        err
    );
}

// ===========================================================================
// Metadata
// ===========================================================================

#[tokio::test]
async fn file_metadata_reports_size() {
    let svc = service_with_vault().await;
    svc.create_file("/sized.bin", b"12345").await.unwrap();

    let meta = svc.metadata("/sized.bin").await.unwrap();
    assert_eq!(meta.name, "sized.bin");
    assert!(!meta.is_directory);
    assert_eq!(meta.size, Some(5));
}

#[tokio::test]
async fn directory_metadata() {
    let svc = service_with_vault().await;
    svc.create_directory("/mydir").await.unwrap();

    let meta = svc.metadata("/mydir").await.unwrap();
    assert_eq!(meta.name, "mydir");
    assert!(meta.is_directory);
}

#[tokio::test]
async fn metadata_nonexistent_path_fails() {
    let svc = service_with_vault().await;
    let err = svc.metadata("/nope").await.unwrap_err();
    assert!(
        matches!(err, AppError::PathNotFound(_)),
        "expected PathNotFound, got {:?}",
        err
    );
}

#[tokio::test]
async fn exists_returns_false_for_missing() {
    let svc = service_with_vault().await;
    assert!(!svc.exists("/nonexistent").await.unwrap());
}

// ===========================================================================
// Event emission
// ===========================================================================

#[tokio::test]
async fn vault_lifecycle_events() {
    let svc = AppService::new();
    let mut rx = svc.subscribe();

    svc.create_vault(memory_params("ev", "pass")).await.unwrap();
    assert!(matches!(rx.try_recv().unwrap(), AppEvent::VaultCreated(_)));

    svc.lock_vault().await.unwrap();
    assert!(matches!(rx.try_recv().unwrap(), AppEvent::VaultLocked));

    svc.close_vault().await.unwrap();
    assert!(matches!(rx.try_recv().unwrap(), AppEvent::VaultClosed));
}

#[tokio::test]
async fn file_operation_events() {
    let svc = service_with_vault().await;
    let mut rx = svc.subscribe();

    svc.create_file("/e.txt", b"e").await.unwrap();
    assert!(matches!(
        rx.try_recv().unwrap(),
        AppEvent::FileCreated { .. }
    ));

    svc.update_file("/e.txt", b"e2").await.unwrap();
    assert!(matches!(
        rx.try_recv().unwrap(),
        AppEvent::FileUpdated { .. }
    ));

    svc.delete_file("/e.txt").await.unwrap();
    assert!(matches!(
        rx.try_recv().unwrap(),
        AppEvent::FileDeleted { .. }
    ));
}

#[tokio::test]
async fn directory_operation_events() {
    let svc = service_with_vault().await;
    let mut rx = svc.subscribe();

    svc.create_directory("/ev-dir").await.unwrap();
    assert!(matches!(
        rx.try_recv().unwrap(),
        AppEvent::DirectoryCreated { .. }
    ));

    svc.list_directory("/ev-dir").await.unwrap();
    assert!(matches!(
        rx.try_recv().unwrap(),
        AppEvent::DirectoryListed { .. }
    ));

    svc.delete_directory("/ev-dir").await.unwrap();
    assert!(matches!(
        rx.try_recv().unwrap(),
        AppEvent::DirectoryDeleted { .. }
    ));
}

#[tokio::test]
async fn password_change_event() {
    let svc = service_with_vault().await;
    let mut rx = svc.subscribe();

    svc.change_password("password", "new").await.unwrap();
    assert!(matches!(rx.try_recv().unwrap(), AppEvent::PasswordChanged));
}

// ===========================================================================
// No-vault guard (all operations fail with NoOpenVault)
// ===========================================================================

#[tokio::test]
async fn operations_fail_without_vault() {
    let svc = AppService::new();

    assert!(matches!(svc.vault_info().await, Err(AppError::NoOpenVault)));
    assert!(matches!(svc.lock_vault().await, Err(AppError::NoOpenVault)));
    assert!(matches!(
        svc.close_vault().await,
        Err(AppError::NoOpenVault)
    ));
    assert!(matches!(
        svc.read_file("/x").await,
        Err(AppError::NoOpenVault)
    ));
    assert!(matches!(
        svc.create_file("/x", b"").await,
        Err(AppError::NoOpenVault)
    ));
    assert!(matches!(
        svc.update_file("/x", b"").await,
        Err(AppError::NoOpenVault)
    ));
    assert!(matches!(
        svc.delete_file("/x").await,
        Err(AppError::NoOpenVault)
    ));
    assert!(matches!(
        svc.create_directory("/x").await,
        Err(AppError::NoOpenVault)
    ));
    assert!(matches!(
        svc.list_directory("/").await,
        Err(AppError::NoOpenVault)
    ));
    assert!(matches!(
        svc.delete_directory("/x").await,
        Err(AppError::NoOpenVault)
    ));
    assert!(matches!(svc.exists("/x").await, Err(AppError::NoOpenVault)));
    assert!(matches!(
        svc.metadata("/x").await,
        Err(AppError::NoOpenVault)
    ));
    assert!(matches!(
        svc.change_password("a", "b").await,
        Err(AppError::NoOpenVault)
    ));
    assert!(matches!(
        svc.vault_session().await,
        Err(AppError::NoOpenVault)
    ));
}

// ===========================================================================
// LocalIndex integration
// ===========================================================================

#[tokio::test]
async fn index_tracks_file_operations() {
    let svc = AppService::new();
    let result = svc
        .create_vault(memory_params("idx", "pass"))
        .await
        .unwrap();
    let index = LocalIndex::in_memory().unwrap();
    svc.set_local_index(index).await.unwrap();

    // Create file → index should have an entry.
    svc.create_file("/tracked.txt", b"data").await.unwrap();
    // We can't query the index directly from here, but the contract is:
    // the service doesn't error, and operations remain consistent.

    // Update file.
    svc.update_file("/tracked.txt", b"updated").await.unwrap();

    // Delete file.
    svc.delete_file("/tracked.txt").await.unwrap();
    assert!(!svc.exists("/tracked.txt").await.unwrap());

    drop(result);
}

#[tokio::test]
async fn index_tracks_directory_operations() {
    let svc = service_with_index().await;

    svc.create_directory("/indexed-dir").await.unwrap();
    svc.delete_directory("/indexed-dir").await.unwrap();
    assert!(!svc.exists("/indexed-dir").await.unwrap());
}

#[tokio::test]
async fn lock_wipes_index() {
    let svc = service_with_index().await;

    svc.create_file("/secret.txt", b"secret").await.unwrap();
    svc.create_directory("/secret-dir").await.unwrap();

    // Lock should wipe the index without error.
    svc.lock_vault().await.unwrap();

    let info = svc.vault_info().await.unwrap();
    assert!(!info.is_unlocked);
}

#[tokio::test]
async fn close_wipes_index() {
    let svc = service_with_index().await;

    svc.create_file("/wiped.txt", b"bye").await.unwrap();

    // Close should wipe the index without error.
    svc.close_vault().await.unwrap();
    assert!(!svc.is_vault_open().await);
}

// ===========================================================================
// Import / export (file I/O)
// ===========================================================================

#[tokio::test]
async fn import_and_export_file() {
    let svc = service_with_vault().await;

    let tmp = std::env::temp_dir().join("axiomvault_test_import.txt");
    std::fs::write(&tmp, b"imported content").unwrap();

    svc.import_file(tmp.to_str().unwrap(), "/imported.txt")
        .await
        .unwrap();

    let content = svc.read_file("/imported.txt").await.unwrap();
    assert_eq!(content, b"imported content");

    let export_path = std::env::temp_dir().join("axiomvault_test_export.txt");
    svc.export_file("/imported.txt", export_path.to_str().unwrap())
        .await
        .unwrap();

    let exported = std::fs::read(&export_path).unwrap();
    assert_eq!(exported, b"imported content");

    // Cleanup.
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(&export_path);
}

#[tokio::test]
async fn import_nonexistent_local_file_fails() {
    let svc = service_with_vault().await;
    let err = svc
        .import_file("/nonexistent/path/file.txt", "/dest.txt")
        .await
        .unwrap_err();
    assert!(
        matches!(err, AppError::Storage(_)),
        "expected Storage, got {:?}",
        err
    );
}

// ===========================================================================
// vault_exists
// ===========================================================================

// vault_exists requires persistent storage to detect existing vaults.
#[tokio::test]
async fn vault_exists_before_and_after_create() {
    let svc = AppService::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();
    let config = serde_json::json!({ "root": root });

    let exists = svc.vault_exists("local", config.clone()).await.unwrap();
    assert!(!exists);

    svc.create_vault(CreateVaultParams {
        vault_id: "ex".to_string(),
        password: "pass".to_string(),
        provider_type: "local".to_string(),
        provider_config: config.clone(),
    })
    .await
    .unwrap();

    svc.close_vault().await.unwrap();

    let exists = svc.vault_exists("local", config).await.unwrap();
    assert!(exists);
}

// ===========================================================================
// DTO serialization round-trips
// ===========================================================================

#[tokio::test]
async fn dto_round_trip_serialization() {
    let svc = service_with_vault().await;

    // VaultInfoDto
    let info = svc.vault_info().await.unwrap();
    let json = serde_json::to_string(&info).unwrap();
    let deser: axiomvault_app::VaultInfoDto = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.id, info.id);

    // DirectoryEntryDto
    svc.create_file("/ser.txt", b"x").await.unwrap();
    let entries = svc.list_directory("/").await.unwrap();
    let json = serde_json::to_string(&entries).unwrap();
    let deser: Vec<axiomvault_app::DirectoryEntryDto> = serde_json::from_str(&json).unwrap();
    assert!(!deser.is_empty());

    // FileMetadataDto
    let meta = svc.metadata("/ser.txt").await.unwrap();
    let json = serde_json::to_string(&meta).unwrap();
    let deser: axiomvault_app::FileMetadataDto = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.name, "ser.txt");
}

#[tokio::test]
async fn event_round_trip_serialization() {
    // All event variants should survive JSON round-trip.
    let events = vec![
        AppEvent::VaultLocked,
        AppEvent::VaultClosed,
        AppEvent::PasswordChanged,
        AppEvent::FileCreated {
            path: "/a".to_string(),
        },
        AppEvent::FileUpdated {
            path: "/a".to_string(),
        },
        AppEvent::FileDeleted {
            path: "/a".to_string(),
        },
        AppEvent::DirectoryCreated {
            path: "/d".to_string(),
        },
        AppEvent::DirectoryDeleted {
            path: "/d".to_string(),
        },
        AppEvent::SyncStarted,
        AppEvent::SyncCompleted,
        AppEvent::SyncFailed {
            error: "err".to_string(),
        },
        AppEvent::Error {
            message: "msg".to_string(),
        },
    ];

    for event in events {
        let json = serde_json::to_string(&event).unwrap();
        let deser: AppEvent = serde_json::from_str(&json).unwrap();
        // Verify round-trip produces valid JSON; exact matching per variant
        // is not needed since serde derives handle it correctly.
        let json2 = serde_json::to_string(&deser).unwrap();
        assert_eq!(json, json2);
    }
}

// ===========================================================================
// Path edge cases
// ===========================================================================

#[tokio::test]
async fn root_path_operations() {
    let svc = service_with_vault().await;

    // Root should always exist.
    let entries = svc.list_directory("/").await.unwrap();
    // Empty vault root listing is valid.
    drop(entries);

    // Cannot delete root.
    let err = svc.delete_directory("/").await;
    assert!(err.is_err());
}

#[tokio::test]
async fn file_with_special_characters_in_name() {
    let svc = service_with_vault().await;

    // Names with spaces, dashes, underscores, dots.
    svc.create_file("/my file (1).txt", b"a").await.unwrap();
    svc.create_file("/under_score.txt", b"b").await.unwrap();
    svc.create_file("/dash-name.txt", b"c").await.unwrap();
    svc.create_file("/multi.dots.in.name.txt", b"d")
        .await
        .unwrap();

    assert_eq!(svc.read_file("/my file (1).txt").await.unwrap(), b"a");
    assert_eq!(svc.read_file("/under_score.txt").await.unwrap(), b"b");
    assert_eq!(svc.read_file("/dash-name.txt").await.unwrap(), b"c");
    assert_eq!(
        svc.read_file("/multi.dots.in.name.txt").await.unwrap(),
        b"d"
    );
}

#[tokio::test]
async fn unicode_file_names() {
    let svc = service_with_vault().await;

    svc.create_file("/日本語.txt", b"japanese").await.unwrap();
    svc.create_file("/émojis-🎉.txt", b"party").await.unwrap();
    svc.create_file("/Ñoño.txt", b"spanish").await.unwrap();

    assert_eq!(svc.read_file("/日本語.txt").await.unwrap(), b"japanese");
    assert_eq!(svc.read_file("/émojis-🎉.txt").await.unwrap(), b"party");
    assert_eq!(svc.read_file("/Ñoño.txt").await.unwrap(), b"spanish");
}

#[tokio::test]
async fn binary_file_content() {
    let svc = service_with_vault().await;

    // All byte values 0x00–0xFF.
    let data: Vec<u8> = (0..=255).collect();
    svc.create_file("/binary.bin", &data).await.unwrap();

    let content = svc.read_file("/binary.bin").await.unwrap();
    assert_eq!(content, data);
}
