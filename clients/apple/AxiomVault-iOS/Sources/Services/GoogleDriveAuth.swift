import Foundation
import AuthenticationServices
import Security
import CryptoKit

/// Manages Google Drive OAuth2 authentication.
///
/// Client credentials are loaded from `GoogleServices-Info.plist` (standard Google pattern).
/// To enable Google Drive sync:
///   1. Create a project in Google Cloud Console
///   2. Enable the Google Drive API
///   3. Create OAuth 2.0 credentials (iOS app type)
///   4. Download the generated `GoogleServices-Info.plist` and add it to the Xcode project
class GoogleDriveAuth: NSObject, ObservableObject {
    /// Singleton instance
    static let shared = GoogleDriveAuth()

    /// Placeholder string used when no real Client ID is configured.
    private static let placeholderClientId = "YOUR_CLIENT_ID.apps.googleusercontent.com"

    /// OAuth2 configuration
    struct Config {
        let clientId: String
        let redirectUri: String
        let scope: String
    }

    /// OAuth2 tokens
    struct Tokens: Codable {
        let accessToken: String
        let refreshToken: String?
        let expiresIn: Int
        let tokenType: String
        let createdAt: Date

        var isExpired: Bool {
            Date().timeIntervalSince(createdAt) >= TimeInterval(expiresIn - 60)
        }
    }

    @Published var isAuthenticated = false
    @Published var tokens: Tokens?
    @Published var error: Error?

    private var config: Config?
    private var webAuthSession: ASWebAuthenticationSession?

    /// PKCE code verifier for the current auth flow
    private var codeVerifier: String?
    /// State parameter for CSRF protection
    private var authState: String?

    private override init() {
        super.init()
        loadConfig()
        loadTokens()
    }

    // MARK: - Configuration

    /// Load OAuth2 configuration from GoogleServices-Info.plist.
    private func loadConfig() {
        guard let plistURL = Bundle.main.url(forResource: "GoogleServices-Info", withExtension: "plist"),
              let plist = NSDictionary(contentsOf: plistURL),
              let clientId = plist["CLIENT_ID"] as? String else {
            // GoogleServices-Info.plist not present — Google Drive sync disabled
            return
        }

        config = Config(
            clientId: clientId,
            redirectUri: "com.axiomvault.ios:/oauth2callback",
            scope: "https://www.googleapis.com/auth/drive.file"
        )
    }

    /// Validate that Google Drive sync is properly configured.
    private func validateConfig() throws {
        guard let config = config else {
            throw GoogleDriveError.notConfigured
        }
        if config.clientId == GoogleDriveAuth.placeholderClientId || config.clientId.isEmpty {
            throw GoogleDriveError.notConfigured
        }
    }

    /// Start the OAuth2 authentication flow.
    func authenticate() async throws -> Tokens {
        try validateConfig()
        guard let config = config else { throw GoogleDriveError.notConfigured }

        // Generate PKCE code verifier and challenge
        let verifier = Self.generateCodeVerifier()
        let challenge = Self.generateCodeChallenge(from: verifier)
        codeVerifier = verifier

        // Generate cryptographic random state for CSRF protection
        let state = Self.generateState()
        authState = state

        let authURL = buildAuthorizationURL(config: config, codeChallenge: challenge, state: state)

        return try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.main.async { [weak self] in
                guard let self = self else {
                    continuation.resume(throwing: GoogleDriveError.authenticationFailed("Self is nil"))
                    return
                }

                let session = ASWebAuthenticationSession(
                    url: authURL,
                    callbackURLScheme: self.extractScheme(from: config.redirectUri)
                ) { [weak self] callbackURL, error in
                    guard let self = self else {
                        continuation.resume(throwing: GoogleDriveError.authenticationFailed("Self is nil"))
                        return
                    }

                    if let error = error {
                        self.clearPKCEState()
                        if let authError = error as? ASWebAuthenticationSessionError,
                           authError.code == .canceledLogin {
                            continuation.resume(throwing: GoogleDriveError.userCancelled)
                        } else {
                            continuation.resume(throwing: error)
                        }
                        return
                    }

                    guard let callbackURL = callbackURL else {
                        self.clearPKCEState()
                        continuation.resume(throwing: GoogleDriveError.noAuthorizationCode)
                        return
                    }

                    // Verify state parameter to prevent CSRF attacks
                    let components = URLComponents(url: callbackURL, resolvingAgainstBaseURL: false)
                    let returnedState = components?.queryItems?.first { $0.name == "state" }?.value

                    guard let expectedState = self.authState, returnedState == expectedState else {
                        self.clearPKCEState()
                        continuation.resume(throwing: GoogleDriveError.stateMismatch)
                        return
                    }

                    guard let code = self.extractAuthorizationCode(from: callbackURL) else {
                        self.clearPKCEState()
                        continuation.resume(throwing: GoogleDriveError.noAuthorizationCode)
                        return
                    }

                    guard let verifier = self.codeVerifier else {
                        self.clearPKCEState()
                        continuation.resume(throwing: GoogleDriveError.authenticationFailed("Missing PKCE code verifier"))
                        return
                    }

                    Task {
                        do {
                            let tokens = try await self.exchangeCodeForTokens(code, config: config, codeVerifier: verifier)
                            self.clearPKCEState()
                            await MainActor.run {
                                self.tokens = tokens
                                self.isAuthenticated = true
                            }
                            self.saveTokens(tokens)
                            continuation.resume(returning: tokens)
                        } catch {
                            self.clearPKCEState()
                            continuation.resume(throwing: error)
                        }
                    }
                }

                session.presentationContextProvider = self
                session.prefersEphemeralWebBrowserSession = false
                self.webAuthSession = session

                if !session.start() {
                    self.clearPKCEState()
                    continuation.resume(throwing: GoogleDriveError.authenticationFailed("Failed to start authentication session"))
                }
            }
        }
    }

    /// Refresh the access token.
    func refreshAccessToken() async throws -> Tokens {
        try validateConfig()
        guard let config = config else { throw GoogleDriveError.notConfigured }

        guard let currentTokens = tokens, let refreshToken = currentTokens.refreshToken else {
            throw GoogleDriveError.noRefreshToken
        }

        let tokenURL = URL(string: "https://oauth2.googleapis.com/token")!
        var request = URLRequest(url: tokenURL)
        request.httpMethod = "POST"
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")

        let body = [
            "client_id": config.clientId,
            "refresh_token": refreshToken,
            "grant_type": "refresh_token",
        ]
        request.httpBody = body.percentEncoded()

        let (data, response) = try await URLSession.shared.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse,
              httpResponse.statusCode == 200 else {
            throw GoogleDriveError.tokenRefreshFailed
        }

        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard let accessToken = json?["access_token"] as? String,
              let expiresIn = json?["expires_in"] as? Int else {
            throw GoogleDriveError.invalidTokenResponse
        }

        let newTokens = Tokens(
            accessToken: accessToken,
            refreshToken: refreshToken,
            expiresIn: expiresIn,
            tokenType: json?["token_type"] as? String ?? "Bearer",
            createdAt: Date()
        )

        await MainActor.run { self.tokens = newTokens }
        saveTokens(newTokens)

        return newTokens
    }

    /// Get a valid access token (refreshing if necessary).
    func getValidAccessToken() async throws -> String {
        try validateConfig()

        guard var currentTokens = tokens else {
            throw GoogleDriveError.notAuthenticated
        }

        if currentTokens.isExpired {
            currentTokens = try await refreshAccessToken()
        }

        return currentTokens.accessToken
    }

    /// Whether Google Drive sync is available (plist present and not placeholder).
    var isSyncAvailable: Bool {
        guard let config = config else { return false }
        return !config.clientId.isEmpty && config.clientId != GoogleDriveAuth.placeholderClientId
    }

    /// Sign out and clear tokens.
    func signOut() {
        tokens = nil
        isAuthenticated = false
        clearTokens()
    }

    // MARK: - PKCE Helpers

    /// Generate a cryptographically random code verifier for PKCE (RFC 7636).
    private static func generateCodeVerifier() -> String {
        var bytes = [UInt8](repeating: 0, count: 32)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        return Data(bytes)
            .base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    /// Derive the S256 code challenge from a code verifier.
    private static func generateCodeChallenge(from verifier: String) -> String {
        let data = Data(verifier.utf8)
        let hash = SHA256.hash(data: data)
        return Data(hash)
            .base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    /// Generate a cryptographically random state parameter for CSRF protection.
    private static func generateState() -> String {
        var bytes = [UInt8](repeating: 0, count: 32)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        return Data(bytes)
            .base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    /// Clear PKCE and state parameters after auth flow completes.
    private func clearPKCEState() {
        codeVerifier = nil
        authState = nil
    }

    // MARK: - Private Helpers

    private func buildAuthorizationURL(config: Config, codeChallenge: String, state: String) -> URL {
        var components = URLComponents(string: "https://accounts.google.com/o/oauth2/v2/auth")!
        components.queryItems = [
            URLQueryItem(name: "client_id", value: config.clientId),
            URLQueryItem(name: "redirect_uri", value: config.redirectUri),
            URLQueryItem(name: "response_type", value: "code"),
            URLQueryItem(name: "scope", value: config.scope),
            URLQueryItem(name: "access_type", value: "offline"),
            URLQueryItem(name: "prompt", value: "consent"),
            URLQueryItem(name: "code_challenge", value: codeChallenge),
            URLQueryItem(name: "code_challenge_method", value: "S256"),
            URLQueryItem(name: "state", value: state),
        ]
        return components.url!
    }

    private func extractScheme(from redirectUri: String) -> String {
        if let url = URL(string: redirectUri) {
            return url.scheme ?? "com.axiomvault.ios"
        }
        return "com.axiomvault.ios"
    }

    private func extractAuthorizationCode(from url: URL) -> String? {
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        return components?.queryItems?.first { $0.name == "code" }?.value
    }

    private func exchangeCodeForTokens(_ code: String, config: Config, codeVerifier: String) async throws -> Tokens {
        let tokenURL = URL(string: "https://oauth2.googleapis.com/token")!
        var request = URLRequest(url: tokenURL)
        request.httpMethod = "POST"
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")

        let body = [
            "client_id": config.clientId,
            "code": code,
            "redirect_uri": config.redirectUri,
            "grant_type": "authorization_code",
            "code_verifier": codeVerifier,
        ]
        request.httpBody = body.percentEncoded()

        let (data, response) = try await URLSession.shared.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse,
              httpResponse.statusCode == 200 else {
            throw GoogleDriveError.tokenExchangeFailed
        }

        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard let accessToken = json?["access_token"] as? String,
              let expiresIn = json?["expires_in"] as? Int else {
            throw GoogleDriveError.invalidTokenResponse
        }

        return Tokens(
            accessToken: accessToken,
            refreshToken: json?["refresh_token"] as? String,
            expiresIn: expiresIn,
            tokenType: json?["token_type"] as? String ?? "Bearer",
            createdAt: Date()
        )
    }

    // MARK: - Keychain Token Persistence

    private static let keychainService = "com.axiomvault.googledrive"
    private static let keychainAccount = "oauth-tokens"

    private func saveTokens(_ tokens: Tokens) {
        guard let data = try? JSONEncoder().encode(tokens) else { return }

        // Delete any existing item first
        let deleteQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.keychainService,
            kSecAttrAccount as String: Self.keychainAccount,
        ]
        SecItemDelete(deleteQuery as CFDictionary)

        // Add the new item
        let addQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.keychainService,
            kSecAttrAccount as String: Self.keychainAccount,
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        ]
        SecItemAdd(addQuery as CFDictionary, nil)
    }

    private func loadTokens() {
        // Migrate tokens from UserDefaults to Keychain if present
        let legacyKey = "com.axiomvault.googledrive.tokens"
        if let legacyData = UserDefaults.standard.data(forKey: legacyKey),
           let savedTokens = try? JSONDecoder().decode(Tokens.self, from: legacyData) {
            saveTokens(savedTokens)
            UserDefaults.standard.removeObject(forKey: legacyKey)
            self.tokens = savedTokens
            self.isAuthenticated = true
            return
        }

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.keychainService,
            kSecAttrAccount as String: Self.keychainAccount,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]

        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)

        guard status == errSecSuccess,
              let data = result as? Data,
              let savedTokens = try? JSONDecoder().decode(Tokens.self, from: data) else {
            return
        }

        self.tokens = savedTokens
        self.isAuthenticated = true
    }

    private func clearTokens() {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.keychainService,
            kSecAttrAccount as String: Self.keychainAccount,
        ]
        SecItemDelete(query as CFDictionary)

        // Also clean up any legacy UserDefaults entry
        UserDefaults.standard.removeObject(forKey: "com.axiomvault.googledrive.tokens")
    }
}

// MARK: - ASWebAuthenticationPresentationContextProviding

extension GoogleDriveAuth: ASWebAuthenticationPresentationContextProviding {
    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap { $0.windows }
            .first { $0.isKeyWindow } ?? ASPresentationAnchor()
    }
}

// MARK: - Errors

enum GoogleDriveError: Error, LocalizedError {
    case notConfigured
    case authenticationFailed(String)
    case userCancelled
    case noAuthorizationCode
    case tokenExchangeFailed
    case tokenRefreshFailed
    case invalidTokenResponse
    case noRefreshToken
    case notAuthenticated
    case stateMismatch

    var errorDescription: String? {
        switch self {
        case .notConfigured:
            return "Google Drive sync not configured. Add GoogleServices-Info.plist with your OAuth2 credentials."
        case .authenticationFailed(let message):
            return "Authentication failed: \(message)"
        case .userCancelled:
            return "Authentication was cancelled"
        case .noAuthorizationCode:
            return "No authorization code received"
        case .tokenExchangeFailed:
            return "Failed to exchange authorization code for tokens"
        case .tokenRefreshFailed:
            return "Failed to refresh access token"
        case .invalidTokenResponse:
            return "Invalid token response from server"
        case .noRefreshToken:
            return "No refresh token available"
        case .notAuthenticated:
            return "Not authenticated with Google Drive"
        case .stateMismatch:
            return "OAuth state mismatch — possible CSRF attack"
        }
    }
}

// MARK: - Dictionary Extension

extension Dictionary where Key == String, Value == String {
    func percentEncoded() -> Data? {
        map { key, value in
            let escapedKey = key.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? ""
            let escapedValue = value.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? ""
            return "\(escapedKey)=\(escapedValue)"
        }
        .joined(separator: "&")
        .data(using: .utf8)
    }
}
