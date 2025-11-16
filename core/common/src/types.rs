//! Common types used throughout AxiomVault.

use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroize;

/// Unique identifier for a vault.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VaultId(String);

impl VaultId {
    /// Create a new VaultId from a string.
    ///
    /// # Preconditions
    /// - `id` must be non-empty
    ///
    /// # Postconditions
    /// - Returns a valid VaultId instance
    ///
    /// # Errors
    /// - Returns error if id is empty
    pub fn new(id: impl Into<String>) -> crate::Result<Self> {
        let id = id.into();
        if id.is_empty() {
            return Err(crate::Error::InvalidInput(
                "VaultId cannot be empty".to_string(),
            ));
        }
        Ok(Self(id))
    }

    /// Get the inner string value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VaultId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A path within a vault, independent of underlying storage.
///
/// This type represents logical paths within the encrypted vault structure,
/// not physical filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VaultPath {
    components: Vec<String>,
}

impl VaultPath {
    /// Create a root path.
    pub fn root() -> Self {
        Self {
            components: Vec::new(),
        }
    }

    /// Create a path from string components.
    ///
    /// # Preconditions
    /// - Components must not contain path separators
    /// - Components must not be empty strings
    ///
    /// # Errors
    /// - Returns error if any component is invalid
    pub fn from_components(components: Vec<String>) -> crate::Result<Self> {
        for comp in &components {
            if comp.is_empty() {
                return Err(crate::Error::InvalidInput(
                    "Path component cannot be empty".to_string(),
                ));
            }
            if comp.contains('/') || comp.contains('\\') {
                return Err(crate::Error::InvalidInput(
                    "Path component cannot contain separators".to_string(),
                ));
            }
        }
        Ok(Self { components })
    }

    /// Parse a path string into VaultPath.
    ///
    /// Uses '/' as separator.
    pub fn parse(path: &str) -> crate::Result<Self> {
        if path.is_empty() || path == "/" {
            return Ok(Self::root());
        }

        let path = path.trim_start_matches('/').trim_end_matches('/');
        if path.is_empty() {
            return Ok(Self::root());
        }

        let components: Vec<String> = path.split('/').map(String::from).collect();
        Self::from_components(components)
    }

    /// Check if this is the root path.
    pub fn is_root(&self) -> bool {
        self.components.is_empty()
    }

    /// Get the parent path, if any.
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            None
        } else {
            let mut components = self.components.clone();
            components.pop();
            Some(Self { components })
        }
    }

    /// Get the file/directory name (last component).
    pub fn name(&self) -> Option<&str> {
        self.components.last().map(|s| s.as_str())
    }

    /// Join this path with a child component.
    pub fn join(&self, child: &str) -> crate::Result<Self> {
        if child.is_empty() {
            return Err(crate::Error::InvalidInput(
                "Child component cannot be empty".to_string(),
            ));
        }
        if child.contains('/') || child.contains('\\') {
            return Err(crate::Error::InvalidInput(
                "Child component cannot contain separators".to_string(),
            ));
        }
        let mut components = self.components.clone();
        components.push(child.to_string());
        Ok(Self { components })
    }

    /// Get the path components.
    pub fn components(&self) -> &[String] {
        &self.components
    }

    /// Convert to a string representation.
    pub fn to_string_path(&self) -> String {
        if self.is_root() {
            "/".to_string()
        } else {
            format!("/{}", self.components.join("/"))
        }
    }
}

impl fmt::Display for VaultPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string_path())
    }
}

/// Sensitive data wrapper that zeroizes on drop.
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct SensitiveBytes(Vec<u8>);

impl SensitiveBytes {
    /// Create new sensitive bytes.
    pub fn new(data: Vec<u8>) -> Self {
        Self(data)
    }

    /// Get a reference to the inner bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Get the length.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SensitiveBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SensitiveBytes([REDACTED; {} bytes])", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vault_id_creation() {
        let id = VaultId::new("test-vault").unwrap();
        assert_eq!(id.as_str(), "test-vault");
    }

    #[test]
    fn test_vault_id_empty_fails() {
        assert!(VaultId::new("").is_err());
    }

    #[test]
    fn test_vault_path_root() {
        let path = VaultPath::root();
        assert!(path.is_root());
        assert_eq!(path.to_string_path(), "/");
    }

    #[test]
    fn test_vault_path_parse() {
        let path = VaultPath::parse("/foo/bar/baz").unwrap();
        assert_eq!(path.components(), &["foo", "bar", "baz"]);
        assert_eq!(path.to_string_path(), "/foo/bar/baz");
    }

    #[test]
    fn test_vault_path_join() {
        let path = VaultPath::root().join("foo").unwrap().join("bar").unwrap();
        assert_eq!(path.to_string_path(), "/foo/bar");
    }

    #[test]
    fn test_vault_path_parent() {
        let path = VaultPath::parse("/foo/bar").unwrap();
        let parent = path.parent().unwrap();
        assert_eq!(parent.to_string_path(), "/foo");
    }

    #[test]
    fn test_vault_path_name() {
        let path = VaultPath::parse("/foo/bar").unwrap();
        assert_eq!(path.name(), Some("bar"));
    }
}
