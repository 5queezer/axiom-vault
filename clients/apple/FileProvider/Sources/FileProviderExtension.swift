import FileProvider
import os.log

class FileProviderExtension: NSObject, NSFileProviderReplicatedExtension {
    private let logger = Logger(subsystem: "com.axiomvault.macos.fileprovider", category: "extension")
    private let cacheManager = CacheManager.shared
    let domain: NSFileProviderDomain

    required init(domain: NSFileProviderDomain) {
        self.domain = domain
        super.init()

        do {
            try VaultCore.shared.initialize()
        } catch {
            logger.error("Failed to initialize VaultCore: \(error.localizedDescription)")
        }
    }

    func invalidate() {
        if cacheManager.clearOnLock {
            cacheManager.clearAll(forVault: currentVaultId)
        }
        VaultCore.shared.closeVault()
    }

    /// The vault ID for the currently open vault, used as cache partition key.
    private var currentVaultId: String {
        (try? VaultCore.shared.getVaultInfo().vaultId) ?? "default"
    }

    // MARK: - Item lookup

    func item(
        for identifier: NSFileProviderItemIdentifier,
        request: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)

        guard VaultCore.shared.isVaultOpen else {
            completionHandler(nil, NSFileProviderError(.notAuthenticated))
            return progress
        }

        if identifier == .rootContainer {
            completionHandler(FileProviderItem.root, nil)
            progress.completedUnitCount = 1
            return progress
        }

        let vaultPath = FileProviderItem.vaultPath(from: identifier)
        do {
            let parentPath = (vaultPath as NSString).deletingLastPathComponent
            let name = (vaultPath as NSString).lastPathComponent
            let entries = try VaultCore.shared.listDirectory(at: parentPath.isEmpty ? "/" : parentPath)

            if let entry = entries.first(where: { $0.name == name }) {
                let item = FileProviderItem(
                    entry: entry,
                    parentIdentifier: FileProviderItem.identifier(for: parentPath.isEmpty ? "/" : parentPath),
                    vaultPath: vaultPath
                )
                completionHandler(item, nil)
            } else {
                completionHandler(nil, NSFileProviderError(.noSuchItem))
            }
        } catch {
            logger.error("item(for:) failed: \(error.localizedDescription)")
            completionHandler(nil, error)
        }

        progress.completedUnitCount = 1
        return progress
    }

    // MARK: - Enumeration

    func enumerator(
        for containerItemIdentifier: NSFileProviderItemIdentifier,
        request: NSFileProviderRequest
    ) throws -> NSFileProviderEnumerator {
        guard VaultCore.shared.isVaultOpen else {
            throw NSFileProviderError(.notAuthenticated)
        }

        let vaultPath: String
        if containerItemIdentifier == .rootContainer {
            vaultPath = "/"
        } else {
            vaultPath = FileProviderItem.vaultPath(from: containerItemIdentifier)
        }

        return FileProviderEnumerator(vaultPath: vaultPath, parentIdentifier: containerItemIdentifier)
    }

    // MARK: - Content fetch (download)

    func fetchContents(
        for itemIdentifier: NSFileProviderItemIdentifier,
        version requestedVersion: NSFileProviderItemVersion?,
        request: NSFileProviderRequest,
        completionHandler: @escaping (URL?, NSFileProviderItem?, Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)

        let vaultPath = FileProviderItem.vaultPath(from: itemIdentifier)
        let vaultId = currentVaultId

        // Build the item metadata (needed for both cache hit and miss paths)
        func buildItem() -> FileProviderItem? {
            let parentPath = (vaultPath as NSString).deletingLastPathComponent
            let name = (vaultPath as NSString).lastPathComponent
            guard let entries = try? VaultCore.shared.listDirectory(at: parentPath.isEmpty ? "/" : parentPath),
                  let entry = entries.first(where: { $0.name == name })
            else { return nil }
            return FileProviderItem(
                entry: entry,
                parentIdentifier: FileProviderItem.identifier(for: parentPath.isEmpty ? "/" : parentPath),
                vaultPath: vaultPath
            )
        }

        // Check cache first
        if let cachedURL = cacheManager.cachedURL(forVault: vaultId, path: vaultPath) {
            logger.info("Serving \(vaultPath) from cache")
            completionHandler(cachedURL, buildItem(), nil)
            progress.completedUnitCount = 1
            return progress
        }

        // Cache miss: extract from vault
        let tempDir = FileManager.default.temporaryDirectory
        let tempFile = tempDir.appendingPathComponent(UUID().uuidString)

        do {
            try VaultCore.shared.extractFile(from: vaultPath, to: tempFile.path)

            // Store in cache for future requests
            cacheManager.cache(from: tempFile, forVault: vaultId, path: vaultPath)

            completionHandler(tempFile, buildItem(), nil)
        } catch {
            logger.error("fetchContents failed: \(error.localizedDescription)")
            completionHandler(nil, nil, error)
        }

        progress.completedUnitCount = 1
        return progress
    }

    // MARK: - Content push (upload)

    func createItem(
        basedOn itemTemplate: NSFileProviderItem,
        fields: NSFileProviderItemFields,
        contents url: URL?,
        options: NSFileProviderCreateItemOptions = [],
        request: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)

        let parentPath = FileProviderItem.vaultPath(from: itemTemplate.parentItemIdentifier)
        let vaultPath = parentPath == "/"
            ? "/\(itemTemplate.filename)"
            : "\(parentPath)/\(itemTemplate.filename)"

        do {
            if itemTemplate.contentType == .folder {
                try VaultCore.shared.createDirectory(at: vaultPath)
            } else if let fileURL = url {
                try VaultCore.shared.addFile(from: fileURL.path, to: vaultPath)
            }

            let entry = VaultEntry(
                name: itemTemplate.filename,
                isDirectory: itemTemplate.contentType == .folder,
                size: itemTemplate.documentSize as? Int64
            )
            let item = FileProviderItem(
                entry: entry,
                parentIdentifier: itemTemplate.parentItemIdentifier,
                vaultPath: vaultPath
            )
            completionHandler(item, [], false, nil)
        } catch {
            logger.error("createItem failed: \(error.localizedDescription)")
            completionHandler(nil, [], false, error)
        }

        progress.completedUnitCount = 1
        return progress
    }

    func modifyItem(
        _ item: NSFileProviderItem,
        baseVersion version: NSFileProviderItemVersion,
        changedFields: NSFileProviderItemFields,
        contents newContents: URL?,
        options: NSFileProviderModifyItemOptions = [],
        request: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)

        let vaultPath = FileProviderItem.vaultPath(from: item.itemIdentifier)

        do {
            if changedFields.contains(.contents), let fileURL = newContents {
                // Remove old, add new
                try VaultCore.shared.removeEntry(at: vaultPath)
                try VaultCore.shared.addFile(from: fileURL.path, to: vaultPath)
                // Invalidate stale cache entry
                cacheManager.invalidate(forVault: currentVaultId, path: vaultPath)
            }

            let entry = VaultEntry(
                name: item.filename,
                isDirectory: item.contentType == .folder,
                size: item.documentSize as? Int64
            )
            let updatedItem = FileProviderItem(
                entry: entry,
                parentIdentifier: item.parentItemIdentifier,
                vaultPath: vaultPath
            )
            completionHandler(updatedItem, [], false, nil)
        } catch {
            logger.error("modifyItem failed: \(error.localizedDescription)")
            completionHandler(nil, [], false, error)
        }

        progress.completedUnitCount = 1
        return progress
    }

    func deleteItem(
        identifier: NSFileProviderItemIdentifier,
        baseVersion version: NSFileProviderItemVersion,
        options: NSFileProviderDeleteItemOptions = [],
        request: NSFileProviderRequest,
        completionHandler: @escaping (Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)

        let vaultPath = FileProviderItem.vaultPath(from: identifier)

        do {
            try VaultCore.shared.removeEntry(at: vaultPath)
            cacheManager.invalidate(forVault: currentVaultId, path: vaultPath)
            completionHandler(nil)
        } catch {
            logger.error("deleteItem failed: \(error.localizedDescription)")
            completionHandler(error)
        }

        progress.completedUnitCount = 1
        return progress
    }
}
