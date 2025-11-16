//! Provider registry for dynamic provider resolution.

use std::collections::HashMap;
use std::sync::Arc;
use serde_json::Value;

use axiomvault_common::{Error, Result};
use crate::provider::StorageProvider;

/// Factory function type for creating providers.
pub type ProviderFactory = Box<dyn Fn(Value) -> Result<Arc<dyn StorageProvider>> + Send + Sync>;

/// Registry for storage provider factories.
///
/// Allows dynamic registration and resolution of storage providers
/// by name and configuration.
pub struct ProviderRegistry {
    factories: HashMap<String, ProviderFactory>,
}

impl ProviderRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    /// Register a provider factory.
    ///
    /// # Preconditions
    /// - `name` must be unique within the registry
    ///
    /// # Postconditions
    /// - Factory is registered and can be resolved by name
    ///
    /// # Errors
    /// - Returns error if name is already registered
    pub fn register(
        &mut self,
        name: impl Into<String>,
        factory: ProviderFactory,
    ) -> Result<()> {
        let name = name.into();
        if self.factories.contains_key(&name) {
            return Err(Error::AlreadyExists(format!(
                "Provider '{}' is already registered",
                name
            )));
        }
        self.factories.insert(name, factory);
        Ok(())
    }

    /// Resolve a provider by name and configuration.
    ///
    /// # Preconditions
    /// - Provider must be registered
    /// - Configuration must be valid for the provider
    ///
    /// # Postconditions
    /// - Returns an instance of the provider
    ///
    /// # Errors
    /// - Provider not found
    /// - Configuration invalid
    pub fn resolve(&self, name: &str, config: Value) -> Result<Arc<dyn StorageProvider>> {
        let factory = self.factories.get(name).ok_or_else(|| {
            Error::NotFound(format!("Provider '{}' is not registered", name))
        })?;
        factory(config)
    }

    /// Get list of registered provider names.
    pub fn providers(&self) -> Vec<String> {
        self.factories.keys().cloned().collect()
    }

    /// Check if a provider is registered.
    pub fn has_provider(&self, name: &str) -> bool {
        self.factories.contains_key(name)
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a registry with default providers.
pub fn create_default_registry() -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();

    // Register memory provider (for testing)
    registry
        .register("memory", Box::new(|_config| {
            Ok(Arc::new(crate::memory::MemoryProvider::new()))
        }))
        .expect("Failed to register memory provider");

    // Register local filesystem provider
    registry
        .register("local", Box::new(|config| {
            let root = config
                .get("root")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::InvalidInput("Local provider requires 'root' path".to_string()))?;
            Ok(Arc::new(crate::local::LocalProvider::new(root)?))
        }))
        .expect("Failed to register local provider");

    // Register Google Drive provider
    registry
        .register("gdrive", Box::new(|config| {
            crate::gdrive::create_gdrive_provider(config)
        }))
        .expect("Failed to register gdrive provider");

    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryProvider;

    #[test]
    fn test_register_and_resolve() {
        let mut registry = ProviderRegistry::new();

        registry
            .register("test", Box::new(|_| Ok(Arc::new(MemoryProvider::new()))))
            .unwrap();

        let provider = registry.resolve("test", Value::Null).unwrap();
        assert_eq!(provider.name(), "memory");
    }

    #[test]
    fn test_duplicate_registration_fails() {
        let mut registry = ProviderRegistry::new();

        registry
            .register("test", Box::new(|_| Ok(Arc::new(MemoryProvider::new()))))
            .unwrap();

        let result = registry.register("test", Box::new(|_| Ok(Arc::new(MemoryProvider::new()))));
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_unknown_fails() {
        let registry = ProviderRegistry::new();
        let result = registry.resolve("unknown", Value::Null);
        assert!(result.is_err());
    }

    #[test]
    fn test_providers_list() {
        let mut registry = ProviderRegistry::new();
        registry
            .register("a", Box::new(|_| Ok(Arc::new(MemoryProvider::new()))))
            .unwrap();
        registry
            .register("b", Box::new(|_| Ok(Arc::new(MemoryProvider::new()))))
            .unwrap();

        let providers = registry.providers();
        assert!(providers.contains(&"a".to_string()));
        assert!(providers.contains(&"b".to_string()));
    }
}
