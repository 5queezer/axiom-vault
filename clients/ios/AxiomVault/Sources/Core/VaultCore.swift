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

/// File or directory entry in the vault
struct VaultEntry: Identifiable, Codable {
    let id: UUID
    let name: String
    let isDirectory: Bool
    let size: Int64?

    enum CodingKeys: String, CodingKey {
        case name
        case isDirectory = "is_directory"
        case size
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.id = UUID()
        self.name = try container.decode(String.self, forKey: .name)
        self.isDirectory = try container.decode(Bool.self, forKey: .isDirectory)
        self.size = try container.decodeIfPresent(Int64.self, forKey: .size)
    }

    init(name: String, isDirectory: Bool, size: Int64?) {
        self.id = UUID()
        self.name = name
        self.isDirectory = isDirectory
        self.size = size
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(name, forKey: .name)
        try container.encode(isDirectory, forKey: .isDirectory)
        try container.encodeIfPresent(size, forKey: .size)
    }
}

/// Swift wrapper for AxiomVault Rust core
class VaultCore {
    /// Singleton instance
    static let shared = VaultCore()

    /// Whether the core has been initialized
    private var initialized = false

    /// Current vault handle
    private var handle: OpaquePointer?

    /// Lock for thread safety
    private let lock = NSLock()

    private init() {}

    deinit {
        closeVault()
    }

    /// Initialize the FFI layer
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

    /// Get the library version
    func version() -> String {
        guard let versionPtr = axiom_version() else {
            return "unknown"
        }
        return String(cString: versionPtr)
    }

    /// Create a new vault
    func createVault(at path: String, password: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard initialized else {
            throw VaultError.initializationFailed
        }

        // Close existing vault if open
        if let existingHandle = handle {
            axiom_vault_close(existingHandle)
            handle = nil
        }

        let pathCStr = path.cString(using: .utf8)!
        let passwordCStr = password.cString(using: .utf8)!

        guard let newHandle = axiom_vault_create(pathCStr, passwordCStr) else {
            let error = getLastError()
            throw VaultError.creationFailed(error)
        }

        handle = newHandle
    }

    /// Open an existing vault
    func openVault(at path: String, password: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard initialized else {
            throw VaultError.initializationFailed
        }

        // Close existing vault if open
        if let existingHandle = handle {
            axiom_vault_close(existingHandle)
            handle = nil
        }

        let pathCStr = path.cString(using: .utf8)!
        let passwordCStr = password.cString(using: .utf8)!

        guard let newHandle = axiom_vault_open(pathCStr, passwordCStr) else {
            let error = getLastError()
            throw VaultError.openFailed(error)
        }

        handle = newHandle
    }

    /// Close the current vault
    func closeVault() {
        lock.lock()
        defer { lock.unlock() }

        if let currentHandle = handle {
            _ = axiom_vault_close(currentHandle)
            handle = nil
        }
    }

    /// Get vault information
    func getVaultInfo() throws -> VaultInfo {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else {
            throw VaultError.invalidHandle
        }

        guard let infoPtr = axiom_vault_info(currentHandle) else {
            let error = getLastError()
            throw VaultError.operationFailed(error)
        }

        defer {
            axiom_vault_info_free(infoPtr)
        }

        let info = infoPtr.pointee

        let vaultId = info.vault_id != nil ? String(cString: info.vault_id) : ""
        let rootPath = info.root_path != nil ? String(cString: info.root_path) : ""

        return VaultInfo(
            vaultId: vaultId,
            rootPath: rootPath,
            fileCount: Int(info.file_count),
            totalSize: info.total_size,
            version: Int(info.version)
        )
    }

    /// List contents of a directory in the vault
    func listDirectory(at path: String = "/") throws -> [VaultEntry] {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else {
            throw VaultError.invalidHandle
        }

        let pathCStr = path.cString(using: .utf8)!

        guard let jsonPtr = axiom_vault_list(currentHandle, pathCStr) else {
            let error = getLastError()
            throw VaultError.operationFailed(error)
        }

        defer {
            axiom_string_free(jsonPtr)
        }

        let jsonString = String(cString: jsonPtr)
        guard let jsonData = jsonString.data(using: .utf8) else {
            throw VaultError.jsonParsingFailed
        }

        let decoder = JSONDecoder()
        return try decoder.decode([VaultEntry].self, from: jsonData)
    }

    /// Add a file to the vault
    func addFile(from localPath: String, to vaultPath: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else {
            throw VaultError.invalidHandle
        }

        let localCStr = localPath.cString(using: .utf8)!
        let vaultCStr = vaultPath.cString(using: .utf8)!

        let result = axiom_vault_add_file(currentHandle, localCStr, vaultCStr)
        if result != 0 {
            let error = getLastError()
            throw VaultError.operationFailed(error)
        }
    }

    /// Extract a file from the vault
    func extractFile(from vaultPath: String, to localPath: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else {
            throw VaultError.invalidHandle
        }

        let vaultCStr = vaultPath.cString(using: .utf8)!
        let localCStr = localPath.cString(using: .utf8)!

        let result = axiom_vault_extract_file(currentHandle, vaultCStr, localCStr)
        if result != 0 {
            let error = getLastError()
            throw VaultError.operationFailed(error)
        }
    }

    /// Create a directory in the vault
    func createDirectory(at vaultPath: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else {
            throw VaultError.invalidHandle
        }

        let pathCStr = vaultPath.cString(using: .utf8)!

        let result = axiom_vault_mkdir(currentHandle, pathCStr)
        if result != 0 {
            let error = getLastError()
            throw VaultError.operationFailed(error)
        }
    }

    /// Remove a file or directory from the vault
    func removeEntry(at vaultPath: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else {
            throw VaultError.invalidHandle
        }

        let pathCStr = vaultPath.cString(using: .utf8)!

        let result = axiom_vault_remove(currentHandle, pathCStr)
        if result != 0 {
            let error = getLastError()
            throw VaultError.operationFailed(error)
        }
    }

    /// Change the vault password
    func changePassword(old oldPassword: String, new newPassword: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else {
            throw VaultError.invalidHandle
        }

        let oldCStr = oldPassword.cString(using: .utf8)!
        let newCStr = newPassword.cString(using: .utf8)!

        let result = axiom_vault_change_password(currentHandle, oldCStr, newCStr)
        if result != 0 {
            let error = getLastError()
            throw VaultError.operationFailed(error)
        }
    }

    /// Check if a vault is currently open
    var isVaultOpen: Bool {
        lock.lock()
        defer { lock.unlock() }
        return handle != nil
    }

    // MARK: - Private helpers

    private func getLastError() -> String {
        guard let errorPtr = axiom_last_error() else {
            return "Unknown error"
        }
        defer {
            axiom_string_free(errorPtr)
        }
        return String(cString: errorPtr)
    }
}
