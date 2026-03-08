import Foundation
import UIKit

extension VaultManager {
    // MARK: - iOS system event observers

    func registeriOSAutoLockObservers() {
        NotificationCenter.default.addObserver(
            forName: UIApplication.didEnterBackgroundNotification,
            object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in
                guard let self, self.isVaultOpen else { return }
                self.closeVault()
            }
        }
    }

    /// Documents/Vaults directory for iOS vault storage
    var vaultsDirectory: URL {
        let paths = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)
        return paths[0].appendingPathComponent("Vaults", isDirectory: true)
    }

    func ensureVaultsDirectory() {
        try? FileManager.default.createDirectory(at: vaultsDirectory, withIntermediateDirectories: true)
    }

    func createVault(name: String, password: String) async {
        ensureVaultsDirectory()
        isLoading = true
        defer { isLoading = false }

        let vaultPath = vaultsDirectory.appendingPathComponent(name).path

        do {
            try VaultCore.shared.createVault(at: vaultPath, password: password)
            isVaultOpen = true
            await refreshState()
            resetAutoLockTimer()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func openVault(at path: String, password: String) async {
        isLoading = true
        defer { isLoading = false }

        do {
            try VaultCore.shared.openVault(at: path, password: password)
            isVaultOpen = true
            await refreshState()
            resetAutoLockTimer()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func addFile(from url: URL) async {
        isLoading = true
        defer { isLoading = false }

        do {
            var isDir: ObjCBool = false
            if FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir), isDir.boolValue {
                try addDirectory(from: url, to: currentPath)
            } else {
                let vaultFilePath = vaultPath(for: url.lastPathComponent)
                try VaultCore.shared.addFile(from: url.path, to: vaultFilePath)
            }
            await refreshEntries()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func extractFile(entry: VaultEntry, to url: URL) async {
        isLoading = true
        defer { isLoading = false }

        let vaultFilePath = vaultPath(for: entry.name)

        do {
            try VaultCore.shared.extractFile(from: vaultFilePath, to: url.path)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func listExistingVaults() -> [URL] {
        ensureVaultsDirectory()
        do {
            let contents = try FileManager.default.contentsOfDirectory(
                at: vaultsDirectory,
                includingPropertiesForKeys: [.isDirectoryKey],
                options: [.skipsHiddenFiles]
            )
            return contents.filter { url in
                var isDir: ObjCBool = false
                return FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir) && isDir.boolValue
            }
        } catch {
            return []
        }
    }
}
