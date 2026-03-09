import Foundation
import SwiftUI

// MARK: - Sync types

/// Current synchronization status
enum SyncStatus: String, CaseIterable {
    case synced = "Synced"
    case syncing = "Syncing"
    case error = "Error"
    case offline = "Offline"
    case notConfigured = "Not Connected"

    var iconName: String {
        switch self {
        case .synced: return "checkmark.icloud"
        case .syncing: return "arrow.triangle.2.circlepath.icloud"
        case .error: return "exclamationmark.icloud"
        case .offline: return "icloud.slash"
        case .notConfigured: return "icloud.slash"
        }
    }

    var tintColor: Color {
        switch self {
        case .synced: return .green
        case .syncing: return .blue
        case .error: return .red
        case .offline: return .secondary
        case .notConfigured: return .secondary
        }
    }
}

/// Strategy for resolving sync conflicts
enum ConflictResolutionStrategy: String, CaseIterable, Identifiable {
    case keepBoth = "keep-both"
    case preferLocal = "prefer-local"
    case preferRemote = "prefer-remote"

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .keepBoth: return "Keep Both"
        case .preferLocal: return "Prefer Local"
        case .preferRemote: return "Prefer Remote"
        }
    }

    var description: String {
        switch self {
        case .keepBoth: return "Keep both versions and rename the conflicting file"
        case .preferLocal: return "Overwrite remote changes with local version"
        case .preferRemote: return "Overwrite local changes with remote version"
        }
    }
}

/// Auto-sync interval options
enum SyncInterval: Int, CaseIterable, Identifiable {
    case oneMinute = 60
    case fiveMinutes = 300
    case fifteenMinutes = 900
    case thirtyMinutes = 1800
    case oneHour = 3600

    var id: Int { rawValue }

    var displayName: String {
        switch self {
        case .oneMinute: return "1 minute"
        case .fiveMinutes: return "5 minutes"
        case .fifteenMinutes: return "15 minutes"
        case .thirtyMinutes: return "30 minutes"
        case .oneHour: return "1 hour"
        }
    }
}

/// An entry in the sync history log
struct SyncLogEntry: Identifiable {
    let id: UUID
    let date: Date
    let status: SyncStatus
    let message: String
    let filesChanged: Int

    init(date: Date, status: SyncStatus, message: String, filesChanged: Int = 0) {
        self.id = UUID()
        self.date = date
        self.status = status
        self.message = message
        self.filesChanged = filesChanged
    }
}

// MARK: - SyncManager

/// Manages cloud synchronization state and operations.
///
/// The underlying sync engine is not wired into the Apple clients yet, so this
/// manager keeps the UI in an explicit preview/offline state instead of
/// reporting fake successful syncs. Persisted settings are scoped per vault.
@MainActor
class SyncManager: ObservableObject {
    @Published var syncStatus: SyncStatus = .notConfigured
    @Published var lastSyncDate: Date?
    @Published var isSyncing = false
    @Published var syncError: String?
    @Published var syncLog: [SyncLogEntry] = []
    @Published private(set) var activeVaultKey: String?

    @Published var autoSyncEnabled: Bool = false {
        didSet {
            guard !isLoadingScopedState else { return }
            persist(autoSyncEnabled, for: .autoSyncEnabled)
            if autoSyncEnabled, isSyncAvailable {
                scheduleAutoSync()
            } else {
                cancelAutoSync()
            }
        }
    }

    @Published var syncInterval: SyncInterval = .fifteenMinutes {
        didSet {
            guard !isLoadingScopedState else { return }
            persist(syncInterval.rawValue, for: .syncInterval)
            if autoSyncEnabled, isSyncAvailable {
                scheduleAutoSync()
            }
        }
    }

    @Published var conflictStrategy: ConflictResolutionStrategy = .keepBoth {
        didSet {
            guard !isLoadingScopedState else { return }
            persist(conflictStrategy.rawValue, for: .conflictStrategy)
        }
    }

    private var autoSyncTimer: Timer?
    private var isLoadingScopedState = false

    private enum KeyKind: String {
        case autoSyncEnabled
        case syncInterval
        case conflictStrategy
        case lastSyncDate
    }

    init() {}

    var isSyncAvailable: Bool { false }

    var availabilityMessage: String {
        guard activeVaultKey != nil else {
            return "Open a vault to configure sync settings."
        }
        return "Cloud sync is not yet connected to the backend. Settings are saved per-vault and will take effect once the sync engine is integrated."
    }

    func setActiveVault(_ vaultKey: String?) {
        if activeVaultKey == vaultKey { return }
        cancelAutoSync()
        activeVaultKey = vaultKey
        loadScopedState()
    }

    func sync() async {
        guard isSyncAvailable else {
            syncStatus = .notConfigured
            syncError = "Sync is not yet connected to the backend. This feature is under development."
            appendLog(status: .notConfigured, message: "Sync not connected — backend integration pending")
            return
        }

        guard !isSyncing else { return }
        isSyncing = true
        syncStatus = .syncing
        syncError = nil
        isSyncing = false
    }

    func configureSyncInterval(_ interval: SyncInterval) {
        syncInterval = interval
    }

    func setConflictStrategy(_ strategy: ConflictResolutionStrategy) {
        conflictStrategy = strategy
    }

    func clearSyncLog() {
        syncLog.removeAll()
    }

    private func scheduleAutoSync() {
        cancelAutoSync()
        guard isSyncAvailable else { return }
        let interval = TimeInterval(syncInterval.rawValue)
        autoSyncTimer = Timer.scheduledTimer(withTimeInterval: interval, repeats: true) { [weak self] _ in
            Task { @MainActor [weak self] in
                await self?.sync()
            }
        }
    }

    private func cancelAutoSync() {
        autoSyncTimer?.invalidate()
        autoSyncTimer = nil
    }

    private func appendLog(status: SyncStatus, message: String, filesChanged: Int = 0) {
        let entry = SyncLogEntry(date: Date(), status: status, message: message, filesChanged: filesChanged)
        syncLog.insert(entry, at: 0)
        if syncLog.count > 50 {
            syncLog = Array(syncLog.prefix(50))
        }
    }

    private func scopedKey(_ kind: KeyKind) -> String? {
        guard let activeVaultKey else { return nil }
        return "com.axiomvault.sync.\(activeVaultKey).\(kind.rawValue)"
    }

    private func persist(_ value: Any, for kind: KeyKind) {
        guard let key = scopedKey(kind) else { return }
        UserDefaults.standard.set(value, forKey: key)
    }

    private func loadScopedState() {
        isLoadingScopedState = true
        defer { isLoadingScopedState = false }

        guard activeVaultKey != nil else {
            autoSyncEnabled = false
            syncInterval = .fifteenMinutes
            conflictStrategy = .keepBoth
            lastSyncDate = nil
            syncStatus = .notConfigured
            syncError = nil
            syncLog = []
            return
        }

        let defaults = UserDefaults.standard
        autoSyncEnabled = defaults.bool(forKey: scopedKey(.autoSyncEnabled)!)
        syncInterval = SyncInterval(rawValue: defaults.integer(forKey: scopedKey(.syncInterval)!)) ?? .fifteenMinutes
        conflictStrategy = ConflictResolutionStrategy(rawValue: defaults.string(forKey: scopedKey(.conflictStrategy)!) ?? "") ?? .keepBoth
        lastSyncDate = defaults.object(forKey: scopedKey(.lastSyncDate)!) as? Date
        syncStatus = .notConfigured
        syncError = nil
        syncLog = []
    }

    var lastSyncDescription: String {
        guard let date = lastSyncDate else {
            return "Never"
        }

        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: Date())
    }
}
