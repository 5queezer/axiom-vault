//! Fan-out and try-first helpers for the composite storage provider.
//!
//! These methods distribute operations across multiple backends,
//! either writing to all (fan-out) or reading from the first success (try-first).

use std::future::Future;
use std::sync::Arc;
use tracing::warn;

use crate::provider::{Metadata, StorageProvider};
use axiomvault_common::{Error, Result};

use super::CompositeStorageProvider;

impl CompositeStorageProvider {
    /// Fan out a `Result<Metadata>` operation to all backends concurrently.
    /// Returns the first successful result and the indices of backends that
    /// succeeded; fails only if ALL backends fail.
    pub(crate) async fn fan_out<F, Fut>(&self, op: &str, f: F) -> Result<(Metadata, Vec<usize>)>
    where
        F: Fn(Arc<dyn StorageProvider>) -> Fut,
        Fut: Future<Output = Result<Metadata>>,
    {
        // Wrap each future to measure per-backend latency.
        let futures: Vec<_> = self
            .backends
            .iter()
            .map(|b| {
                let fut = f(Arc::clone(b));
                async move {
                    let start = tokio::time::Instant::now();
                    let result = fut.await;
                    (result, start.elapsed())
                }
            })
            .collect();
        let results = futures::future::join_all(futures).await;

        let mut first_success: Option<Metadata> = None;
        let mut succeeded: Vec<usize> = Vec::new();
        let mut last_error: Option<Error> = None;
        let mut failure_count = 0usize;

        for (i, (result, latency)) in results.into_iter().enumerate() {
            match result {
                Ok(meta) => {
                    self.record_health_success(i, latency).await;
                    succeeded.push(i);
                    if first_success.is_none() {
                        first_success = Some(meta);
                    }
                }
                Err(e) => {
                    self.record_health_failure(i).await;
                    failure_count += 1;
                    warn!(
                        backend = self.backends[i].name(),
                        operation = op,
                        error = %e,
                        "Backend write failed"
                    );
                    last_error = Some(e);
                }
            }
        }

        if failure_count > 0 && first_success.is_some() {
            warn!(
                operation = op,
                failed = failure_count,
                total = self.backends.len(),
                "Partial write: {}/{} backends failed",
                failure_count,
                self.backends.len()
            );
            self.check_redundancy_warning().await;
        }

        first_success.map(|meta| (meta, succeeded)).ok_or_else(|| {
            last_error.unwrap_or_else(|| Error::Storage(format!("All backends failed for {}", op)))
        })
    }

    /// Fan out a `Result<()>` operation to all backends concurrently.
    pub(crate) async fn fan_out_void<F, Fut>(&self, op: &str, f: F) -> Result<()>
    where
        F: Fn(Arc<dyn StorageProvider>) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let futures: Vec<_> = self
            .backends
            .iter()
            .map(|b| {
                let fut = f(Arc::clone(b));
                async move {
                    let start = tokio::time::Instant::now();
                    let result = fut.await;
                    (result, start.elapsed())
                }
            })
            .collect();
        let results = futures::future::join_all(futures).await;

        let mut any_success = false;
        let mut last_error: Option<Error> = None;
        let mut failure_count = 0usize;

        for (i, (result, latency)) in results.into_iter().enumerate() {
            match result {
                Ok(()) => {
                    self.record_health_success(i, latency).await;
                    any_success = true;
                }
                Err(e) => {
                    self.record_health_failure(i).await;
                    failure_count += 1;
                    warn!(
                        backend = self.backends[i].name(),
                        operation = op,
                        error = %e,
                        "Backend write failed"
                    );
                    last_error = Some(e);
                }
            }
        }

        if failure_count > 0 && any_success {
            warn!(
                operation = op,
                failed = failure_count,
                total = self.backends.len(),
                "Partial write: {}/{} backends failed",
                failure_count,
                self.backends.len()
            );
            self.check_redundancy_warning().await;
        }

        if any_success {
            Ok(())
        } else {
            Err(last_error
                .unwrap_or_else(|| Error::Storage(format!("All backends failed for {}", op))))
        }
    }

    /// Try backends in priority order, returning the first success.
    ///
    /// Healthy backends are tried first. Degraded/offline backends are only
    /// attempted if all healthy ones fail (prevents total lockout).
    /// Only warns on errors that are not `NotFound` (which is a normal result).
    pub(crate) async fn try_first<T, F, Fut>(&self, op: &str, f: F) -> Result<T>
    where
        F: Fn(Arc<dyn StorageProvider>) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.probe_if_due().await;

        let healthy_indices = self.healthy_backend_indices().await;
        let mut last_error: Option<Error> = None;

        // First pass: try healthy backends
        for &i in &healthy_indices {
            let start = tokio::time::Instant::now();
            match f(Arc::clone(&self.backends[i])).await {
                Ok(val) => {
                    self.record_health_success(i, start.elapsed()).await;
                    return Ok(val);
                }
                Err(e) => {
                    if !matches!(&e, Error::NotFound(_)) {
                        self.record_health_failure(i).await;
                        warn!(
                            backend = self.backends[i].name(),
                            operation = op,
                            error = %e,
                            "Backend read failed, trying next"
                        );
                    }
                    last_error = Some(e);
                }
            }
        }

        // Second pass: try remaining (degraded) backends not already tried
        for (i, backend) in self.backends.iter().enumerate() {
            if healthy_indices.contains(&i) {
                continue;
            }
            let start = tokio::time::Instant::now();
            match f(Arc::clone(backend)).await {
                Ok(val) => {
                    self.record_health_success(i, start.elapsed()).await;
                    return Ok(val);
                }
                Err(e) => {
                    if !matches!(&e, Error::NotFound(_)) {
                        self.record_health_failure(i).await;
                        warn!(
                            backend = backend.name(),
                            operation = op,
                            error = %e,
                            "Degraded backend read also failed"
                        );
                    }
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| Error::Storage(format!("All backends failed for {}", op))))
    }
}
