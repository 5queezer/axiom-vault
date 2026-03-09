//! iCloud Drive storage provider for AxiomVault.
//!
//! Uses the local iCloud Drive mount point on macOS. iCloud Drive does not
//! expose a public REST API, so this provider delegates to `LocalProvider`
//! using the auto-detected (or user-specified) iCloud Drive folder.

pub mod provider;

pub use provider::{create_icloud_provider, ICloudConfig, ICloudProvider};

/// Detect the iCloud Drive mount point on macOS.
///
/// Returns the path to `~/Library/Mobile Documents/com~apple~CloudDocs/`
/// if it exists.
#[cfg(target_os = "macos")]
pub fn detect_icloud_path() -> Option<std::path::PathBuf> {
    if let Some(home) = dirs::home_dir() {
        let icloud_path = home
            .join("Library")
            .join("Mobile Documents")
            .join("com~apple~CloudDocs");
        if icloud_path.exists() {
            return Some(icloud_path);
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub fn detect_icloud_path() -> Option<std::path::PathBuf> {
    None
}
