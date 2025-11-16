//! Tokio runtime management for FFI
//!
//! Provides a global async runtime for FFI functions.

use once_cell::sync::OnceCell;
use std::sync::Arc;
use tokio::runtime::Runtime;

static RUNTIME: OnceCell<Arc<Runtime>> = OnceCell::new();

/// Get or create the global Tokio runtime.
pub fn get_runtime() -> Result<Arc<Runtime>, String> {
    RUNTIME
        .get_or_try_init(|| {
            Runtime::new()
                .map(Arc::new)
                .map_err(|e| format!("Failed to create Tokio runtime: {}", e))
        })
        .cloned()
}
