use std::sync::Arc;

use axiomvault_app::{AppEvent, AppService};
use axiomvault_common::VaultPath;
use axiomvault_storage::{MemoryProvider, StorageProvider};
use axiomvault_sync::{ChangeType, SyncConfig};

#[tokio::test]
async fn app_service_exposes_configure_stage_and_sync_lifecycle() {
    let service = AppService::new();
    let mut events = service.subscribe();
    let remote: Arc<dyn StorageProvider> = Arc::new(MemoryProvider::new());
    let local: Arc<dyn StorageProvider> = Arc::new(MemoryProvider::new());
    let state_dir = tempfile::tempdir().unwrap();
    let path = VaultPath::parse("/app.bin").unwrap();
    local.upload(&path, b"from-app".to_vec()).await.unwrap();

    service
        .configure_sync(
            remote.clone(),
            local,
            state_dir.path(),
            SyncConfig::default(),
        )
        .await
        .unwrap();
    service
        .stage_sync_change("/app.bin", b"from-app".to_vec(), ChangeType::Create)
        .await
        .unwrap();
    let result = service.sync_now().await.unwrap();

    assert_eq!(result.files_synced, 1);
    assert_eq!(remote.download(&path).await.unwrap(), b"from-app");
    assert!(matches!(events.try_recv().unwrap(), AppEvent::SyncStarted));
    assert!(matches!(
        events.try_recv().unwrap(),
        AppEvent::SyncCompleted
    ));
}

#[tokio::test]
async fn app_service_rejects_sync_before_configuration() {
    let error = AppService::new().sync_now().await.unwrap_err();
    assert!(error.to_string().contains("not configured"));
}
