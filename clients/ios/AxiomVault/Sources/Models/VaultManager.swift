import Foundation
import SwiftUI

/// Manages vault state and operations
class VaultManager: ObservableObject {
    @Published var isVaultOpen = false
    @Published var currentPath = "/"
    @Published var entries: [VaultEntry] = []
    @Published var vaultInfo: VaultInfo?
    @Published var errorMessage: String?
    @Published var isLoading = false
    @Published var pathStack: [String] = ["/"]

    private let core = VaultCore.shared
    private let fileManager = FileManager.default

    /// Get the documents directory for storing vaults
    var vaultsDirectory: URL {
        let paths = fileManager.urls(for: .documentDirectory, in: .userDomainMask)
        return paths[0].appendingPathComponent("Vaults", isDirectory: true)
    }

    init() {
        // Ensure vaults directory exists
        try? fileManager.createDirectory(at: vaultsDirectory, withIntermediateDirectories: true)
    }

    /// Create a new vault
    func createVault(name: String, password: String) async {
        await MainActor.run { isLoading = true }

        let vaultPath = vaultsDirectory.appendingPathComponent(name).path

        do {
            try core.createVault(at: vaultPath, password: password)
            await refreshState()
            await MainActor.run {
                isVaultOpen = true
                errorMessage = nil
            }
        } catch {
            await MainActor.run {
                errorMessage = error.localizedDescription
            }
        }

        await MainActor.run { isLoading = false }
    }

    /// Open an existing vault
    func openVault(at path: String, password: String) async {
        await MainActor.run { isLoading = true }

        do {
            try core.openVault(at: path, password: password)
            await refreshState()
            await MainActor.run {
                isVaultOpen = true
                errorMessage = nil
            }
        } catch {
            await MainActor.run {
                errorMessage = error.localizedDescription
            }
        }

        await MainActor.run { isLoading = false }
    }

    /// Close the current vault
    func closeVault() {
        core.closeVault()
        isVaultOpen = false
        currentPath = "/"
        pathStack = ["/"]
        entries = []
        vaultInfo = nil
    }

    /// Navigate to a directory
    func navigateTo(directory: String) async {
        let newPath: String
        if directory == ".." {
            // Go up one level
            if pathStack.count > 1 {
                await MainActor.run {
                    pathStack.removeLast()
                }
            }
            newPath = pathStack.last ?? "/"
        } else {
            // Navigate into directory
            if currentPath == "/" {
                newPath = "/\(directory)"
            } else {
                newPath = "\(currentPath)/\(directory)"
            }
            await MainActor.run {
                pathStack.append(newPath)
            }
        }

        await MainActor.run {
            currentPath = newPath
        }

        await refreshEntries()
    }

    /// Refresh vault information and current directory listing
    func refreshState() async {
        await refreshVaultInfo()
        await refreshEntries()
    }

    /// Refresh vault information
    func refreshVaultInfo() async {
        do {
            let info = try core.getVaultInfo()
            await MainActor.run {
                vaultInfo = info
            }
        } catch {
            await MainActor.run {
                errorMessage = error.localizedDescription
            }
        }
    }

    /// Refresh current directory listing
    func refreshEntries() async {
        await MainActor.run { isLoading = true }

        do {
            let newEntries = try core.listDirectory(at: currentPath)
            await MainActor.run {
                entries = newEntries.sorted { $0.name < $1.name }
                errorMessage = nil
            }
        } catch {
            await MainActor.run {
                errorMessage = error.localizedDescription
            }
        }

        await MainActor.run { isLoading = false }
    }

    /// Add a file to the vault
    func addFile(from url: URL) async {
        await MainActor.run { isLoading = true }

        let vaultPath: String
        if currentPath == "/" {
            vaultPath = "/\(url.lastPathComponent)"
        } else {
            vaultPath = "\(currentPath)/\(url.lastPathComponent)"
        }

        do {
            try core.addFile(from: url.path, to: vaultPath)
            await refreshEntries()
            await MainActor.run {
                errorMessage = nil
            }
        } catch {
            await MainActor.run {
                errorMessage = error.localizedDescription
            }
        }

        await MainActor.run { isLoading = false }
    }

    /// Extract a file from the vault
    func extractFile(entry: VaultEntry, to url: URL) async {
        await MainActor.run { isLoading = true }

        let vaultPath: String
        if currentPath == "/" {
            vaultPath = "/\(entry.name)"
        } else {
            vaultPath = "\(currentPath)/\(entry.name)"
        }

        do {
            try core.extractFile(from: vaultPath, to: url.path)
            await MainActor.run {
                errorMessage = nil
            }
        } catch {
            await MainActor.run {
                errorMessage = error.localizedDescription
            }
        }

        await MainActor.run { isLoading = false }
    }

    /// Create a directory in the vault
    func createDirectory(name: String) async {
        await MainActor.run { isLoading = true }

        let vaultPath: String
        if currentPath == "/" {
            vaultPath = "/\(name)"
        } else {
            vaultPath = "\(currentPath)/\(name)"
        }

        do {
            try core.createDirectory(at: vaultPath)
            await refreshEntries()
            await MainActor.run {
                errorMessage = nil
            }
        } catch {
            await MainActor.run {
                errorMessage = error.localizedDescription
            }
        }

        await MainActor.run { isLoading = false }
    }

    /// Delete an entry from the vault
    func deleteEntry(_ entry: VaultEntry) async {
        await MainActor.run { isLoading = true }

        let vaultPath: String
        if currentPath == "/" {
            vaultPath = "/\(entry.name)"
        } else {
            vaultPath = "\(currentPath)/\(entry.name)"
        }

        do {
            try core.removeEntry(at: vaultPath)
            await refreshEntries()
            await MainActor.run {
                errorMessage = nil
            }
        } catch {
            await MainActor.run {
                errorMessage = error.localizedDescription
            }
        }

        await MainActor.run { isLoading = false }
    }

    /// Change vault password
    func changePassword(old: String, new: String) async {
        await MainActor.run { isLoading = true }

        do {
            try core.changePassword(old: old, new: new)
            await MainActor.run {
                errorMessage = nil
            }
        } catch {
            await MainActor.run {
                errorMessage = error.localizedDescription
            }
        }

        await MainActor.run { isLoading = false }
    }

    /// List existing vaults
    func listExistingVaults() -> [URL] {
        do {
            let contents = try fileManager.contentsOfDirectory(
                at: vaultsDirectory,
                includingPropertiesForKeys: [.isDirectoryKey],
                options: [.skipsHiddenFiles]
            )
            return contents.filter { url in
                var isDir: ObjCBool = false
                return fileManager.fileExists(atPath: url.path, isDirectory: &isDir) && isDir.boolValue
            }
        } catch {
            return []
        }
    }
}
