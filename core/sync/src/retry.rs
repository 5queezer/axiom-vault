//! Retry strategy with exponential backoff for transient errors.

use std::future::Future;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, warn};

use axiomvault_common::{Result, Error};

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts.
    pub max_retries: u32,
    /// Initial delay between retries.
    pub initial_delay: Duration,
    /// Maximum delay (cap for exponential growth).
    pub max_delay: Duration,
    /// Multiplier for exponential backoff.
    pub backoff_multiplier: f64,
    /// Whether to add jitter to delays.
    pub jitter: bool,
}

impl RetryConfig {
    /// Create a new retry configuration.
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }

    /// Set initial delay.
    pub fn with_initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    /// Set maximum delay.
    pub fn with_max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Set backoff multiplier.
    pub fn with_backoff_multiplier(mut self, multiplier: f64) -> Self {
        self.backoff_multiplier = multiplier;
        self
    }

    /// Enable or disable jitter.
    pub fn with_jitter(mut self, jitter: bool) -> Self {
        self.jitter = jitter;
        self
    }

    /// Calculate delay for a given attempt number.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base_delay = self.initial_delay.as_millis() as f64
            * self.backoff_multiplier.powi(attempt as i32);

        let capped_delay = base_delay.min(self.max_delay.as_millis() as f64);

        let final_delay = if self.jitter {
            // Add random jitter of +/- 25%
            let jitter_factor = 0.75 + (rand::random::<f64>() * 0.5);
            capped_delay * jitter_factor
        } else {
            capped_delay
        };

        Duration::from_millis(final_delay as u64)
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self::new(3)
    }
}

/// Retry executor for running operations with retry logic.
pub struct RetryExecutor {
    config: RetryConfig,
}

impl RetryExecutor {
    /// Create a new retry executor.
    pub fn new(config: RetryConfig) -> Self {
        Self { config }
    }

    /// Execute an operation with retry logic.
    ///
    /// The operation will be retried according to the config if it fails
    /// with a transient error.
    pub async fn execute<F, Fut, T>(&self, operation: F) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut attempt = 0;
        let mut last_error: Option<Error> = None;

        loop {
            match operation().await {
                Ok(result) => {
                    if attempt > 0 {
                        debug!("Operation succeeded after {} retries", attempt);
                    }
                    return Ok(result);
                }
                Err(err) => {
                    if !self.is_retryable(&err) {
                        return Err(err);
                    }

                    attempt += 1;
                    if attempt > self.config.max_retries {
                        warn!(
                            "Operation failed after {} attempts: {}",
                            self.config.max_retries, err
                        );
                        return Err(last_error.unwrap_or(err));
                    }

                    let delay = self.config.delay_for_attempt(attempt - 1);
                    warn!(
                        "Attempt {} failed: {}. Retrying in {:?}...",
                        attempt, err, delay
                    );

                    last_error = Some(err);
                    sleep(delay).await;
                }
            }
        }
    }

    /// Execute with a custom retry condition.
    pub async fn execute_with_condition<F, Fut, T, C>(
        &self,
        operation: F,
        should_retry: C,
    ) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T>>,
        C: Fn(&Error) -> bool,
    {
        let mut attempt = 0;
        let mut last_error: Option<Error> = None;

        loop {
            match operation().await {
                Ok(result) => {
                    if attempt > 0 {
                        debug!("Operation succeeded after {} retries", attempt);
                    }
                    return Ok(result);
                }
                Err(err) => {
                    if !should_retry(&err) {
                        return Err(err);
                    }

                    attempt += 1;
                    if attempt > self.config.max_retries {
                        warn!(
                            "Operation failed after {} attempts: {}",
                            self.config.max_retries, err
                        );
                        return Err(last_error.unwrap_or(err));
                    }

                    let delay = self.config.delay_for_attempt(attempt - 1);
                    warn!(
                        "Attempt {} failed: {}. Retrying in {:?}...",
                        attempt, err, delay
                    );

                    last_error = Some(err);
                    sleep(delay).await;
                }
            }
        }
    }

    /// Check if an error is retryable.
    fn is_retryable(&self, err: &Error) -> bool {
        matches!(
            err,
            Error::Network(_) | Error::Io(_)
        )
    }

    /// Get the retry configuration.
    pub fn config(&self) -> &RetryConfig {
        &self.config
    }
}

impl Default for RetryExecutor {
    fn default() -> Self {
        Self::new(RetryConfig::default())
    }
}

/// Helper trait for adding retry capability to operations.
#[async_trait::async_trait]
pub trait WithRetry {
    /// Execute this operation with default retry config.
    async fn with_retry<F, Fut, T>(operation: F) -> Result<T>
    where
        F: Fn() -> Fut + Send + Sync,
        Fut: Future<Output = Result<T>> + Send,
        T: Send;

    /// Execute this operation with custom retry config.
    async fn with_retry_config<F, Fut, T>(config: RetryConfig, operation: F) -> Result<T>
    where
        F: Fn() -> Fut + Send + Sync,
        Fut: Future<Output = Result<T>> + Send,
        T: Send;
}

/// Convenience function for simple retry with defaults.
pub async fn retry<F, Fut, T>(operation: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    RetryExecutor::default().execute(operation).await
}

/// Convenience function for retry with custom config.
pub async fn retry_with_config<F, Fut, T>(config: RetryConfig, operation: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    RetryExecutor::new(config).execute(operation).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_retry_config_delay_calculation() {
        let config = RetryConfig::new(3)
            .with_initial_delay(Duration::from_secs(1))
            .with_backoff_multiplier(2.0)
            .with_jitter(false);

        let delay0 = config.delay_for_attempt(0);
        let delay1 = config.delay_for_attempt(1);
        let delay2 = config.delay_for_attempt(2);

        assert_eq!(delay0, Duration::from_secs(1));
        assert_eq!(delay1, Duration::from_secs(2));
        assert_eq!(delay2, Duration::from_secs(4));
    }

    #[test]
    fn test_max_delay_cap() {
        let config = RetryConfig::new(10)
            .with_initial_delay(Duration::from_secs(1))
            .with_max_delay(Duration::from_secs(10))
            .with_backoff_multiplier(10.0)
            .with_jitter(false);

        // 1 * 10^5 = 100000 seconds, but should be capped at 10
        let delay = config.delay_for_attempt(5);
        assert_eq!(delay, Duration::from_secs(10));
    }

    #[tokio::test]
    async fn test_successful_operation() {
        let executor = RetryExecutor::default();

        let result: Result<i32> = executor.execute(|| async { Ok(42) }).await;

        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_on_network_error() {
        let attempt_count = Arc::new(AtomicU32::new(0));
        let count_clone = attempt_count.clone();

        let config = RetryConfig::new(3)
            .with_initial_delay(Duration::from_millis(1))
            .with_jitter(false);
        let executor = RetryExecutor::new(config);

        let result: Result<i32> = executor
            .execute(move || {
                let count = count_clone.clone();
                async move {
                    let current = count.fetch_add(1, Ordering::SeqCst);
                    if current < 2 {
                        Err(Error::Network("Connection failed".to_string()))
                    } else {
                        Ok(42)
                    }
                }
            })
            .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempt_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_non_retryable_error() {
        let attempt_count = Arc::new(AtomicU32::new(0));
        let count_clone = attempt_count.clone();

        let executor = RetryExecutor::default();

        let result: Result<i32> = executor
            .execute(move || {
                let count = count_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Err(Error::NotFound("File not found".to_string()))
                }
            })
            .await;

        assert!(result.is_err());
        // Should only try once because NotFound is not retryable
        assert_eq!(attempt_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_max_retries_exceeded() {
        let attempt_count = Arc::new(AtomicU32::new(0));
        let count_clone = attempt_count.clone();

        let config = RetryConfig::new(2).with_initial_delay(Duration::from_millis(1));
        let executor = RetryExecutor::new(config);

        let result: Result<i32> = executor
            .execute(move || {
                let count = count_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Err(Error::Network("Always fails".to_string()))
                }
            })
            .await;

        assert!(result.is_err());
        // Initial + 2 retries = 3 attempts
        assert_eq!(attempt_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_convenience_retry_function() {
        let result: Result<String> = retry(|| async { Ok("success".to_string()) }).await;
        assert_eq!(result.unwrap(), "success");
    }
}
