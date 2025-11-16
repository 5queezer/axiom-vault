import Foundation
import LocalAuthentication
import Security

/// Manages biometric authentication for vault access
class BiometricAuth {
    /// Singleton instance
    static let shared = BiometricAuth()

    private let context = LAContext()

    private init() {}

    /// Check if biometric authentication is available
    var isBiometricAvailable: Bool {
        var error: NSError?
        return context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error)
    }

    /// Get the type of biometric available
    var biometricType: LABiometryType {
        _ = isBiometricAvailable
        return context.biometryType
    }

    /// Get a user-friendly name for the biometric type
    var biometricName: String {
        switch biometricType {
        case .faceID:
            return "Face ID"
        case .touchID:
            return "Touch ID"
        case .opticID:
            return "Optic ID"
        default:
            return "Biometric"
        }
    }

    /// Authenticate using biometrics
    func authenticate(reason: String = "Unlock your vault") async throws -> Bool {
        let context = LAContext()
        context.localizedFallbackTitle = "Use Password"

        return try await withCheckedThrowingContinuation { continuation in
            context.evaluatePolicy(
                .deviceOwnerAuthenticationWithBiometrics,
                localizedReason: reason
            ) { success, error in
                if success {
                    continuation.resume(returning: true)
                } else if let error = error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume(returning: false)
                }
            }
        }
    }

    // MARK: - Keychain Storage for Vault Passwords

    private func keychainKey(for vaultPath: String) -> String {
        "com.axiomvault.password.\(vaultPath.hashValue)"
    }

    /// Store vault password in Keychain protected by biometrics
    func storePassword(_ password: String, for vaultPath: String) throws {
        let key = keychainKey(for: vaultPath)

        // Delete existing password if any
        let deleteQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrAccount as String: key,
        ]
        SecItemDelete(deleteQuery as CFDictionary)

        // Create access control with biometric protection
        guard let accessControl = SecAccessControlCreateWithFlags(
            nil,
            kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly,
            .biometryAny,
            nil
        ) else {
            throw BiometricError.accessControlCreationFailed
        }

        // Store the password
        let passwordData = password.data(using: .utf8)!
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrAccount as String: key,
            kSecValueData as String: passwordData,
            kSecAttrAccessControl as String: accessControl,
        ]

        let status = SecItemAdd(query as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw BiometricError.keychainStoreFailed(status)
        }
    }

    /// Retrieve vault password from Keychain using biometrics
    func retrievePassword(for vaultPath: String) async throws -> String? {
        let key = keychainKey(for: vaultPath)

        let context = LAContext()
        context.localizedReason = "Retrieve your vault password"

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrAccount as String: key,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
            kSecUseAuthenticationContext as String: context,
        ]

        return try await withCheckedThrowingContinuation { continuation in
            var result: AnyObject?
            let status = SecItemCopyMatching(query as CFDictionary, &result)

            if status == errSecSuccess, let data = result as? Data {
                let password = String(data: data, encoding: .utf8)
                continuation.resume(returning: password)
            } else if status == errSecItemNotFound {
                continuation.resume(returning: nil)
            } else {
                continuation.resume(throwing: BiometricError.keychainRetrieveFailed(status))
            }
        }
    }

    /// Check if password is stored for a vault
    func hasStoredPassword(for vaultPath: String) -> Bool {
        let key = keychainKey(for: vaultPath)

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrAccount as String: key,
            kSecReturnAttributes as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
            kSecUseAuthenticationUIAllow as String: kSecUseAuthenticationUIFail,
        ]

        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        return status == errSecSuccess || status == errSecInteractionNotAllowed
    }

    /// Remove stored password for a vault
    func removePassword(for vaultPath: String) throws {
        let key = keychainKey(for: vaultPath)

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrAccount as String: key,
        ]

        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw BiometricError.keychainDeleteFailed(status)
        }
    }
}

/// Errors related to biometric authentication
enum BiometricError: Error, LocalizedError {
    case accessControlCreationFailed
    case keychainStoreFailed(OSStatus)
    case keychainRetrieveFailed(OSStatus)
    case keychainDeleteFailed(OSStatus)
    case notAvailable

    var errorDescription: String? {
        switch self {
        case .accessControlCreationFailed:
            return "Failed to create access control for biometric protection"
        case .keychainStoreFailed(let status):
            return "Failed to store password in Keychain (status: \(status))"
        case .keychainRetrieveFailed(let status):
            return "Failed to retrieve password from Keychain (status: \(status))"
        case .keychainDeleteFailed(let status):
            return "Failed to delete password from Keychain (status: \(status))"
        case .notAvailable:
            return "Biometric authentication is not available"
        }
    }
}
