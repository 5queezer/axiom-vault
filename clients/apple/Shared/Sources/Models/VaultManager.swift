import Foundation
import SwiftUI

/// Base vault manager with shared logic for iOS and macOS
@MainActor
class VaultManager: ObservableObject {
    @Published var isVaultOpen = false
    @Published var currentPath = "/"
    @Published var entries: [VaultEntry] = []
    @Published var vaultInfo: VaultInfo?
    @Published var errorMessage: String?
    @Published var isLoading = false
    @Published var pathStack: [String] = ["/"]
    @Published var cacheSize: Int64 = 0

    // MARK: - Vault lifecycle

    func closeVault() {
        VaultCore.shared.closeVault()
        isVaultOpen = false
        currentPath = "/"
        pathStack = ["/"]
        entries = []
        vaultInfo = nil
        #if os(macOS)
        currentVaultName = nil
        #endif
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
        refreshCacheSize()
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

    // MARK: - Cache

    func refreshCacheSize() {
        cacheSize = CacheManager.shared.totalCacheSize()
    }

    func clearCache() {
        if let info = vaultInfo {
            CacheManager.shared.clearAll(forVault: info.vaultId)
        } else {
            CacheManager.shared.clearAll()
        }
        cacheSize = 0
    }

    // MARK: - Helpers

    func vaultPath(for name: String) -> String {
        currentPath == "/" ? "/\(name)" : "\(currentPath)/\(name)"
    }

    /// Recursively import a local directory into the vault, preserving structure.
    func addDirectory(from localURL: URL, to baseVaultPath: String) throws {
        let fm = FileManager.default
        let dirName = localURL.lastPathComponent
        let targetPath = baseVaultPath == "/"
            ? "/\(dirName)"
            : "\(baseVaultPath)/\(dirName)"

        try VaultCore.shared.createDirectory(at: targetPath)

        guard let enumerator = fm.enumerator(
            at: localURL,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        ) else { return }

        for case let fileURL as URL in enumerator {
            let resourceValues = try fileURL.resourceValues(forKeys: [.isDirectoryKey])
            let relativePath = fileURL.path.replacingOccurrences(of: localURL.path, with: "")
            let vaultEntryPath = targetPath + relativePath

            if resourceValues.isDirectory == true {
                try VaultCore.shared.createDirectory(at: vaultEntryPath)
            } else {
                try VaultCore.shared.addFile(from: fileURL.path, to: vaultEntryPath)
            }
        }
    }
}
