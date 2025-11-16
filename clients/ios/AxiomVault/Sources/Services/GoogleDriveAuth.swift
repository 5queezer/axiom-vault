import Foundation
import AuthenticationServices

/// Manages Google Drive OAuth2 authentication
class GoogleDriveAuth: NSObject, ObservableObject {
    /// Singleton instance
    static let shared = GoogleDriveAuth()

    /// OAuth2 configuration
    struct Config {
        let clientId: String
        let redirectUri: String
        let scope: String

        static let defaultConfig = Config(
            clientId: "YOUR_CLIENT_ID.apps.googleusercontent.com",
            redirectUri: "com.axiomvault.ios:/oauth2callback",
            scope: "https://www.googleapis.com/auth/drive.file"
        )
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

    private var config = Config.defaultConfig
    private var webAuthSession: ASWebAuthenticationSession?
    private var presentationContextProvider: ASWebAuthenticationPresentationContextProviding?

    private override init() {
        super.init()
        loadTokens()
    }

    /// Configure OAuth2 with custom client ID
    func configure(clientId: String, redirectUri: String? = nil) {
        self.config = Config(
            clientId: clientId,
            redirectUri: redirectUri ?? "com.axiomvault.ios:/oauth2callback",
            scope: Config.defaultConfig.scope
        )
    }

    /// Start the OAuth2 authentication flow
    func authenticate() async throws -> Tokens {
        let authURL = buildAuthorizationURL()

        return try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.main.async { [weak self] in
                guard let self = self else {
                    continuation.resume(throwing: GoogleDriveError.authenticationFailed("Self is nil"))
                    return
                }

                let session = ASWebAuthenticationSession(
                    url: authURL,
                    callbackURLScheme: self.extractScheme(from: self.config.redirectUri)
                ) { [weak self] callbackURL, error in
                    guard let self = self else {
                        continuation.resume(throwing: GoogleDriveError.authenticationFailed("Self is nil"))
                        return
                    }

                    if let error = error {
                        if let authError = error as? ASWebAuthenticationSessionError,
                           authError.code == .canceledLogin {
                            continuation.resume(throwing: GoogleDriveError.userCancelled)
                        } else {
                            continuation.resume(throwing: error)
                        }
                        return
                    }

                    guard let callbackURL = callbackURL,
                          let code = self.extractAuthorizationCode(from: callbackURL) else {
                        continuation.resume(throwing: GoogleDriveError.noAuthorizationCode)
                        return
                    }

                    // Exchange code for tokens
                    Task {
                        do {
                            let tokens = try await self.exchangeCodeForTokens(code)
                            await MainActor.run {
                                self.tokens = tokens
                                self.isAuthenticated = true
                            }
                            self.saveTokens(tokens)
                            continuation.resume(returning: tokens)
                        } catch {
                            continuation.resume(throwing: error)
                        }
                    }
                }

                session.presentationContextProvider = self
                session.prefersEphemeralWebBrowserSession = false

                self.webAuthSession = session

                if !session.start() {
                    continuation.resume(throwing: GoogleDriveError.authenticationFailed("Failed to start authentication session"))
                }
            }
        }
    }

    /// Refresh the access token
    func refreshAccessToken() async throws -> Tokens {
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

        await MainActor.run {
            self.tokens = newTokens
        }
        saveTokens(newTokens)

        return newTokens
    }

    /// Get a valid access token (refreshing if necessary)
    func getValidAccessToken() async throws -> String {
        guard var currentTokens = tokens else {
            throw GoogleDriveError.notAuthenticated
        }

        if currentTokens.isExpired {
            currentTokens = try await refreshAccessToken()
        }

        return currentTokens.accessToken
    }

    /// Sign out and clear tokens
    func signOut() {
        tokens = nil
        isAuthenticated = false
        clearTokens()
    }

    // MARK: - Private Helpers

    private func buildAuthorizationURL() -> URL {
        var components = URLComponents(string: "https://accounts.google.com/o/oauth2/v2/auth")!
        components.queryItems = [
            URLQueryItem(name: "client_id", value: config.clientId),
            URLQueryItem(name: "redirect_uri", value: config.redirectUri),
            URLQueryItem(name: "response_type", value: "code"),
            URLQueryItem(name: "scope", value: config.scope),
            URLQueryItem(name: "access_type", value: "offline"),
            URLQueryItem(name: "prompt", value: "consent"),
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

    private func exchangeCodeForTokens(_ code: String) async throws -> Tokens {
        let tokenURL = URL(string: "https://oauth2.googleapis.com/token")!
        var request = URLRequest(url: tokenURL)
        request.httpMethod = "POST"
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")

        let body = [
            "client_id": config.clientId,
            "code": code,
            "redirect_uri": config.redirectUri,
            "grant_type": "authorization_code",
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

    // MARK: - Token Persistence

    private var tokensKey: String {
        "com.axiomvault.googledrive.tokens"
    }

    private func saveTokens(_ tokens: Tokens) {
        if let data = try? JSONEncoder().encode(tokens) {
            UserDefaults.standard.set(data, forKey: tokensKey)
        }
    }

    private func loadTokens() {
        if let data = UserDefaults.standard.data(forKey: tokensKey),
           let savedTokens = try? JSONDecoder().decode(Tokens.self, from: data) {
            self.tokens = savedTokens
            self.isAuthenticated = true
        }
    }

    private func clearTokens() {
        UserDefaults.standard.removeObject(forKey: tokensKey)
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
    case authenticationFailed(String)
    case userCancelled
    case noAuthorizationCode
    case tokenExchangeFailed
    case tokenRefreshFailed
    case invalidTokenResponse
    case noRefreshToken
    case notAuthenticated

    var errorDescription: String? {
        switch self {
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
