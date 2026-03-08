import Foundation
import SwiftUI

// MARK: - Sync types

/// Current synchronization status
enum SyncStatus: String, CaseIterable {
    case synced = "Synced"
    case syncing = "Syncing"
    case error = "Error"
    case offline = "Offline"

    var iconName: String {
        switch self {
        case .synced: return "checkmark.icloud"
        case .syncing: return "arrow.triangle.2.circlepath.icloud"
        case .error: return "exclamationmark.icloud"
        case .offline: return "icloud.slash"
        }
    }

    var tintColor: Color {
        switch self {
        case .synced: return .green
        case .syncing: return .blue
        case .error: return .red
        case .offline: return .secondary
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
/// Uses placeholder/mock implementations since FFI sync bindings are not yet
/// fully implemented. Settings are persisted via UserDefaults.
@MainActor
class SyncManager: ObservableObject {
    // MARK: - Published state

    @Published var syncStatus: SyncStatus = .offline
    @Published var lastSyncDate: Date?
    @Published var isSyncing = false
    @Published var syncError: String?
    @Published var syncLog: [SyncLogEntry] = []

    // MARK: - Settings (persisted via UserDefaults)

    @Published var autoSyncEnabled: Bool {
        didSet {
            UserDefaults.standard.set(autoSyncEnabled, forKey: Keys.autoSyncEnabled)
            if autoSyncEnabled {
                scheduleAutoSync()
            } else {
                cancelAutoSync()
            }
        }
    }

    @Published var syncInterval: SyncInterval {
        didSet {
            UserDefaults.standard.set(syncInterval.rawValue, forKey: Keys.syncInterval)
            if autoSyncEnabled {
                scheduleAutoSync()
            }
        }
    }

    @Published var conflictStrategy: ConflictResolutionStrategy {
        didSet {
            UserDefaults.standard.set(conflictStrategy.rawValue, forKey: Keys.conflictStrategy)
        }
    }

    // MARK: - Private

    private var autoSyncTimer: Timer?

    private enum Keys {
        static let autoSyncEnabled = "com.axiomvault.sync.autoSyncEnabled"
        static let syncInterval = "com.axiomvault.sync.interval"
        static let conflictStrategy = "com.axiomvault.sync.conflictStrategy"
        static let lastSyncDate = "com.axiomvault.sync.lastSyncDate"
    }

    // MARK: - Init

    init() {
        // Restore persisted settings
        self.autoSyncEnabled = UserDefaults.standard.bool(forKey: Keys.autoSyncEnabled)

        let storedInterval = UserDefaults.standard.integer(forKey: Keys.syncInterval)
        self.syncInterval = SyncInterval(rawValue: storedInterval) ?? .fifteenMinutes

        let storedStrategy = UserDefaults.standard.string(forKey: Keys.conflictStrategy) ?? ""
        self.conflictStrategy = ConflictResolutionStrategy(rawValue: storedStrategy) ?? .keepBoth

        self.lastSyncDate = UserDefaults.standard.object(forKey: Keys.lastSyncDate) as? Date

        if autoSyncEnabled {
            scheduleAutoSync()
        }
    }

    // MARK: - Sync operations

    /// Trigger a manual sync. Currently a placeholder that simulates a sync cycle.
    func sync() async {
        guard !isSyncing else { return }

        isSyncing = true
        syncStatus = .syncing
        syncError = nil

        // Placeholder: simulate sync delay
        // In production this would call into the Rust sync engine via FFI
        do {
            try await Task.sleep(nanoseconds: 1_500_000_000)  // 1.5s

            let now = Date()
            lastSyncDate = now
            UserDefaults.standard.set(now, forKey: Keys.lastSyncDate)
            syncStatus = .synced

            let entry = SyncLogEntry(
                date: now,
                status: .synced,
                message: "Sync completed successfully",
                filesChanged: 0
            )
            syncLog.insert(entry, at: 0)

            // Keep log to a reasonable size
            if syncLog.count > 50 {
                syncLog = Array(syncLog.prefix(50))
            }
        } catch {
            syncStatus = .error
            syncError = "Sync was cancelled"

            let entry = SyncLogEntry(
                date: Date(),
                status: .error,
                message: "Sync cancelled"
            )
            syncLog.insert(entry, at: 0)
        }

        isSyncing = false
    }

    /// Configure the auto-sync interval.
    func configureSyncInterval(_ interval: SyncInterval) {
        syncInterval = interval
    }

    /// Set the conflict resolution strategy.
    func setConflictStrategy(_ strategy: ConflictResolutionStrategy) {
        conflictStrategy = strategy
    }

    /// Clear the sync log.
    func clearSyncLog() {
        syncLog.removeAll()
    }

    // MARK: - Auto-sync scheduling

    private func scheduleAutoSync() {
        cancelAutoSync()
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

    // MARK: - Formatting helpers

    var lastSyncDescription: String {
        guard let date = lastSyncDate else {
            return "Never"
        }

        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: Date())
    }
}
