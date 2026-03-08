import CryptoKit
import Foundation
import os.log

// SECURITY NOTE: This cache stores decrypted vault contents on disk for performance.
// On iOS, files are protected with FileProtection.complete so they are inaccessible
// when the device is locked. The `clearOnLock` setting (default: true) provides an
// additional layer by deleting cached files when the vault is locked. Users who
// disable clearOnLock accept the tradeoff of cached plaintext persisting across
// lock/unlock cycles in exchange for faster file access.

/// Manages a file-based cache of decrypted vault contents for the File Provider extension.
/// Uses a JSON manifest to track cached files and implements LRU eviction.
class CacheManager {
    static let shared = CacheManager()

    private let logger = Logger(subsystem: "com.axiomvault.macos.fileprovider", category: "cache")
    private let fileManager = FileManager.default
    private let queue = DispatchQueue(label: "com.axiomvault.cache", attributes: .concurrent)

    /// Default maximum cache size: 500 MB
    private static let defaultMaxCacheSize: Int64 = 500 * 1024 * 1024

    /// UserDefaults key for configurable max cache size
    private static let maxCacheSizeKey = "com.axiomvault.cache.maxSize"

    /// UserDefaults key for clear-on-lock preference
    private static let clearOnLockKey = "com.axiomvault.cache.clearOnLock"

    /// Maximum allowed cache size in bytes (configurable)
    var maxCacheSize: Int64 {
        get {
            let stored = UserDefaults.standard.object(forKey: Self.maxCacheSizeKey) as? Int64
            return stored ?? Self.defaultMaxCacheSize
        }
        set {
            UserDefaults.standard.set(newValue, forKey: Self.maxCacheSizeKey)
        }
    }

    /// Whether to clear the cache when the vault is locked.
    /// Defaults to `true` for security — cached files are decrypted plaintext.
    var clearOnLock: Bool {
        get { UserDefaults.standard.bool(forKey: Self.clearOnLockKey) }
        set { UserDefaults.standard.set(newValue, forKey: Self.clearOnLockKey) }
    }

    /// In-memory manifest cache, keyed by vault ID.
    /// Avoids re-reading JSON from disk on every operation.
    private var manifestCache: [String: CacheManifest] = [:]

    /// Tracks whether the in-memory manifest has unsaved changes, keyed by vault ID.
    private var manifestDirty: Set<String> = []

    private init() {
        // Register default so clearOnLock is true even if the user never set it.
        UserDefaults.standard.register(defaults: [Self.clearOnLockKey: true])
    }

    // MARK: - Cache directory

    /// Root cache directory for all vaults
    private var cacheRootDirectory: URL {
        let caches = fileManager.urls(for: .cachesDirectory, in: .userDomainMask).first!
        return caches.appendingPathComponent("com.axiomvault.filecache", isDirectory: true)
    }

    /// Cache directory for a specific vault
    private func cacheDirectory(forVault vaultId: String) -> URL {
        cacheRootDirectory.appendingPathComponent(vaultId, isDirectory: true)
    }

    /// Path to the JSON manifest for a specific vault
    private func manifestURL(forVault vaultId: String) -> URL {
        cacheDirectory(forVault: vaultId).appendingPathComponent("manifest.json")
    }

    // MARK: - Manifest management

    /// Metadata for a single cached file
    struct CacheEntry: Codable {
        let vaultPath: String
        let localFilename: String
        let cachedDate: Date
        let size: Int64
        let etag: String?
        var lastAccessDate: Date
    }

    /// The full cache manifest for a vault
    struct CacheManifest: Codable {
        var entries: [String: CacheEntry]  // keyed by vault path

        static var empty: CacheManifest { CacheManifest(entries: [:]) }
    }

    /// Loads the manifest from the in-memory cache, falling back to disk.
    /// Must be called within the queue.
    private func loadManifest(forVault vaultId: String) -> CacheManifest {
        if let cached = manifestCache[vaultId] {
            return cached
        }

        let url = manifestURL(forVault: vaultId)
        guard let data = try? Data(contentsOf: url),
              let manifest = try? JSONDecoder().decode(CacheManifest.self, from: data)
        else {
            let empty = CacheManifest.empty
            manifestCache[vaultId] = empty
            return empty
        }
        manifestCache[vaultId] = manifest
        return manifest
    }

    /// Stores the manifest in-memory and marks it dirty.
    /// Must be called within the queue (barrier).
    private func storeManifest(_ manifest: CacheManifest, forVault vaultId: String) {
        manifestCache[vaultId] = manifest
        manifestDirty.insert(vaultId)
    }

    /// Flushes a dirty manifest to disk.
    /// Must be called within the queue (barrier).
    private func flushManifest(forVault vaultId: String) {
        guard manifestDirty.contains(vaultId),
              let manifest = manifestCache[vaultId]
        else { return }

        let url = manifestURL(forVault: vaultId)
        let dir = url.deletingLastPathComponent()
        try? fileManager.createDirectory(at: dir, withIntermediateDirectories: true)

        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]

        if let data = try? encoder.encode(manifest) {
            try? data.write(to: url, options: .atomic)
        }

        manifestDirty.remove(vaultId)
    }

    /// Flushes all dirty manifests to disk.
    func flushAllManifests() {
        queue.async(flags: .barrier) { [self] in
            for vaultId in manifestDirty {
                flushManifest(forVault: vaultId)
            }
        }
    }

    // MARK: - File protection

    /// Sets appropriate file protection on cached files (iOS only).
    private func applyFileProtection(to url: URL) {
        #if os(iOS)
        try? fileManager.setAttributes(
            [.protectionKey: FileProtectionType.complete],
            ofItemAtPath: url.path
        )
        #endif
    }

    // MARK: - Public API

    /// Returns the local cached URL for a vault path, or nil if not cached.
    /// Updates the last-access date for LRU tracking.
    func cachedURL(forVault vaultId: String, path vaultPath: String) -> URL? {
        var result: URL?

        queue.sync {
            var manifest = loadManifest(forVault: vaultId)
            guard var entry = manifest.entries[vaultPath] else { return }

            let localURL = cacheDirectory(forVault: vaultId)
                .appendingPathComponent(entry.localFilename)

            guard fileManager.fileExists(atPath: localURL.path) else {
                // Stale manifest entry; clean up
                manifest.entries.removeValue(forKey: vaultPath)
                // Need barrier to mutate, schedule it
                result = nil
                return
            }

            // Update LRU access time
            entry.lastAccessDate = Date()
            manifest.entries[vaultPath] = entry
            result = localURL
            // Defer the write; store in memory only
            manifestCache[vaultId] = manifest
            manifestDirty.insert(vaultId)
        }

        // If we found a stale entry, clean it up with a barrier write
        if result == nil {
            queue.async(flags: .barrier) { [self] in
                var manifest = loadManifest(forVault: vaultId)
                if let entry = manifest.entries[vaultPath] {
                    let localURL = cacheDirectory(forVault: vaultId)
                        .appendingPathComponent(entry.localFilename)
                    if !fileManager.fileExists(atPath: localURL.path) {
                        manifest.entries.removeValue(forKey: vaultPath)
                        storeManifest(manifest, forVault: vaultId)
                        flushManifest(forVault: vaultId)
                    }
                }
            }
        }

        if result != nil {
            logger.debug("Cache hit for \(vaultPath)")
        }

        return result
    }

    /// Caches decrypted file data for a vault path.
    /// The `sourceURL` should point to the already-decrypted temp file on disk.
    /// Returns the cached file URL on success.
    @discardableResult
    func cache(
        from sourceURL: URL,
        forVault vaultId: String,
        path vaultPath: String,
        etag: String? = nil
    ) -> URL? {
        // Read file attributes outside the lock
        guard let attrs = try? fileManager.attributesOfItem(atPath: sourceURL.path),
              let fileSize = attrs[.size] as? Int64
        else {
            logger.error("Cannot read attributes of source file for caching")
            return nil
        }

        let dir = cacheDirectory(forVault: vaultId)
        let localFilename = stableFilename(for: vaultPath)
        let destURL = dir.appendingPathComponent(localFilename)

        // Perform directory creation and file copy outside the barrier
        try? fileManager.createDirectory(at: dir, withIntermediateDirectories: true)

        do {
            if fileManager.fileExists(atPath: destURL.path) {
                try fileManager.removeItem(at: destURL)
            }
            try fileManager.copyItem(at: sourceURL, to: destURL)
        } catch {
            logger.error("Failed to copy file to cache: \(error.localizedDescription)")
            return nil
        }

        // Apply file protection for iOS
        applyFileProtection(to: destURL)

        // Update manifest under barrier
        queue.sync(flags: .barrier) { [self] in
            var manifest = loadManifest(forVault: vaultId)
            let now = Date()
            manifest.entries[vaultPath] = CacheEntry(
                vaultPath: vaultPath,
                localFilename: localFilename,
                cachedDate: now,
                size: fileSize,
                etag: etag,
                lastAccessDate: now
            )
            storeManifest(manifest, forVault: vaultId)

            evictIfNeeded(forVault: vaultId, manifest: &manifest)
            storeManifest(manifest, forVault: vaultId)
            flushManifest(forVault: vaultId)
        }

        logger.debug("Cached \(vaultPath) (\(fileSize) bytes)")

        return destURL
    }

    /// Invalidates a single cached entry.
    func invalidate(forVault vaultId: String, path vaultPath: String) {
        queue.sync(flags: .barrier) { [self] in
            var manifest = loadManifest(forVault: vaultId)
            guard let entry = manifest.entries.removeValue(forKey: vaultPath) else { return }

            let localURL = cacheDirectory(forVault: vaultId)
                .appendingPathComponent(entry.localFilename)
            try? fileManager.removeItem(at: localURL)
            storeManifest(manifest, forVault: vaultId)
            flushManifest(forVault: vaultId)

            logger.debug("Invalidated cache for \(vaultPath)")
        }
    }

    /// Clears the entire cache for a vault.
    func clearAll(forVault vaultId: String) {
        queue.sync(flags: .barrier) { [self] in
            let dir = cacheDirectory(forVault: vaultId)
            try? fileManager.removeItem(at: dir)
            manifestCache.removeValue(forKey: vaultId)
            manifestDirty.remove(vaultId)

            logger.info("Cleared cache for vault \(vaultId)")
        }
    }

    /// Clears the cache for all vaults.
    func clearAll() {
        queue.sync(flags: .barrier) { [self] in
            try? fileManager.removeItem(at: cacheRootDirectory)
            manifestCache.removeAll()
            manifestDirty.removeAll()

            logger.info("Cleared all vault caches")
        }
    }

    /// Returns the total cache size in bytes for a vault, reconciled against actual disk state.
    func cacheSize(forVault vaultId: String) -> Int64 {
        queue.sync {
            reconcileManifest(forVault: vaultId)
            let manifest = loadManifest(forVault: vaultId)
            return manifest.entries.values.reduce(0) { $0 + $1.size }
        }
    }

    /// Returns the total cache size across all vaults, reconciled against actual disk state.
    func totalCacheSize() -> Int64 {
        queue.sync {
            guard let contents = try? fileManager.contentsOfDirectory(
                at: cacheRootDirectory,
                includingPropertiesForKeys: nil
            ) else { return 0 }

            var total: Int64 = 0
            for dir in contents {
                let vaultId = dir.lastPathComponent
                reconcileManifest(forVault: vaultId)
                let manifest = loadManifest(forVault: vaultId)
                total += manifest.entries.values.reduce(0) { $0 + $1.size }
            }
            return total
        }
    }

    /// Returns the number of cached entries for a vault.
    func entryCount(forVault vaultId: String) -> Int {
        queue.sync {
            let manifest = loadManifest(forVault: vaultId)
            return manifest.entries.count
        }
    }

    // MARK: - Reconciliation

    /// Removes manifest entries whose backing files no longer exist on disk
    /// (e.g., if the OS purged cached files). Updates sizes accordingly.
    /// Must be called within the queue.
    private func reconcileManifest(forVault vaultId: String) {
        var manifest = loadManifest(forVault: vaultId)
        let dir = cacheDirectory(forVault: vaultId)
        var changed = false

        for (path, entry) in manifest.entries {
            let localURL = dir.appendingPathComponent(entry.localFilename)
            if !fileManager.fileExists(atPath: localURL.path) {
                manifest.entries.removeValue(forKey: path)
                changed = true
                logger.debug("Reconciliation: removed stale entry \(path)")
            }
        }

        if changed {
            manifestCache[vaultId] = manifest
            manifestDirty.insert(vaultId)
        }
    }

    // MARK: - LRU Eviction

    private func evictIfNeeded(forVault vaultId: String, manifest: inout CacheManifest) {
        var currentSize = manifest.entries.values.reduce(0) { $0 + $1.size }
        guard currentSize > maxCacheSize else { return }

        // Sort entries by last access date (oldest first)
        let sorted = manifest.entries.sorted { $0.value.lastAccessDate < $1.value.lastAccessDate }

        let dir = cacheDirectory(forVault: vaultId)
        for (path, entry) in sorted {
            guard currentSize > maxCacheSize else { break }

            let localURL = dir.appendingPathComponent(entry.localFilename)
            try? fileManager.removeItem(at: localURL)
            manifest.entries.removeValue(forKey: path)
            currentSize -= entry.size

            logger.debug("Evicted \(path) from cache (LRU)")
        }
    }

    // MARK: - Helpers

    /// Generates a stable filename from a vault path using SHA-256.
    private func stableFilename(for vaultPath: String) -> String {
        let pathData = Data(vaultPath.utf8)
        let digest = SHA256.hash(data: pathData)
        let hash = digest.map { String(format: "%02x", $0) }.joined()
        let ext = (vaultPath as NSString).pathExtension
        if ext.isEmpty {
            return hash
        }
        return "\(hash).\(ext)"
    }
}
