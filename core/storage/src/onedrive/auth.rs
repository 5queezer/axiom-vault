//! OAuth2 authentication and token management for OneDrive.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use oauth2::{
    basic::BasicClient, AuthUrl, ClientId, ClientSecret, RedirectUrl, Scope, TokenResponse,
    TokenUrl,
};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use axiomvault_common::{Error, Result};

use crate::cloud_auth::{CloudTokenManager, CloudTokens, TokenRefresher};

/// Re-export `CloudTokens` as `OneDriveTokens` for backward compatibility.
pub type OneDriveTokens = CloudTokens;

/// Re-export `CloudTokenManager<OneDriveAuthManager>` as `OneDriveTokenManager`.
pub type OneDriveTokenManager = CloudTokenManager<OneDriveAuthManager>;

type OAuthClient = BasicClient<
    oauth2::EndpointSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointSet,
>;

/// Microsoft identity platform authorization endpoint (consumers tenant).
const MS_AUTH_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
/// Microsoft identity platform token endpoint.
const MS_TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
/// Redirect URL for OAuth2 flow.
const REDIRECT_URL: &str = "http://localhost:8080/callback";

/// Required scopes for OneDrive file access.
const ONEDRIVE_SCOPES: &[&str] = &["Files.ReadWrite", "offline_access"];

/// Configuration for OneDrive OAuth2 authentication.
#[derive(Debug, Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct OneDriveAuthConfig {
    /// Azure AD application (client) ID.
    pub client_id: String,
    /// Client secret.
    pub client_secret: String,
    /// Redirect URL for OAuth2 callback.
    #[zeroize(skip)]
    pub redirect_url: String,
}

impl Default for OneDriveAuthConfig {
    fn default() -> Self {
        let client_id = std::env::var("AXIOMVAULT_ONEDRIVE_CLIENT_ID").unwrap_or_default();
        let client_secret = std::env::var("AXIOMVAULT_ONEDRIVE_CLIENT_SECRET").unwrap_or_default();
        Self {
            client_id,
            client_secret,
            redirect_url: REDIRECT_URL.to_string(),
        }
    }
}

impl OneDriveAuthConfig {
    /// Validate that required credentials are set.
    pub fn validate(&self) -> Result<()> {
        if self.client_id.is_empty() {
            return Err(Error::InvalidInput(
                "OneDrive client ID not configured. \
                 Set the AXIOMVAULT_ONEDRIVE_CLIENT_ID environment variable."
                    .to_string(),
            ));
        }
        if self.client_secret.is_empty() {
            return Err(Error::InvalidInput(
                "OneDrive client secret not configured. \
                 Set the AXIOMVAULT_ONEDRIVE_CLIENT_SECRET environment variable."
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// OAuth2 authentication manager for OneDrive.
pub struct OneDriveAuthManager {
    client: OAuthClient,
    #[cfg_attr(not(test), allow(dead_code))]
    config: OneDriveAuthConfig,
}

impl OneDriveAuthManager {
    /// Create a new authentication manager.
    pub fn new(config: OneDriveAuthConfig) -> Result<Self> {
        let client = BasicClient::new(ClientId::new(config.client_id.clone()))
            .set_client_secret(ClientSecret::new(config.client_secret.clone()))
            .set_auth_uri(
                AuthUrl::new(MS_AUTH_URL.to_string())
                    .map_err(|e| Error::InvalidInput(format!("Invalid auth URL: {}", e)))?,
            )
            .set_token_uri(
                TokenUrl::new(MS_TOKEN_URL.to_string())
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
        let mut auth_request = self.client.authorize_url(oauth2::CsrfToken::new_random);

        for scope in ONEDRIVE_SCOPES {
            auth_request = auth_request.add_scope(Scope::new(scope.to_string()));
        }

        let (auth_url, csrf_token) = auth_request.url();
        (auth_url.to_string(), csrf_token.secret().clone())
    }

    /// Exchange an authorization code for tokens.
    pub async fn exchange_code(&self, code: &str) -> Result<OneDriveTokens> {
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
                    "No refresh token received. Ensure 'offline_access' scope was requested."
                        .to_string(),
                )
            })?
            .secret()
            .clone();

        let expires_in = token_result
            .expires_in()
            .unwrap_or_else(|| std::time::Duration::from_secs(3600));

        let expires_at =
            Utc::now() + Duration::from_std(expires_in).unwrap_or_else(|_| Duration::hours(1));

        Ok(OneDriveTokens {
            access_token,
            refresh_token,
            expires_at,
        })
    }

    /// Get the current configuration (test-only).
    #[cfg(test)]
    pub(crate) fn config(&self) -> &OneDriveAuthConfig {
        &self.config
    }
}

#[async_trait]
impl TokenRefresher for OneDriveAuthManager {
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
            .unwrap_or_else(|| std::time::Duration::from_secs(3600));

        let expires_at =
            Utc::now() + Duration::from_std(expires_in).unwrap_or_else(|_| Duration::hours(1));

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
        let expired = OneDriveTokens {
            access_token: "test".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() - Duration::hours(1),
        };
        assert!(expired.is_expired());

        let valid = OneDriveTokens {
            access_token: "test".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
        };
        assert!(!valid.is_expired());
    }

    #[test]
    fn test_tokens_serialization() {
        let tokens = OneDriveTokens {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now(),
        };
        let json = serde_json::to_string(&tokens).unwrap();
        let deserialized: OneDriveTokens = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.access_token, tokens.access_token);
    }

    #[test]
    fn test_auth_config_serialization() {
        let config = OneDriveAuthConfig {
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
            redirect_url: REDIRECT_URL.to_string(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: OneDriveAuthConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.client_id, config.client_id);
    }

    #[test]
    fn test_auth_manager_creation() {
        let config = OneDriveAuthConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            redirect_url: "http://localhost:8080/callback".to_string(),
        };
        let manager = OneDriveAuthManager::new(config).unwrap();
        assert_eq!(manager.config().client_id, "test_id");
    }

    #[test]
    fn test_authorization_url() {
        let config = OneDriveAuthConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            redirect_url: "http://localhost:8080/callback".to_string(),
        };
        let manager = OneDriveAuthManager::new(config).unwrap();
        let (url, csrf) = manager.authorization_url();
        assert!(url.contains("login.microsoftonline.com"));
        assert!(url.contains("client_id=test_id"));
        assert!(url.contains("Files.ReadWrite"));
        assert!(!csrf.is_empty());
    }
}
