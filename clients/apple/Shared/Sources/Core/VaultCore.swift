import Foundation

/// Errors that can occur during vault operations
enum VaultError: Error, LocalizedError {
    case initializationFailed
    case creationFailed(String)
    case openFailed(String)
    case operationFailed(String)
    case invalidHandle
    case invalidPath
    case jsonParsingFailed

    var errorDescription: String? {
        switch self {
        case .initializationFailed:
            return "Failed to initialize AxiomVault"
        case .creationFailed(let msg):
            return "Failed to create vault: \(msg)"
        case .openFailed(let msg):
            return "Failed to open vault: \(msg)"
        case .operationFailed(let msg):
            return "Vault operation failed: \(msg)"
        case .invalidHandle:
            return "Invalid vault handle"
        case .invalidPath:
            return "Invalid path"
        case .jsonParsingFailed:
            return "Failed to parse JSON response"
        }
    }
}

/// Information about a vault
struct VaultInfo {
    let vaultId: String
    let rootPath: String
    let fileCount: Int
    let totalSize: Int64
    let version: Int
}

/// File or directory entry in the vault.
///
/// Aligned with Rust `DirectoryEntryDto`.
struct VaultEntry: Identifiable, Codable {
    let id: UUID
    let name: String
    let path: String
    let isDirectory: Bool
    let size: Int64?
    let modifiedAt: String?

    enum CodingKeys: String, CodingKey {
        case name
        case path
        case isDirectory = "is_directory"
        case size
        case modifiedAt = "modified_at"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.id = UUID()
        self.name = try container.decode(String.self, forKey: .name)
        self.path = try container.decodeIfPresent(String.self, forKey: .path) ?? ""
        self.isDirectory = try container.decode(Bool.self, forKey: .isDirectory)
        self.size = try container.decodeIfPresent(Int64.self, forKey: .size)
        self.modifiedAt = try container.decodeIfPresent(String.self, forKey: .modifiedAt)
    }

    init(name: String, path: String = "", isDirectory: Bool, size: Int64? = nil, modifiedAt: String? = nil) {
        self.id = UUID()
        self.name = name
        self.path = path
        self.isDirectory = isDirectory
        self.size = size
        self.modifiedAt = modifiedAt
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(name, forKey: .name)
        try container.encode(path, forKey: .path)
        try container.encode(isDirectory, forKey: .isDirectory)
        try container.encodeIfPresent(size, forKey: .size)
        try container.encodeIfPresent(modifiedAt, forKey: .modifiedAt)
    }
}

/// Migration status for a vault.
enum MigrationStatus {
    case upToDate
    case needsMigration
    case error(String)
}

/// Vault event received from the Rust core.
///
/// Decoded from JSON-encoded `AppEvent` variants.
enum VaultEvent {
    case vaultCreated
    case vaultOpened
    case vaultLocked
    case vaultClosed
    case passwordChanged
    case fileCreated(path: String)
    case fileUpdated(path: String)
    case fileDeleted(path: String)
    case directoryCreated(path: String)
    case directoryDeleted(path: String)
    case directoryListed(path: String, entries: [VaultEntry])
    case syncStarted
    case syncCompleted
    case syncFailed(error: String)
    case error(message: String)
    case unknown(String)

    init(json: String) {
        guard let data = json.data(using: .utf8),
              let dict = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            self = .unknown(json)
            return
        }

        // Serde tagged enum: {"VariantName": payload} or just "VariantName"
        if let _ = dict["VaultCreated"] { self = .vaultCreated }
        else if let _ = dict["VaultOpened"] { self = .vaultOpened }
        else if dict.keys.contains("VaultLocked") || json.contains("\"VaultLocked\"") { self = .vaultLocked }
        else if dict.keys.contains("VaultClosed") || json.contains("\"VaultClosed\"") { self = .vaultClosed }
        else if dict.keys.contains("PasswordChanged") || json.contains("\"PasswordChanged\"") { self = .passwordChanged }
        else if let payload = dict["FileCreated"] as? [String: Any],
                let path = payload["path"] as? String { self = .fileCreated(path: path) }
        else if let payload = dict["FileUpdated"] as? [String: Any],
                let path = payload["path"] as? String { self = .fileUpdated(path: path) }
        else if let payload = dict["FileDeleted"] as? [String: Any],
                let path = payload["path"] as? String { self = .fileDeleted(path: path) }
        else if let payload = dict["DirectoryCreated"] as? [String: Any],
                let path = payload["path"] as? String { self = .directoryCreated(path: path) }
        else if let payload = dict["DirectoryDeleted"] as? [String: Any],
                let path = payload["path"] as? String { self = .directoryDeleted(path: path) }
        else if dict.keys.contains("SyncStarted") { self = .syncStarted }
        else if dict.keys.contains("SyncCompleted") { self = .syncCompleted }
        else if let payload = dict["SyncFailed"] as? [String: Any],
                let err = payload["error"] as? String { self = .syncFailed(error: err) }
        else if let payload = dict["Error"] as? [String: Any],
                let msg = payload["message"] as? String { self = .error(message: msg) }
        else { self = .unknown(json) }
    }
}

/// Swift wrapper for AxiomVault Rust core (shared between iOS, macOS, and File Provider)
class VaultCore {
    static let shared = VaultCore()

    private var initialized = false
    private var handle: OpaquePointer?
    private let lock = NSLock()

    /// Registered event handler. Called on the main thread.
    var onEvent: ((VaultEvent) -> Void)?

    private init() {}

    deinit {
        closeVault()
    }

    func initialize() throws {
        lock.lock()
        defer { lock.unlock() }

        guard !initialized else { return }

        let result = axiom_init()
        guard result == 0 else {
            throw VaultError.initializationFailed
        }

        initialized = true
    }

    func version() -> String {
        guard let versionPtr = axiom_version() else {
            return "unknown"
        }
        return String(cString: versionPtr)
    }

    // MARK: - Vault lifecycle

    func createVault(at path: String, password: String) throws -> String? {
        lock.lock()
        defer { lock.unlock() }

        guard initialized else { throw VaultError.initializationFailed }

        if let existingHandle = handle {
            unsubscribeEventsLocked()
            axiom_vault_close(existingHandle)
            handle = nil
        }

        guard let newHandle = axiom_vault_create(path, password) else {
            throw VaultError.creationFailed(getLastError())
        }

        handle = newHandle
        subscribeEventsLocked()

        // Retrieve recovery words (one-time).
        var recoveryWords: String? = nil
        if let wordsPtr = axiom_vault_get_recovery_words(newHandle) {
            recoveryWords = String(cString: wordsPtr)
            axiom_string_free(wordsPtr)
        }

        return recoveryWords
    }

    func openVault(at path: String, password: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard initialized else { throw VaultError.initializationFailed }

        if let existingHandle = handle {
            unsubscribeEventsLocked()
            axiom_vault_close(existingHandle)
            handle = nil
        }

        guard let newHandle = axiom_vault_open(path, password) else {
            throw VaultError.openFailed(getLastError())
        }

        handle = newHandle
        subscribeEventsLocked()
    }

    func closeVault() {
        lock.lock()
        defer { lock.unlock() }

        if let currentHandle = handle {
            unsubscribeEventsLocked()
            _ = axiom_vault_close(currentHandle)
            handle = nil
        }
    }

    // MARK: - Vault info

    func getVaultInfo() throws -> VaultInfo {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else { throw VaultError.invalidHandle }

        guard let infoPtr = axiom_vault_info(currentHandle) else {
            throw VaultError.operationFailed(getLastError())
        }
        defer { axiom_vault_info_free(infoPtr) }

        let info = infoPtr.pointee
        return VaultInfo(
            vaultId: info.vault_id != nil ? String(cString: info.vault_id) : "",
            rootPath: info.root_path != nil ? String(cString: info.root_path) : "",
            fileCount: Int(info.file_count),
            totalSize: info.total_size,
            version: Int(info.version)
        )
    }

    // MARK: - File and directory operations

    func listDirectory(at path: String = "/") throws -> [VaultEntry] {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else { throw VaultError.invalidHandle }

        guard let jsonPtr = axiom_vault_list(currentHandle, path) else {
            throw VaultError.operationFailed(getLastError())
        }
        defer { axiom_string_free(jsonPtr) }

        let jsonString = String(cString: jsonPtr)
        guard let jsonData = jsonString.data(using: .utf8) else {
            throw VaultError.jsonParsingFailed
        }

        return try JSONDecoder().decode([VaultEntry].self, from: jsonData)
    }

    func addFile(from localPath: String, to vaultPath: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else { throw VaultError.invalidHandle }

        let result = axiom_vault_add_file(currentHandle, localPath, vaultPath)
        if result != 0 {
            throw VaultError.operationFailed(getLastError())
        }
    }

    func extractFile(from vaultPath: String, to localPath: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else { throw VaultError.invalidHandle }

        let result = axiom_vault_extract_file(currentHandle, vaultPath, localPath)
        if result != 0 {
            throw VaultError.operationFailed(getLastError())
        }
    }

    func createDirectory(at vaultPath: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else { throw VaultError.invalidHandle }

        let result = axiom_vault_mkdir(currentHandle, vaultPath)
        if result != 0 {
            throw VaultError.operationFailed(getLastError())
        }
    }

    func removeEntry(at vaultPath: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else { throw VaultError.invalidHandle }

        let result = axiom_vault_remove(currentHandle, vaultPath)
        if result != 0 {
            throw VaultError.operationFailed(getLastError())
        }
    }

    // MARK: - Password and recovery

    func changePassword(old oldPassword: String, new newPassword: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else { throw VaultError.invalidHandle }

        let result = axiom_vault_change_password(currentHandle, oldPassword, newPassword)
        if result != 0 {
            throw VaultError.operationFailed(getLastError())
        }
    }

    func showRecoveryKey() throws -> String {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else { throw VaultError.invalidHandle }

        guard let wordsPtr = axiom_vault_show_recovery_key(currentHandle) else {
            throw VaultError.operationFailed(getLastError())
        }
        defer { axiom_string_free(wordsPtr) }

        return String(cString: wordsPtr)
    }

    func resetPassword(path: String, recoveryWords: String, newPassword: String) throws {
        lock.lock()
        defer { lock.unlock() }

        if let existingHandle = handle {
            unsubscribeEventsLocked()
            axiom_vault_close(existingHandle)
            handle = nil
        }

        guard let newHandle = axiom_vault_reset_password(path, recoveryWords, newPassword) else {
            throw VaultError.operationFailed(getLastError())
        }

        handle = newHandle
        subscribeEventsLocked()
    }

    // MARK: - Health check and migration

    func checkMigration(path: String) -> MigrationStatus {
        let result = axiom_vault_check_migration(path)
        switch result {
        case 0: return .upToDate
        case 1: return .needsMigration
        default: return .error(getLastError())
        }
    }

    func runMigration(path: String, password: String) throws {
        let result = axiom_vault_migrate(path, password)
        if result != 0 {
            throw VaultError.operationFailed(getLastError())
        }
    }

    func healthCheck(path: String, password: String? = nil) throws -> String {
        guard let jsonPtr = axiom_vault_health_check(path, password) else {
            throw VaultError.operationFailed(getLastError())
        }
        defer { axiom_string_free(jsonPtr) }

        return String(cString: jsonPtr)
    }

    // MARK: - Event subscription

    /// Subscribe to events from the Rust core (must be called while lock is held).
    private func subscribeEventsLocked() {
        guard let currentHandle = handle else { return }

        let callback: FFIEventCallback = { jsonPtr in
            guard let jsonPtr = jsonPtr else { return }
            let json = String(cString: jsonPtr)
            let event = VaultEvent(json: json)
            DispatchQueue.main.async {
                VaultCore.shared.onEvent?(event)
            }
        }

        axiom_vault_subscribe_events(currentHandle, callback)
    }

    /// Unsubscribe from events (must be called while lock is held).
    private func unsubscribeEventsLocked() {
        guard let currentHandle = handle else { return }
        axiom_vault_subscribe_events(currentHandle, nil)
    }

    // MARK: - Helpers

    var isVaultOpen: Bool {
        lock.lock()
        defer { lock.unlock() }
        return handle != nil
    }

    private func getLastError() -> String {
        guard let errorPtr = axiom_last_error() else { return "Unknown error" }
        defer { axiom_string_free(errorPtr) }
        return String(cString: errorPtr)
    }
}
