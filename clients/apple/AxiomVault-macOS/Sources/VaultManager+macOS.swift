import Foundation

/// Persisted vault bookmark for remembering user-selected vault locations (sandbox-safe)
struct VaultBookmark: Identifiable, Codable {
    let id: UUID
    let name: String
    let bookmarkData: Data
    var lastOpened: Date?

    var url: URL? {
        var isStale = false
        return try? URL(
            resolvingBookmarkData: bookmarkData,
            options: .withSecurityScope,
            relativeTo: nil,
            bookmarkDataIsStale: &isStale
        )
    }
}

extension VaultManager {
    private static let bookmarksKey = "com.axiomvault.macos.vaultBookmarks"
    private static let currentVaultNameKey = "com.axiomvault.macos.currentVaultName"

    var currentVaultName: String? {
        get { UserDefaults.standard.string(forKey: Self.currentVaultNameKey) }
        set {
            UserDefaults.standard.set(newValue, forKey: Self.currentVaultNameKey)
            objectWillChange.send()
        }
    }

    var vaultBookmarks: [VaultBookmark] {
        get {
            guard let data = UserDefaults.standard.data(forKey: Self.bookmarksKey),
                  let bookmarks = try? JSONDecoder().decode([VaultBookmark].self, from: data)
            else { return [] }
            return bookmarks
        }
        set {
            if let data = try? JSONEncoder().encode(newValue) {
                UserDefaults.standard.set(data, forKey: Self.bookmarksKey)
            }
            objectWillChange.send()
        }
    }

    // MARK: - macOS vault operations

    func createVault(at url: URL, password: String) async {
        isLoading = true
        defer { isLoading = false }

        do {
            try VaultCore.shared.createVault(at: url.path, password: password)
            isVaultOpen = true
            currentVaultName = url.lastPathComponent
            saveBookmark(for: url)
            await refreshState()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func openVault(at url: URL, password: String) async {
        isLoading = true
        defer { isLoading = false }

        let didAccess = url.startAccessingSecurityScopedResource()
        defer { if didAccess { url.stopAccessingSecurityScopedResource() } }

        do {
            try VaultCore.shared.openVault(at: url.path, password: password)
            isVaultOpen = true
            currentVaultName = url.lastPathComponent
            updateBookmarkLastOpened(for: url)
            await refreshState()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func addFiles(from urls: [URL]) async {
        isLoading = true
        defer { isLoading = false }

        for url in urls {
            let didAccess = url.startAccessingSecurityScopedResource()
            defer { if didAccess { url.stopAccessingSecurityScopedResource() } }

            let filePath = vaultPath(for: url.lastPathComponent)

            do {
                try VaultCore.shared.addFile(from: url.path, to: filePath)
            } catch {
                errorMessage = error.localizedDescription
                return
            }
        }
        await refreshEntries()
    }

    func extractFile(entry: VaultEntry, to destinationURL: URL) async {
        isLoading = true
        defer { isLoading = false }

        let filePath = vaultPath(for: entry.name)
        let localPath = destinationURL.appendingPathComponent(entry.name).path

        do {
            try VaultCore.shared.extractFile(from: filePath, to: localPath)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    // MARK: - Bookmark management

    func removeBookmark(_ bookmark: VaultBookmark) {
        var bookmarks = vaultBookmarks
        bookmarks.removeAll { $0.id == bookmark.id }
        vaultBookmarks = bookmarks
    }

    private func saveBookmark(for url: URL) {
        do {
            let data = try url.bookmarkData(
                options: .withSecurityScope,
                includingResourceValuesForKeys: nil,
                relativeTo: nil
            )
            let bookmark = VaultBookmark(
                id: UUID(),
                name: url.lastPathComponent,
                bookmarkData: data,
                lastOpened: Date()
            )

            var bookmarks = vaultBookmarks
            bookmarks.removeAll { $0.url?.path == url.path }
            bookmarks.insert(bookmark, at: 0)
            vaultBookmarks = bookmarks
        } catch {
            // Non-fatal: vault still works, just won't appear in recents
        }
    }

    private func updateBookmarkLastOpened(for url: URL) {
        var bookmarks = vaultBookmarks
        if let index = bookmarks.firstIndex(where: { $0.url?.path == url.path }) {
            var updated = bookmarks[index]
            updated.lastOpened = Date()
            bookmarks[index] = updated
            vaultBookmarks = bookmarks
        } else {
            saveBookmark(for: url)
        }
    }
}
