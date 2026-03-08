import Foundation
import SwiftUI

// MARK: - Auto-lock duration

enum AutoLockDuration: Int, CaseIterable {
    case oneMinute = 60
    case fiveMinutes = 300
    case fifteenMinutes = 900
    case thirtyMinutes = 1800
    case never = 0

    var displayName: String {
        switch self {
        case .oneMinute: return "1 Minute"
        case .fiveMinutes: return "5 Minutes"
        case .fifteenMinutes: return "15 Minutes"
        case .thirtyMinutes: return "30 Minutes"
        case .never: return "Never"
        }
    }
}

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

    @Published var autoLockDuration: AutoLockDuration {
        didSet {
            UserDefaults.standard.set(autoLockDuration.rawValue, forKey: "autoLockDuration")
            resetAutoLockTimer()
        }
    }

    private var autoLockTimer: Timer?

    init() {
        UserDefaults.standard.register(defaults: ["autoLockDuration": AutoLockDuration.fifteenMinutes.rawValue])
        let saved = UserDefaults.standard.integer(forKey: "autoLockDuration")
        self.autoLockDuration = AutoLockDuration(rawValue: saved) ?? .fifteenMinutes
    }

    // MARK: - Auto-lock timer

    func resetAutoLockTimer() {
        guard isVaultOpen, autoLockDuration != .never else {
            cancelAutoLockTimer()
            return
        }
        if let timer = autoLockTimer, timer.isValid {
            timer.fireDate = Date().addingTimeInterval(TimeInterval(autoLockDuration.rawValue))
        } else {
            cancelAutoLockTimer()
            startAutoLockTimer()
        }
    }

    func startAutoLockTimer() {
        guard isVaultOpen, autoLockDuration != .never else { return }
        let duration = TimeInterval(autoLockDuration.rawValue)
        autoLockTimer = Timer.scheduledTimer(withTimeInterval: duration, repeats: false) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.closeVault()
            }
        }
    }

    func cancelAutoLockTimer() {
        autoLockTimer?.invalidate()
        autoLockTimer = nil
    }

    // MARK: - Vault lifecycle

    func closeVault() {
        cancelAutoLockTimer()
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
        resetAutoLockTimer()
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
        resetAutoLockTimer()
        guard index < pathStack.count else { return }
        pathStack = Array(pathStack.prefix(index + 1))
        currentPath = pathStack.last ?? "/"
        await refreshEntries()
    }

    // MARK: - File operations

    func createDirectory(name: String) async {
        resetAutoLockTimer()
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
        resetAutoLockTimer()
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
