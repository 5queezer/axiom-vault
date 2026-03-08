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

/// Swift wrapper for AxiomVault Rust core (shared between iOS, macOS, and File Provider)
class VaultCore {
    static let shared = VaultCore()

    private var initialized = false
    private var handle: OpaquePointer?
    private let lock = NSLock()

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

    func createVault(at path: String, password: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard initialized else { throw VaultError.initializationFailed }

        if let existingHandle = handle {
            axiom_vault_close(existingHandle)
            handle = nil
        }

        guard let newHandle = axiom_vault_create(path, password) else {
            throw VaultError.creationFailed(getLastError())
        }

        handle = newHandle
    }

    func openVault(at path: String, password: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard initialized else { throw VaultError.initializationFailed }

        if let existingHandle = handle {
            axiom_vault_close(existingHandle)
            handle = nil
        }

        guard let newHandle = axiom_vault_open(path, password) else {
            throw VaultError.openFailed(getLastError())
        }

        handle = newHandle
    }

    func closeVault() {
        lock.lock()
        defer { lock.unlock() }

        if let currentHandle = handle {
            _ = axiom_vault_close(currentHandle)
            handle = nil
        }
    }

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

    func changePassword(old oldPassword: String, new newPassword: String) throws {
        lock.lock()
        defer { lock.unlock() }

        guard let currentHandle = handle else { throw VaultError.invalidHandle }

        let result = axiom_vault_change_password(currentHandle, oldPassword, newPassword)
        if result != 0 {
            throw VaultError.operationFailed(getLastError())
        }
    }

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
