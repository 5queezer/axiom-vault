import AuthenticationServices
import Foundation

/// Sign in with Apple — required by App Store Review Guidelines §4.8 when
/// any third-party (e.g. Google) sign-in is offered.
///
/// This implementation provides the architectural stub using Apple's
/// `AuthenticationServices` framework (ASAuthorizationAppleIDProvider).
/// A full production implementation would persist the user identifier,
/// validate server-side tokens, and handle credential state changes.
class AppleSignIn: NSObject, ObservableObject {
    /// Singleton instance.
    static let shared = AppleSignIn()

    /// Published authentication state.
    @Published var isAuthenticated = false
    @Published var userIdentifier: String?
    @Published var fullName: PersonNameComponents?
    @Published var email: String?
    @Published var error: Error?

    private var authorizationController: ASAuthorizationController?

    private override init() {
        super.init()
        checkCredentialState()
    }

    // MARK: - Public API

    /// Initiate the Sign in with Apple flow.
    func signIn() async throws -> AppleSignInResult {
        let provider = ASAuthorizationAppleIDProvider()
        let request = provider.createRequest()
        request.requestedScopes = [.fullName, .email]

        return try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.main.async { [weak self] in
                guard let self = self else {
                    continuation.resume(throwing: AppleSignInError.unknown)
                    return
                }

                let controller = ASAuthorizationController(authorizationRequests: [request])
                controller.delegate = self
                controller.presentationContextProvider = self

                // Store reference to avoid deallocation before delegate fires
                self.authorizationController = controller

                // Use a completion bridge via NotificationCenter for this stub
                // In production, use a proper delegate pattern or Combine
                NotificationCenter.default.addObserver(
                    forName: .appleSignInDidComplete,
                    object: nil,
                    queue: .main
                ) { notification in
                    NotificationCenter.default.removeObserver(self, name: .appleSignInDidComplete, object: nil)
                    if let result = notification.object as? AppleSignInResult {
                        continuation.resume(returning: result)
                    } else if let error = notification.userInfo?["error"] as? Error {
                        continuation.resume(throwing: error)
                    } else {
                        continuation.resume(throwing: AppleSignInError.unknown)
                    }
                }

                controller.performRequests()
            }
        }
    }

    /// Sign out and clear stored credentials.
    func signOut() {
        userIdentifier = nil
        fullName = nil
        email = nil
        isAuthenticated = false
        UserDefaults.standard.removeObject(forKey: userIdentifierKey)
    }

    // MARK: - Credential State

    /// Check if the current Apple ID credential is still valid.
    private func checkCredentialState() {
        guard let userId = UserDefaults.standard.string(forKey: userIdentifierKey) else { return }

        let provider = ASAuthorizationAppleIDProvider()
        provider.getCredentialState(forUserID: userId) { [weak self] state, _ in
            DispatchQueue.main.async {
                switch state {
                case .authorized:
                    self?.isAuthenticated = true
                    self?.userIdentifier = userId
                case .revoked, .notFound, .transferred:
                    self?.signOut()
                @unknown default:
                    break
                }
            }
        }
    }

    // MARK: - Private

    private let userIdentifierKey = "com.axiomvault.apple.userIdentifier"
}

// MARK: - ASAuthorizationControllerDelegate

extension AppleSignIn: ASAuthorizationControllerDelegate {
    func authorizationController(
        controller: ASAuthorizationController,
        didCompleteWithAuthorization authorization: ASAuthorization
    ) {
        guard let credential = authorization.credential as? ASAuthorizationAppleIDCredential else {
            NotificationCenter.default.post(
                name: .appleSignInDidComplete,
                object: nil,
                userInfo: ["error": AppleSignInError.invalidCredential]
            )
            return
        }

        let result = AppleSignInResult(
            userIdentifier: credential.user,
            fullName: credential.fullName,
            email: credential.email,
            identityToken: credential.identityToken.flatMap { String(data: $0, encoding: .utf8) },
            authorizationCode: credential.authorizationCode.flatMap { String(data: $0, encoding: .utf8) }
        )

        // Persist the user identifier for credential state checks on next launch
        UserDefaults.standard.set(credential.user, forKey: userIdentifierKey)

        DispatchQueue.main.async { [weak self] in
            self?.isAuthenticated = true
            self?.userIdentifier = credential.user
            self?.fullName = credential.fullName
            self?.email = credential.email
        }

        NotificationCenter.default.post(name: .appleSignInDidComplete, object: result)
    }

    func authorizationController(
        controller: ASAuthorizationController,
        didCompleteWithError error: Error
    ) {
        let mappedError: AppleSignInError
        if let authError = error as? ASAuthorizationError {
            switch authError.code {
            case .canceled:
                mappedError = .userCancelled
            case .failed:
                mappedError = .authorizationFailed(authError.localizedDescription)
            case .invalidResponse:
                mappedError = .invalidCredential
            case .notHandled:
                mappedError = .authorizationFailed("Request not handled")
            case .unknown:
                mappedError = .unknown
            case .notInteractive:
                mappedError = .authorizationFailed("Not interactive")
            @unknown default:
                mappedError = .unknown
            }
        } else {
            mappedError = .authorizationFailed(error.localizedDescription)
        }

        DispatchQueue.main.async { [weak self] in
            self?.error = mappedError
        }

        NotificationCenter.default.post(
            name: .appleSignInDidComplete,
            object: nil,
            userInfo: ["error": mappedError]
        )
    }
}

// MARK: - ASAuthorizationControllerPresentationContextProviding

extension AppleSignIn: ASAuthorizationControllerPresentationContextProviding {
    func presentationAnchor(for controller: ASAuthorizationController) -> ASPresentationAnchor {
        UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap { $0.windows }
            .first { $0.isKeyWindow } ?? ASPresentationAnchor()
    }
}

// MARK: - Supporting Types

/// Result of a successful Sign in with Apple flow.
struct AppleSignInResult {
    let userIdentifier: String
    let fullName: PersonNameComponents?
    let email: String?
    /// JWT identity token — validate server-side before trusting.
    let identityToken: String?
    /// Single-use authorization code for server-side token exchange.
    let authorizationCode: String?
}

/// Errors that can occur during Sign in with Apple.
enum AppleSignInError: Error, LocalizedError {
    case userCancelled
    case invalidCredential
    case authorizationFailed(String)
    case unknown

    var errorDescription: String? {
        switch self {
        case .userCancelled:
            return "Sign in with Apple was cancelled"
        case .invalidCredential:
            return "Invalid Apple ID credential received"
        case .authorizationFailed(let message):
            return "Sign in with Apple failed: \(message)"
        case .unknown:
            return "An unknown error occurred during Sign in with Apple"
        }
    }
}

// MARK: - Notification

private extension Notification.Name {
    static let appleSignInDidComplete = Notification.Name("AppleSignInDidComplete")
}
