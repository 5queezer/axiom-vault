//! OAuth2 authentication and token management for Google Drive.

use chrono::{DateTime, Duration, Utc};
use oauth2::{
    basic::BasicClient, AuthUrl, ClientId, ClientSecret, RedirectUrl, Scope, TokenResponse,
    TokenUrl,
};
use serde::{Deserialize, Serialize};

use axiomvault_common::{Error, Result};

/// OAuth2 client ID for Google Drive API.
const GOOGLE_CLIENT_ID: &str = "YOUR_CLIENT_ID";
/// OAuth2 client secret (note: in production, this should be securely managed).
const GOOGLE_CLIENT_SECRET: &str = "YOUR_CLIENT_SECRET";
/// OAuth2 authorization endpoint.
const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// OAuth2 token endpoint.
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// Redirect URL for OAuth2 flow (localhost for desktop apps).
const REDIRECT_URL: &str = "http://localhost:8080/callback";

/// Google Drive OAuth2 scopes.
const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

/// OAuth2 tokens with expiration tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    /// Access token for API requests.
    pub access_token: String,
    /// Refresh token for obtaining new access tokens.
    pub refresh_token: String,
    /// When the access token expires.
    pub expires_at: DateTime<Utc>,
}

impl Tokens {
    /// Check if the access token is expired or about to expire.
    pub fn is_expired(&self) -> bool {
        // Consider expired if less than 5 minutes remaining
        self.expires_at < Utc::now() + Duration::minutes(5)
    }
}

/// Configuration for OAuth2 authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Client ID (can be overridden from default).
    pub client_id: String,
    /// Client secret (can be overridden from default).
    pub client_secret: String,
    /// Redirect URL for OAuth2 callback.
    pub redirect_url: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            client_id: GOOGLE_CLIENT_ID.to_string(),
            client_secret: GOOGLE_CLIENT_SECRET.to_string(),
            redirect_url: REDIRECT_URL.to_string(),
        }
    }
}

/// OAuth2 authentication manager for Google Drive.
pub struct AuthManager {
    client: BasicClient,
    config: AuthConfig,
}

impl AuthManager {
    /// Create a new authentication manager.
    pub fn new(config: AuthConfig) -> Result<Self> {
        let client = BasicClient::new(
            ClientId::new(config.client_id.clone()),
            Some(ClientSecret::new(config.client_secret.clone())),
            AuthUrl::new(GOOGLE_AUTH_URL.to_string())
                .map_err(|e| Error::InvalidInput(format!("Invalid auth URL: {}", e)))?,
            Some(
                TokenUrl::new(GOOGLE_TOKEN_URL.to_string())
                    .map_err(|e| Error::InvalidInput(format!("Invalid token URL: {}", e)))?,
            ),
        )
        .set_redirect_uri(
            RedirectUrl::new(config.redirect_url.clone())
                .map_err(|e| Error::InvalidInput(format!("Invalid redirect URL: {}", e)))?,
        );

        Ok(Self { client, config })
    }

    /// Create with default configuration.
    pub fn with_defaults() -> Result<Self> {
        Self::new(AuthConfig::default())
    }

    /// Generate the authorization URL for the user to visit.
    ///
    /// Returns the URL and a CSRF token that should be verified on callback.
    pub fn authorization_url(&self) -> (String, String) {
        let (auth_url, csrf_token) = self
            .client
            .authorize_url(oauth2::CsrfToken::new_random)
            .add_scope(Scope::new(DRIVE_SCOPE.to_string()))
            .add_extra_param("access_type", "offline")
            .add_extra_param("prompt", "consent")
            .url();

        (auth_url.to_string(), csrf_token.secret().clone())
    }

    /// Exchange an authorization code for tokens.
    ///
    /// # Preconditions
    /// - `code` is a valid authorization code from the OAuth2 callback
    ///
    /// # Postconditions
    /// - Returns access and refresh tokens
    ///
    /// # Errors
    /// - Invalid authorization code
    /// - Network errors
    pub async fn exchange_code(&self, code: &str) -> Result<Tokens> {
        use oauth2::reqwest::async_http_client;
        use oauth2::AuthorizationCode;

        let token_result = self
            .client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .request_async(async_http_client)
            .await
            .map_err(|e| Error::Authentication(format!("Token exchange failed: {}", e)))?;

        let access_token = token_result.access_token().secret().clone();
        let refresh_token = token_result
            .refresh_token()
            .ok_or_else(|| {
                Error::Authentication("No refresh token received. Ensure 'offline' access and 'consent' prompt were requested.".to_string())
            })?
            .secret()
            .clone();

        let expires_in = token_result
            .expires_in()
            .unwrap_or_else(|| std::time::Duration::from_secs(3600));

        let expires_at = Utc::now() + Duration::from_std(expires_in).unwrap_or_else(|_| Duration::hours(1));

        Ok(Tokens {
            access_token,
            refresh_token,
            expires_at,
        })
    }

    /// Refresh an access token using the refresh token.
    ///
    /// # Preconditions
    /// - `refresh_token` is a valid refresh token
    ///
    /// # Postconditions
    /// - Returns new tokens (access token is refreshed)
    ///
    /// # Errors
    /// - Invalid or revoked refresh token
    /// - Network errors
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<Tokens> {
        use oauth2::reqwest::async_http_client;
        use oauth2::RefreshToken;

        let token_result = self
            .client
            .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
            .request_async(async_http_client)
            .await
            .map_err(|e| Error::Authentication(format!("Token refresh failed: {}", e)))?;

        let access_token = token_result.access_token().secret().clone();

        // Refresh tokens may or may not be returned in refresh response
        let new_refresh_token = token_result
            .refresh_token()
            .map(|t| t.secret().clone())
            .unwrap_or_else(|| refresh_token.to_string());

        let expires_in = token_result
            .expires_in()
            .unwrap_or_else(|| std::time::Duration::from_secs(3600));

        let expires_at = Utc::now() + Duration::from_std(expires_in).unwrap_or_else(|_| Duration::hours(1));

        Ok(Tokens {
            access_token,
            refresh_token: new_refresh_token,
            expires_at,
        })
    }

    /// Get the current configuration.
    pub fn config(&self) -> &AuthConfig {
        &self.config
    }
}

/// Token manager that automatically refreshes expired tokens.
pub struct TokenManager {
    auth_manager: AuthManager,
    tokens: tokio::sync::RwLock<Tokens>,
}

impl TokenManager {
    /// Create a new token manager with initial tokens.
    pub fn new(auth_manager: AuthManager, tokens: Tokens) -> Self {
        Self {
            auth_manager,
            tokens: tokio::sync::RwLock::new(tokens),
        }
    }

    /// Get a valid access token, refreshing if necessary.
    ///
    /// # Postconditions
    /// - Returns a valid (non-expired) access token
    ///
    /// # Errors
    /// - Token refresh failed
    pub async fn get_access_token(&self) -> Result<String> {
        let tokens = self.tokens.read().await;

        if !tokens.is_expired() {
            return Ok(tokens.access_token.clone());
        }

        drop(tokens);

        // Need to refresh
        let mut tokens = self.tokens.write().await;

        // Double-check after acquiring write lock
        if !tokens.is_expired() {
            return Ok(tokens.access_token.clone());
        }

        tracing::info!("Refreshing expired access token");

        let new_tokens = self
            .auth_manager
            .refresh_token(&tokens.refresh_token)
            .await?;

        *tokens = new_tokens;

        Ok(tokens.access_token.clone())
    }

    /// Get the current tokens.
    pub async fn get_tokens(&self) -> Tokens {
        self.tokens.read().await.clone()
    }

    /// Update tokens (e.g., after manual refresh).
    pub async fn update_tokens(&self, tokens: Tokens) {
        *self.tokens.write().await = tokens;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokens_expiration() {
        let tokens = Tokens {
            access_token: "test".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() - Duration::hours(1),
        };

        assert!(tokens.is_expired());

        let valid_tokens = Tokens {
            access_token: "test".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
        };

        assert!(!valid_tokens.is_expired());
    }

    #[test]
    fn test_tokens_near_expiration() {
        // Token expiring in 4 minutes should be considered expired (5 min buffer)
        let tokens = Tokens {
            access_token: "test".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() + Duration::minutes(4),
        };

        assert!(tokens.is_expired());
    }

    #[test]
    fn test_auth_config_serialization() {
        let config = AuthConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AuthConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.client_id, config.client_id);
        assert_eq!(deserialized.redirect_url, config.redirect_url);
    }

    #[test]
    fn test_tokens_serialization() {
        let tokens = Tokens {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now(),
        };

        let json = serde_json::to_string(&tokens).unwrap();
        let deserialized: Tokens = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.access_token, tokens.access_token);
        assert_eq!(deserialized.refresh_token, tokens.refresh_token);
    }

    #[test]
    fn test_auth_manager_creation() {
        let config = AuthConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            redirect_url: "http://localhost:8080/callback".to_string(),
        };

        let manager = AuthManager::new(config.clone()).unwrap();
        assert_eq!(manager.config().client_id, "test_id");
    }

    #[test]
    fn test_authorization_url_generation() {
        let config = AuthConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            redirect_url: "http://localhost:8080/callback".to_string(),
        };

        let manager = AuthManager::new(config).unwrap();
        let (url, csrf_token) = manager.authorization_url();

        assert!(url.contains("accounts.google.com"));
        assert!(url.contains("client_id=test_id"));
        assert!(url.contains("scope="));
        assert!(url.contains("access_type=offline"));
        assert!(!csrf_token.is_empty());
    }
}
