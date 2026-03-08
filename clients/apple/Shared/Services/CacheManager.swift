import Foundation
import os.log

/// Manages a file-based cache of decrypted vault contents for the File Provider extension.
/// Uses a JSON manifest to track cached files and implements LRU eviction.
class CacheManager {
    static let shared = CacheManager()

    private let logger = Logger(subsystem: "com.axiomvault.macos.fileprovider", category: "cache")
    private let fileManager = FileManager.default
    private let lock = NSLock()

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

    /// Whether to clear the cache when the vault is locked
    var clearOnLock: Bool {
        get { UserDefaults.standard.bool(forKey: Self.clearOnLockKey) }
        set { UserDefaults.standard.set(newValue, forKey: Self.clearOnLockKey) }
    }

    private init() {}

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

    private func loadManifest(forVault vaultId: String) -> CacheManifest {
        let url = manifestURL(forVault: vaultId)
        guard let data = try? Data(contentsOf: url),
              let manifest = try? JSONDecoder().decode(CacheManifest.self, from: data)
        else {
            return .empty
        }
        return manifest
    }

    private func saveManifest(_ manifest: CacheManifest, forVault vaultId: String) {
        let url = manifestURL(forVault: vaultId)
        let dir = url.deletingLastPathComponent()
        try? fileManager.createDirectory(at: dir, withIntermediateDirectories: true)

        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]

        if let data = try? encoder.encode(manifest) {
            try? data.write(to: url, options: .atomic)
        }
    }

    // MARK: - Public API

    /// Returns the local cached URL for a vault path, or nil if not cached.
    /// Updates the last-access date for LRU tracking.
    func cachedURL(forVault vaultId: String, path vaultPath: String) -> URL? {
        lock.lock()
        defer { lock.unlock() }

        var manifest = loadManifest(forVault: vaultId)
        guard var entry = manifest.entries[vaultPath] else { return nil }

        let localURL = cacheDirectory(forVault: vaultId)
            .appendingPathComponent(entry.localFilename)

        guard fileManager.fileExists(atPath: localURL.path) else {
            // Stale manifest entry; clean up
            manifest.entries.removeValue(forKey: vaultPath)
            saveManifest(manifest, forVault: vaultId)
            return nil
        }

        // Update LRU access time
        entry.lastAccessDate = Date()
        manifest.entries[vaultPath] = entry
        saveManifest(manifest, forVault: vaultId)

        logger.debug("Cache hit for \(vaultPath)")
        return localURL
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
        lock.lock()
        defer { lock.unlock() }

        let dir = cacheDirectory(forVault: vaultId)
        try? fileManager.createDirectory(at: dir, withIntermediateDirectories: true)

        // Determine file size
        guard let attrs = try? fileManager.attributesOfItem(atPath: sourceURL.path),
              let fileSize = attrs[.size] as? Int64
        else {
            logger.error("Cannot read attributes of source file for caching")
            return nil
        }

        // Generate a stable filename based on vault path to allow overwrites
        let localFilename = stableFilename(for: vaultPath)
        let destURL = dir.appendingPathComponent(localFilename)

        // Copy file to cache
        do {
            if fileManager.fileExists(atPath: destURL.path) {
                try fileManager.removeItem(at: destURL)
            }
            try fileManager.copyItem(at: sourceURL, to: destURL)
        } catch {
            logger.error("Failed to copy file to cache: \(error.localizedDescription)")
            return nil
        }

        // Update manifest
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
        saveManifest(manifest, forVault: vaultId)

        logger.debug("Cached \(vaultPath) (\(fileSize) bytes)")

        // Evict if over budget (non-blocking for the caller)
        evictIfNeeded(forVault: vaultId, manifest: &manifest)
        saveManifest(manifest, forVault: vaultId)

        return destURL
    }

    /// Invalidates a single cached entry.
    func invalidate(forVault vaultId: String, path vaultPath: String) {
        lock.lock()
        defer { lock.unlock() }

        var manifest = loadManifest(forVault: vaultId)
        guard let entry = manifest.entries.removeValue(forKey: vaultPath) else { return }

        let localURL = cacheDirectory(forVault: vaultId)
            .appendingPathComponent(entry.localFilename)
        try? fileManager.removeItem(at: localURL)
        saveManifest(manifest, forVault: vaultId)

        logger.debug("Invalidated cache for \(vaultPath)")
    }

    /// Clears the entire cache for a vault.
    func clearAll(forVault vaultId: String) {
        lock.lock()
        defer { lock.unlock() }

        let dir = cacheDirectory(forVault: vaultId)
        try? fileManager.removeItem(at: dir)

        logger.info("Cleared cache for vault \(vaultId)")
    }

    /// Clears the cache for all vaults.
    func clearAll() {
        lock.lock()
        defer { lock.unlock() }

        try? fileManager.removeItem(at: cacheRootDirectory)

        logger.info("Cleared all vault caches")
    }

    /// Returns the total cache size in bytes for a vault.
    func cacheSize(forVault vaultId: String) -> Int64 {
        lock.lock()
        defer { lock.unlock() }

        let manifest = loadManifest(forVault: vaultId)
        return manifest.entries.values.reduce(0) { $0 + $1.size }
    }

    /// Returns the total cache size across all vaults.
    func totalCacheSize() -> Int64 {
        lock.lock()
        defer { lock.unlock() }

        guard let contents = try? fileManager.contentsOfDirectory(
            at: cacheRootDirectory,
            includingPropertiesForKeys: nil
        ) else { return 0 }

        var total: Int64 = 0
        for dir in contents {
            let manifestURL = dir.appendingPathComponent("manifest.json")
            guard let data = try? Data(contentsOf: manifestURL),
                  let manifest = try? JSONDecoder().decode(CacheManifest.self, from: data)
            else { continue }
            total += manifest.entries.values.reduce(0) { $0 + $1.size }
        }
        return total
    }

    /// Returns the number of cached entries for a vault.
    func entryCount(forVault vaultId: String) -> Int {
        lock.lock()
        defer { lock.unlock() }

        let manifest = loadManifest(forVault: vaultId)
        return manifest.entries.count
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

    /// Generates a stable filename from a vault path using a hash.
    private func stableFilename(for vaultPath: String) -> String {
        // Use a simple hash-based name, preserving the extension for type identification
        let pathData = Data(vaultPath.utf8)
        let hash = pathData.reduce(0) { (result: UInt64, byte: UInt8) in
            result &* 31 &+ UInt64(byte)
        }
        let ext = (vaultPath as NSString).pathExtension
        if ext.isEmpty {
            return String(format: "%016llx", hash)
        }
        return String(format: "%016llx.%@", hash, ext)
    }
}
