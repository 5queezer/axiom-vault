//! Sync scheduling - on-demand and periodic modes.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::time::{interval, Instant};
use tracing::{debug, error, info};

use axiomvault_common::Result;

/// Sync mode configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncMode {
    /// Manual sync only.
    Manual,
    /// Sync triggered immediately on changes.
    OnDemand,
    /// Sync at regular intervals.
    Periodic { interval: Duration },
    /// Both on-demand and periodic.
    Hybrid { interval: Duration },
}

/// Sync request types.
#[derive(Debug)]
pub enum SyncRequest {
    /// Full sync of all files.
    Full,
    /// Sync specific paths.
    Paths(Vec<String>),
    /// Shutdown the scheduler.
    Shutdown,
}

/// Sync result from the engine.
#[derive(Debug, Clone)]
pub struct SyncResult {
    pub files_synced: usize,
    pub files_failed: usize,
    pub conflicts_found: usize,
    pub duration: Duration,
}

/// Scheduler for managing sync timing and requests.
pub struct SyncScheduler {
    /// Current sync mode.
    mode: Arc<RwLock<SyncMode>>,
    /// Channel to send sync requests.
    request_tx: mpsc::Sender<(SyncRequest, oneshot::Sender<Result<SyncResult>>)>,
    /// Shutdown signal.
    shutdown: Arc<RwLock<bool>>,
}

impl SyncScheduler {
    /// Create a new scheduler with the given mode.
    pub fn new(mode: SyncMode) -> (Self, SyncSchedulerHandle) {
        let (request_tx, request_rx) = mpsc::channel(100);
        let mode = Arc::new(RwLock::new(mode));
        let shutdown = Arc::new(RwLock::new(false));

        let scheduler = Self {
            mode: mode.clone(),
            request_tx: request_tx.clone(),
            shutdown: shutdown.clone(),
        };

        let handle = SyncSchedulerHandle {
            mode,
            request_tx,
            request_rx: Some(request_rx),
            shutdown,
        };

        (scheduler, handle)
    }

    /// Request a full sync.
    pub async fn request_sync(&self) -> Result<SyncResult> {
        self.request_sync_internal(SyncRequest::Full).await
    }

    /// Request sync for specific paths.
    pub async fn request_paths_sync(&self, paths: Vec<String>) -> Result<SyncResult> {
        self.request_sync_internal(SyncRequest::Paths(paths)).await
    }

    /// Internal sync request handler.
    async fn request_sync_internal(&self, request: SyncRequest) -> Result<SyncResult> {
        let (response_tx, response_rx) = oneshot::channel();

        self.request_tx
            .send((request, response_tx))
            .await
            .map_err(|_| axiomvault_common::Error::Vault("Scheduler not running".to_string()))?;

        response_rx.await.map_err(|_| {
            axiomvault_common::Error::Vault("Failed to receive sync result".to_string())
        })?
    }

    /// Change the sync mode.
    pub async fn set_mode(&self, mode: SyncMode) {
        let mut current_mode = self.mode.write().await;
        *current_mode = mode;
    }

    /// Get current sync mode.
    pub async fn get_mode(&self) -> SyncMode {
        self.mode.read().await.clone()
    }

    /// Shutdown the scheduler.
    pub async fn shutdown(&self) {
        let mut is_shutdown = self.shutdown.write().await;
        *is_shutdown = true;

        // Send shutdown request
        let (response_tx, _) = oneshot::channel();
        let _ = self
            .request_tx
            .send((SyncRequest::Shutdown, response_tx))
            .await;
    }
}

/// Handle for the scheduler background task.
pub struct SyncSchedulerHandle {
    mode: Arc<RwLock<SyncMode>>,
    request_tx: mpsc::Sender<(SyncRequest, oneshot::Sender<Result<SyncResult>>)>,
    request_rx: Option<mpsc::Receiver<(SyncRequest, oneshot::Sender<Result<SyncResult>>)>>,
    shutdown: Arc<RwLock<bool>>,
}

impl SyncSchedulerHandle {
    /// Run the scheduler background task.
    ///
    /// This should be spawned in a tokio task. The `sync_fn` is called
    /// whenever a sync is needed.
    pub async fn run<F, Fut>(mut self, sync_fn: F)
    where
        F: Fn(SyncRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<SyncResult>> + Send,
    {
        let mut request_rx = self.request_rx.take().expect("Handle can only be run once");
        let mut periodic_interval = self.create_periodic_interval().await;
        let mut last_sync = Instant::now();

        info!("Sync scheduler started");

        loop {
            // Check for shutdown
            if *self.shutdown.read().await {
                info!("Sync scheduler shutting down");
                break;
            }

            tokio::select! {
                // Handle incoming sync requests
                Some((request, response_tx)) = request_rx.recv() => {
                    match request {
                        SyncRequest::Shutdown => {
                            info!("Received shutdown request");
                            break;
                        }
                        _ => {
                            debug!("Processing sync request: {:?}", request);
                            let result = sync_fn(request).await;
                            last_sync = Instant::now();
                            let _ = response_tx.send(result);
                        }
                    }
                }

                // Handle periodic sync
                _ = Self::wait_for_periodic(&mut periodic_interval) => {
                    let mode = self.mode.read().await.clone();
                    match mode {
                        SyncMode::Periodic { .. } | SyncMode::Hybrid { .. } => {
                            debug!("Triggering periodic sync");
                            let result = sync_fn(SyncRequest::Full).await;
                            last_sync = Instant::now();
                            match &result {
                                Ok(sync_result) => {
                                    info!(
                                        "Periodic sync completed: {} synced, {} failed, {} conflicts",
                                        sync_result.files_synced,
                                        sync_result.files_failed,
                                        sync_result.conflicts_found
                                    );
                                }
                                Err(e) => {
                                    error!("Periodic sync failed: {}", e);
                                }
                            }
                        }
                        _ => {
                            // Update interval in case mode changed
                            periodic_interval = self.create_periodic_interval().await;
                        }
                    }
                }
            }

            // Recreate interval if mode changed
            let current_interval = self.get_interval_duration().await;
            let expected_interval = match &*self.mode.read().await {
                SyncMode::Periodic { interval } | SyncMode::Hybrid { interval } => Some(*interval),
                _ => None,
            };
            if current_interval != expected_interval {
                periodic_interval = self.create_periodic_interval().await;
            }
        }
    }

    async fn create_periodic_interval(&self) -> Option<tokio::time::Interval> {
        let mode = self.mode.read().await;
        match &*mode {
            SyncMode::Periodic { interval: duration } | SyncMode::Hybrid { interval: duration } => {
                Some(interval(*duration))
            }
            _ => None,
        }
    }

    async fn get_interval_duration(&self) -> Option<Duration> {
        let mode = self.mode.read().await;
        match &*mode {
            SyncMode::Periodic { interval } | SyncMode::Hybrid { interval } => Some(*interval),
            _ => None,
        }
    }

    async fn wait_for_periodic(interval: &mut Option<tokio::time::Interval>) {
        if let Some(ref mut interval) = interval {
            interval.tick().await;
        } else {
            // If no periodic sync, wait indefinitely
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }

    /// Notify the scheduler of a change (for on-demand mode).
    pub async fn notify_change(&self, paths: Vec<String>) {
        let mode = self.mode.read().await.clone();
        match mode {
            SyncMode::OnDemand | SyncMode::Hybrid { .. } => {
                let (response_tx, _) = oneshot::channel();
                let _ = self
                    .request_tx
                    .send((SyncRequest::Paths(paths), response_tx))
                    .await;
            }
            _ => {
                debug!("Change notification ignored (mode: {:?})", mode);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_scheduler_creation() {
        let (scheduler, _handle) = SyncScheduler::new(SyncMode::Manual);
        let mode = scheduler.get_mode().await;
        assert!(matches!(mode, SyncMode::Manual));
    }

    #[tokio::test]
    async fn test_mode_change() {
        let (scheduler, _handle) = SyncScheduler::new(SyncMode::Manual);

        scheduler
            .set_mode(SyncMode::Periodic {
                interval: Duration::from_secs(60),
            })
            .await;

        let mode = scheduler.get_mode().await;
        assert!(matches!(mode, SyncMode::Periodic { .. }));
    }

    #[tokio::test]
    async fn test_sync_request() {
        let (scheduler, handle) = SyncScheduler::new(SyncMode::OnDemand);

        let sync_count = Arc::new(AtomicU32::new(0));
        let count_clone = sync_count.clone();

        // Spawn the scheduler task
        let handle_task = tokio::spawn(async move {
            handle
                .run(move |_request| {
                    let count = count_clone.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        Ok(SyncResult {
                            files_synced: 1,
                            files_failed: 0,
                            conflicts_found: 0,
                            duration: Duration::from_millis(100),
                        })
                    }
                })
                .await;
        });

        // Request a sync
        let result = scheduler.request_sync().await.unwrap();
        assert_eq!(result.files_synced, 1);
        assert_eq!(sync_count.load(Ordering::SeqCst), 1);

        // Shutdown
        scheduler.shutdown().await;
        let _ = handle_task.await;
    }
}
