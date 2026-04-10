//! OAuth2 authentication and token management for Dropbox.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use oauth2::{
    basic::BasicClient, AuthUrl, ClientId, ClientSecret, RedirectUrl, TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use axiomvault_common::{Error, Result};

use crate::cloud_auth::{CloudTokenManager, CloudTokens, TokenRefresher};

/// Re-export `CloudTokens` as `DropboxTokens` for backward compatibility.
pub type DropboxTokens = CloudTokens;

/// Re-export `CloudTokenManager<DropboxAuthManager>` as `DropboxTokenManager`.
pub type DropboxTokenManager = CloudTokenManager<DropboxAuthManager>;

type OAuthClient = BasicClient<
    oauth2::EndpointSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointSet,
>;

/// OAuth2 authorization endpoint.
const DROPBOX_AUTH_URL: &str = "https://www.dropbox.com/oauth2/authorize";
/// OAuth2 token endpoint.
const DROPBOX_TOKEN_URL: &str = "https://api.dropboxapi.com/oauth2/token";
/// Redirect URL for OAuth2 flow.
const REDIRECT_URL: &str = "http://localhost:8080/callback";

/// Configuration for Dropbox OAuth2 authentication.
#[derive(Debug, Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct DropboxAuthConfig {
    /// Dropbox app key (client ID).
    pub app_key: String,
    /// Dropbox app secret (client secret).
    pub app_secret: String,
    /// Redirect URL for OAuth2 callback.
    #[zeroize(skip)]
    pub redirect_url: String,
}

impl Default for DropboxAuthConfig {
    fn default() -> Self {
        let app_key = std::env::var("AXIOMVAULT_DROPBOX_APP_KEY").unwrap_or_default();
        let app_secret = std::env::var("AXIOMVAULT_DROPBOX_APP_SECRET").unwrap_or_default();
        Self {
            app_key,
            app_secret,
            redirect_url: REDIRECT_URL.to_string(),
        }
    }
}

impl DropboxAuthConfig {
    /// Validate that required credentials are set.
    pub fn validate(&self) -> Result<()> {
        if self.app_key.is_empty() {
            return Err(Error::InvalidInput(
                "Dropbox app key not configured. \
                 Set the AXIOMVAULT_DROPBOX_APP_KEY environment variable."
                    .to_string(),
            ));
        }
        if self.app_secret.is_empty() {
            return Err(Error::InvalidInput(
                "Dropbox app secret not configured. \
                 Set the AXIOMVAULT_DROPBOX_APP_SECRET environment variable."
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// OAuth2 authentication manager for Dropbox.
pub struct DropboxAuthManager {
    client: OAuthClient,
    #[cfg_attr(not(test), allow(dead_code))]
    config: DropboxAuthConfig,
}

impl DropboxAuthManager {
    /// Create a new authentication manager.
    pub fn new(config: DropboxAuthConfig) -> Result<Self> {
        let client = BasicClient::new(ClientId::new(config.app_key.clone()))
            .set_client_secret(ClientSecret::new(config.app_secret.clone()))
            .set_auth_uri(
                AuthUrl::new(DROPBOX_AUTH_URL.to_string())
                    .map_err(|e| Error::InvalidInput(format!("Invalid auth URL: {}", e)))?,
            )
            .set_token_uri(
                TokenUrl::new(DROPBOX_TOKEN_URL.to_string())
                    .map_err(|e| Error::InvalidInput(format!("Invalid token URL: {}", e)))?,
            )
            .set_redirect_uri(
                RedirectUrl::new(config.redirect_url.clone())
                    .map_err(|e| Error::InvalidInput(format!("Invalid redirect URL: {}", e)))?,
            );

        Ok(Self { client, config })
    }

    /// Generate the authorization URL for the user to visit.
    pub fn authorization_url(&self) -> (String, String) {
        let (auth_url, csrf_token) = self
            .client
            .authorize_url(oauth2::CsrfToken::new_random)
            .add_extra_param("token_access_type", "offline")
            .url();

        (auth_url.to_string(), csrf_token.secret().clone())
    }

    /// Exchange an authorization code for tokens.
    pub async fn exchange_code(&self, code: &str) -> Result<DropboxTokens> {
        use oauth2::AuthorizationCode;

        let http_client = oauth2::reqwest::ClientBuilder::new()
            .redirect(oauth2::reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Error::Authentication(format!("Failed to build HTTP client: {}", e)))?;

        let token_result = self
            .client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .request_async(&http_client)
            .await
            .map_err(|e| Error::Authentication(format!("Token exchange failed: {}", e)))?;

        let access_token = token_result.access_token().secret().clone();
        let refresh_token = token_result
            .refresh_token()
            .ok_or_else(|| {
                Error::Authentication(
                    "No refresh token received. Ensure 'token_access_type=offline' was requested."
                        .to_string(),
                )
            })?
            .secret()
            .clone();

        let expires_in = token_result
            .expires_in()
            .unwrap_or_else(|| std::time::Duration::from_secs(14400));

        let expires_at =
            Utc::now() + Duration::from_std(expires_in).unwrap_or_else(|_| Duration::hours(4));

        Ok(DropboxTokens {
            access_token,
            refresh_token,
            expires_at,
        })
    }

    /// Get the current configuration (test-only).
    #[cfg(test)]
    pub(crate) fn config(&self) -> &DropboxAuthConfig {
        &self.config
    }
}

#[async_trait]
impl TokenRefresher for DropboxAuthManager {
    /// Refresh an access token using the refresh token.
    async fn refresh(&self, refresh_token: &str) -> Result<CloudTokens> {
        use oauth2::RefreshToken;

        let http_client = oauth2::reqwest::ClientBuilder::new()
            .redirect(oauth2::reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Error::Authentication(format!("Failed to build HTTP client: {}", e)))?;

        let refresh_token_value = refresh_token.to_string();
        let token_result = self
            .client
            .exchange_refresh_token(&RefreshToken::new(refresh_token_value.clone()))
            .request_async(&http_client)
            .await
            .map_err(|e| Error::Authentication(format!("Token refresh failed: {}", e)))?;

        let access_token = token_result.access_token().secret().clone();
        let new_refresh_token = token_result
            .refresh_token()
            .map(|t| t.secret().clone())
            .unwrap_or_else(|| refresh_token_value.clone());

        let expires_in = token_result
            .expires_in()
            .unwrap_or_else(|| std::time::Duration::from_secs(14400));

        let expires_at =
            Utc::now() + Duration::from_std(expires_in).unwrap_or_else(|_| Duration::hours(4));

        Ok(CloudTokens {
            access_token,
            refresh_token: new_refresh_token,
            expires_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokens_expiration() {
        let expired = DropboxTokens {
            access_token: "test".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() - Duration::hours(1),
        };
        assert!(expired.is_expired());

        let valid = DropboxTokens {
            access_token: "test".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
        };
        assert!(!valid.is_expired());
    }

    #[test]
    fn test_tokens_serialization() {
        let tokens = DropboxTokens {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now(),
        };
        let json = serde_json::to_string(&tokens).unwrap();
        let deserialized: DropboxTokens = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.access_token, tokens.access_token);
    }

    #[test]
    fn test_auth_config_serialization() {
        let config = DropboxAuthConfig {
            app_key: "key".to_string(),
            app_secret: "secret".to_string(),
            redirect_url: REDIRECT_URL.to_string(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: DropboxAuthConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.app_key, config.app_key);
    }

    #[test]
    fn test_auth_manager_creation() {
        let config = DropboxAuthConfig {
            app_key: "test_key".to_string(),
            app_secret: "test_secret".to_string(),
            redirect_url: "http://localhost:8080/callback".to_string(),
        };
        let manager = DropboxAuthManager::new(config).unwrap();
        assert_eq!(manager.config().app_key, "test_key");
    }

    #[test]
    fn test_authorization_url() {
        let config = DropboxAuthConfig {
            app_key: "test_key".to_string(),
            app_secret: "test_secret".to_string(),
            redirect_url: "http://localhost:8080/callback".to_string(),
        };
        let manager = DropboxAuthManager::new(config).unwrap();
        let (url, csrf) = manager.authorization_url();
        assert!(url.contains("dropbox.com"));
        assert!(url.contains("client_id=test_key"));
        assert!(url.contains("token_access_type=offline"));
        assert!(!csrf.is_empty());
    }
}
