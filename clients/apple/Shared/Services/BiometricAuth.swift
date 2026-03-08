import Foundation
import LocalAuthentication
import Security

/// Manages biometric authentication for vault access (Face ID / Touch ID on both iOS and macOS)
class BiometricAuth {
    static let shared = BiometricAuth()

    private let context = LAContext()

    private init() {}

    var isBiometricAvailable: Bool {
        var error: NSError?
        return context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error)
    }

    var biometricType: LABiometryType {
        _ = isBiometricAvailable
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

    func storePassword(_ password: String, for vaultPath: String) throws {
        let key = keychainKey(for: vaultPath)

        let deleteQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrAccount as String: key,
        ]
        SecItemDelete(deleteQuery as CFDictionary)

        guard let accessControl = SecAccessControlCreateWithFlags(
            nil,
            kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly,
            .biometryAny,
            nil
        ) else {
            throw BiometricError.accessControlCreationFailed
        }

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
