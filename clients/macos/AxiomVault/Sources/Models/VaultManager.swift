import Foundation
import SwiftUI

/// Persisted vault bookmark for remembering user-selected vault locations
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

@MainActor
class VaultManager: ObservableObject {
    @Published var isVaultOpen = false
    @Published var currentPath = "/"
    @Published var entries: [VaultEntry] = []
    @Published var vaultInfo: VaultInfo?
    @Published var errorMessage: String?
    @Published var isLoading = false
    @Published var pathStack: [String] = ["/"]
    @Published var vaultBookmarks: [VaultBookmark] = []
    @Published var currentVaultName: String?

    private let bookmarksKey = "com.axiomvault.macos.vaultBookmarks"

    init() {
        loadBookmarks()
    }

    // MARK: - Vault lifecycle

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

    func closeVault() {
        VaultCore.shared.closeVault()
        isVaultOpen = false
        currentVaultName = nil
        currentPath = "/"
        pathStack = ["/"]
        entries = []
        vaultInfo = nil
    }

    // MARK: - Navigation

    func navigateTo(directory: String) async {
        if directory == ".." {
            if pathStack.count > 1 {
                pathStack.removeLast()
            }
        } else {
            let newPath = currentPath == "/"
                ? "/\(directory)"
                : "\(currentPath)/\(directory)"
            pathStack.append(newPath)
        }
        currentPath = pathStack.last ?? "/"
        await refreshEntries()
    }

    func navigateToStackIndex(_ index: Int) async {
        guard index < pathStack.count else { return }
        pathStack = Array(pathStack.prefix(index + 1))
        currentPath = pathStack.last ?? "/"
        await refreshEntries()
    }

    // MARK: - File operations

    func addFiles(from urls: [URL]) async {
        isLoading = true
        defer { isLoading = false }

        for url in urls {
            let didAccess = url.startAccessingSecurityScopedResource()
            defer { if didAccess { url.stopAccessingSecurityScopedResource() } }

            let vaultPath = currentPath == "/"
                ? "/\(url.lastPathComponent)"
                : "\(currentPath)/\(url.lastPathComponent)"

            do {
                try VaultCore.shared.addFile(from: url.path, to: vaultPath)
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

        let vaultPath = currentPath == "/"
            ? "/\(entry.name)"
            : "\(currentPath)/\(entry.name)"

        let localPath = destinationURL.appendingPathComponent(entry.name).path

        do {
            try VaultCore.shared.extractFile(from: vaultPath, to: localPath)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func createDirectory(name: String) async {
        let vaultPath = currentPath == "/"
            ? "/\(name)"
            : "\(currentPath)/\(name)"

        do {
            try VaultCore.shared.createDirectory(at: vaultPath)
            await refreshEntries()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func deleteEntry(_ entry: VaultEntry) async {
        let vaultPath = currentPath == "/"
            ? "/\(entry.name)"
            : "\(currentPath)/\(entry.name)"

        do {
            try VaultCore.shared.removeEntry(at: vaultPath)
            await refreshEntries()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func changePassword(old: String, new: String) async {
        isLoading = true
        defer { isLoading = false }

        do {
            try VaultCore.shared.changePassword(old: old, new: new)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    // MARK: - Refresh

    func refreshState() async {
        await refreshVaultInfo()
        await refreshEntries()
    }

    func refreshVaultInfo() async {
        do {
            vaultInfo = try VaultCore.shared.getVaultInfo()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func refreshEntries() async {
        do {
            entries = try VaultCore.shared.listDirectory(at: currentPath)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    // MARK: - Bookmarks (sandboxed file access persistence)

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

            // Remove existing bookmark for same path
            vaultBookmarks.removeAll { $0.url?.path == url.path }
            vaultBookmarks.insert(bookmark, at: 0)
            persistBookmarks()
        } catch {
            // Non-fatal: vault still works, just won't appear in recents
        }
    }

    private func updateBookmarkLastOpened(for url: URL) {
        if let index = vaultBookmarks.firstIndex(where: { $0.url?.path == url.path }) {
            var updated = vaultBookmarks[index]
            updated.lastOpened = Date()
            vaultBookmarks[index] = updated
            persistBookmarks()
        } else {
            saveBookmark(for: url)
        }
    }

    func removeBookmark(_ bookmark: VaultBookmark) {
        vaultBookmarks.removeAll { $0.id == bookmark.id }
        persistBookmarks()
    }

    private func loadBookmarks() {
        guard let data = UserDefaults.standard.data(forKey: bookmarksKey),
              let bookmarks = try? JSONDecoder().decode([VaultBookmark].self, from: data)
        else { return }
        vaultBookmarks = bookmarks
    }

    private func persistBookmarks() {
        if let data = try? JSONEncoder().encode(vaultBookmarks) {
            UserDefaults.standard.set(data, forKey: bookmarksKey)
        }
    }
}
