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

    /// Set after a successful password unlock to offer biometric enrollment
    @Published var shouldOfferBiometricSave = false
    /// The vault path that was just unlocked (used for biometric save prompt)
    var lastUnlockedVaultPath: String?

    @Published var autoLockDuration: AutoLockDuration {
        didSet {
            UserDefaults.standard.set(autoLockDuration.rawValue, forKey: Self.autoLockKey)
            resetAutoLockTimer()
        }
    }

    private var autoLockTimer: Timer?
    static let autoLockKey = "autoLockDuration"

    init() {
        if let stored = UserDefaults.standard.object(forKey: Self.autoLockKey) as? Int,
           let duration = AutoLockDuration(rawValue: stored) {
            self.autoLockDuration = duration
        } else {
            self.autoLockDuration = .fifteenMinutes
            UserDefaults.standard.set(AutoLockDuration.fifteenMinutes.rawValue, forKey: Self.autoLockKey)
        }
    }

    // MARK: - Biometric helpers

    /// Whether biometric unlock is available for a given vault path
    func canUseBiometric(for vaultPath: String) -> Bool {
        BiometricAuth.shared.isBiometricAvailable
            && BiometricAuth.shared.hasStoredPassword(for: vaultPath)
    }

    /// Save the password for biometric unlock
    func enableBiometric(password: String, vaultPath: String) {
        do {
            try BiometricAuth.shared.storePassword(password, for: vaultPath)
        } catch {
            errorMessage = error.localizedDescription
        }
        shouldOfferBiometricSave = false
    }

    /// Remove stored biometric password for a vault
    func disableBiometric(for vaultPath: String) {
        do {
            try BiometricAuth.shared.removePassword(for: vaultPath)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    /// Dismiss the biometric save offer
    func declineBiometricSave() {
        shouldOfferBiometricSave = false
    }

    // MARK: - Auto-lock timer

    func resetAutoLockTimer() {
        guard isVaultOpen, autoLockDuration != .never else {
            cancelAutoLockTimer()
            return
        }
        if let timer = autoLockTimer, timer.isValid {
            timer.fireDate = Date().addingTimeInterval(TimeInterval(autoLockDuration.rawValue))
            return
        }
        startAutoLockTimer()
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
        shouldOfferBiometricSave = false
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
