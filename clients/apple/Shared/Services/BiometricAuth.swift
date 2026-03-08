import Foundation
import LocalAuthentication
import Security

/// Manages biometric authentication for vault access (Face ID / Touch ID on both iOS and macOS)
class BiometricAuth {
    static let shared = BiometricAuth()

    private static let serviceName = "com.axiomvault.vault-password"

    private init() {}

    var isBiometricAvailable: Bool {
        let context = LAContext()
        var error: NSError?
        return context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error)
    }

    var biometricType: LABiometryType {
        let context = LAContext()
        _ = context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil)
        return context.biometryType
    }

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

    var unlockButtonLabel: String {
        "Unlock with \(biometricName)"
    }

    var unlockButtonIcon: String {
        switch biometricType {
        case .faceID:
            return "faceid"
        case .touchID:
            return "touchid"
        default:
            return "lock.shield"
        }
    }

    // MARK: - Keychain Storage for Vault Passwords

    /// Account key derived from vault path for keychain storage
    private func keychainAccount(for vaultPath: String) -> String {
        vaultPath
    }

    func storePassword(_ password: String, for vaultPath: String) throws {
        let account = keychainAccount(for: vaultPath)

        // Delete any existing entry first
        let deleteQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.serviceName,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(deleteQuery as CFDictionary)

        guard let accessControl = SecAccessControlCreateWithFlags(
            nil,
            kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly,
            .biometryCurrentSet,
            nil
        ) else {
            throw BiometricError.accessControlCreationFailed
        }

        let passwordData = password.data(using: .utf8)!
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.serviceName,
            kSecAttrAccount as String: account,
            kSecValueData as String: passwordData,
            kSecAttrAccessControl as String: accessControl,
        ]

        let status = SecItemAdd(query as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw BiometricError.keychainStoreFailed(status)
        }
    }

    func retrievePassword(for vaultPath: String) async throws -> String? {
        let account = keychainAccount(for: vaultPath)

        let context = LAContext()
        context.localizedReason = "Unlock your vault"

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.serviceName,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
            kSecUseAuthenticationContext as String: context,
        ]

        return try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
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
    }

    func hasStoredPassword(for vaultPath: String) -> Bool {
        let account = keychainAccount(for: vaultPath)

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.serviceName,
            kSecAttrAccount as String: account,
            kSecReturnAttributes as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
            kSecUseAuthenticationUI as String: kSecUseAuthenticationUIFail,
        ]

        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        return status == errSecSuccess || status == errSecInteractionNotAllowed
    }

    func removePassword(for vaultPath: String) throws {
        let account = keychainAccount(for: vaultPath)

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.serviceName,
            kSecAttrAccount as String: account,
        ]

        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw BiometricError.keychainDeleteFailed(status)
        }
    }
}

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
