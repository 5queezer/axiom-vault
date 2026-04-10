//! Shared OAuth2 token types and token management for cloud storage providers.
//!
//! All cloud providers (Google Drive, Dropbox, OneDrive) use the same pattern:
//! - A `Tokens` struct with access token, refresh token, and expiration
//! - A `TokenManager` that auto-refreshes expired tokens with double-check locking
//!
//! Provider-specific auth managers implement [`TokenRefresher`] to plug into
//! the generic [`CloudTokenManager`].

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use axiomvault_common::Result;

/// OAuth2 tokens with expiration tracking.
///
/// Common across all cloud providers. Contains the access token for API
/// requests, a refresh token for renewal, and an expiration timestamp.
#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct CloudTokens {
    /// Access token for API requests.
    pub access_token: String,
    /// Refresh token for obtaining new access tokens.
    pub refresh_token: String,
    /// When the access token expires.
    #[zeroize(skip)]
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for CloudTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudTokens")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl CloudTokens {
    /// Check if the access token is expired or about to expire.
    ///
    /// Uses a 5-minute buffer to avoid using a token that will expire
    /// during a request.
    pub fn is_expired(&self) -> bool {
        self.expires_at <= Utc::now() + Duration::minutes(5)
    }
}

/// Trait for provider-specific token refresh logic.
///
/// Each cloud provider has its own OAuth2 endpoints and token format.
/// Implement this trait to provide the refresh behavior, then wrap it
/// in a [`CloudTokenManager`] for automatic refresh with double-check locking.
#[async_trait]
pub trait TokenRefresher: Send + Sync {
    /// Refresh an access token using the given refresh token.
    ///
    /// Returns new tokens (the refresh token itself may or may not be rotated).
    async fn refresh(&self, refresh_token: &str) -> Result<CloudTokens>;
}

/// Generic token manager with automatic refresh via double-check locking.
///
/// Wraps any [`TokenRefresher`] implementation and provides thread-safe
/// access to a valid access token, refreshing automatically when expired.
pub struct CloudTokenManager<R: TokenRefresher> {
    refresher: R,
    tokens: tokio::sync::RwLock<CloudTokens>,
}

impl<R: TokenRefresher> CloudTokenManager<R> {
    /// Create a new token manager with initial tokens.
    pub fn new(refresher: R, tokens: CloudTokens) -> Self {
        Self {
            refresher,
            tokens: tokio::sync::RwLock::new(tokens),
        }
    }

    /// Get a valid access token, refreshing if necessary.
    ///
    /// Uses double-check locking: first acquires a read lock to check
    /// expiration, then upgrades to a write lock only when refresh is needed.
    /// After acquiring the write lock, re-checks expiration to avoid
    /// redundant refreshes from concurrent callers.
    pub async fn get_access_token(&self) -> Result<String> {
        let tokens = self.tokens.read().await;
        if !tokens.is_expired() {
            return Ok(tokens.access_token.clone());
        }
        drop(tokens);

        let mut tokens = self.tokens.write().await;
        // Double-check after acquiring write lock
        if !tokens.is_expired() {
            return Ok(tokens.access_token.clone());
        }

        tracing::info!("Refreshing expired access token");
        let new_tokens = self.refresher.refresh(&tokens.refresh_token).await?;
        *tokens = new_tokens;
        Ok(tokens.access_token.clone())
    }

    /// Get the current tokens (e.g. for persistence).
    pub async fn get_tokens(&self) -> CloudTokens {
        self.tokens.read().await.clone()
    }

    /// Replace the current tokens (e.g. after manual refresh).
    pub async fn update_tokens(&self, tokens: CloudTokens) {
        *self.tokens.write().await = tokens;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cloud_tokens_expiration() {
        let expired = CloudTokens {
            access_token: "test".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() - Duration::hours(1),
        };
        assert!(expired.is_expired());

        let valid = CloudTokens {
            access_token: "test".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
        };
        assert!(!valid.is_expired());
    }

    #[test]
    fn test_cloud_tokens_near_expiration() {
        // Token expiring in 4 minutes should be considered expired (5 min buffer)
        let tokens = CloudTokens {
            access_token: "test".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() + Duration::minutes(4),
        };
        assert!(tokens.is_expired());
    }

    #[test]
    fn test_cloud_tokens_serialization() {
        let tokens = CloudTokens {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now(),
        };
        let json = serde_json::to_string(&tokens).unwrap();
        let deserialized: CloudTokens = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.access_token, tokens.access_token);
        assert_eq!(deserialized.refresh_token, tokens.refresh_token);
    }

    /// Dummy refresher for testing the token manager.
    struct TestRefresher;

    #[async_trait]
    impl TokenRefresher for TestRefresher {
        async fn refresh(&self, _refresh_token: &str) -> Result<CloudTokens> {
            Ok(CloudTokens {
                access_token: "refreshed".to_string(),
                refresh_token: "new_refresh".to_string(),
                expires_at: Utc::now() + Duration::hours(1),
            })
        }
    }

    #[tokio::test]
    async fn test_cloud_token_manager_returns_valid_token() {
        let tokens = CloudTokens {
            access_token: "valid".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
        };
        let manager = CloudTokenManager::new(TestRefresher, tokens);
        let token = manager.get_access_token().await.unwrap();
        assert_eq!(token, "valid");
    }

    #[tokio::test]
    async fn test_cloud_token_manager_refreshes_expired_token() {
        let tokens = CloudTokens {
            access_token: "expired".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() - Duration::hours(1),
        };
        let manager = CloudTokenManager::new(TestRefresher, tokens);
        let token = manager.get_access_token().await.unwrap();
        assert_eq!(token, "refreshed");
    }

    #[tokio::test]
    async fn test_cloud_token_manager_update_tokens() {
        let tokens = CloudTokens {
            access_token: "old".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
        };
        let manager = CloudTokenManager::new(TestRefresher, tokens);

        let new_tokens = CloudTokens {
            access_token: "new".to_string(),
            refresh_token: "new_refresh".to_string(),
            expires_at: Utc::now() + Duration::hours(2),
        };
        manager.update_tokens(new_tokens).await;

        let token = manager.get_access_token().await.unwrap();
        assert_eq!(token, "new");
    }
}
