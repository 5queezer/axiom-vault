import SwiftUI

/// A compact sync status indicator suitable for placement in toolbars.
///
/// Shows an icon representing the current sync state with a text label.
/// Tapping triggers a manual sync.
struct SyncStatusView: View {
    @EnvironmentObject var syncManager: SyncManager

    var body: some View {
        HStack(spacing: 6) {
            statusIcon
            Text(syncManager.syncStatus.rawValue)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private var statusIcon: some View {
        if syncManager.isSyncing {
            ProgressView()
                .controlSize(.small)
        } else {
            Image(systemName: syncManager.syncStatus.iconName)
                .foregroundStyle(syncManager.syncStatus.tintColor)
                .imageScale(.medium)
        }
    }
}

/// A more detailed sync status view with last-sync time, used in menus or sheets.
struct SyncStatusDetailView: View {
    @EnvironmentObject var syncManager: SyncManager
    @State private var isDismissed = false

    var body: some View {
        if !isDismissed {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    statusIcon
                        .font(.title3)

                    if syncManager.isSyncAvailable {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(syncManager.syncStatus.rawValue)
                                .font(.subheadline.weight(.medium))

                            Text("Last sync: \(syncManager.lastSyncDescription)")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    } else {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Cloud Sync Available")
                                .font(.subheadline.weight(.medium))

                            Text("Sync your vault across devices.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }

                    Spacer()

                    if syncManager.isSyncAvailable {
                        Button {
                            Task { await syncManager.sync() }
                        } label: {
                            Label("Sync Now", systemImage: "arrow.triangle.2.circlepath")
                        }
                        .disabled(syncManager.isSyncing)
                        #if os(macOS)
                        .buttonStyle(.bordered)
                        #else
                        .buttonStyle(.borderedProminent)
                        .controlSize(.small)
                        #endif
                    } else {
                        Button {
                            isDismissed = true
                        } label: {
                            Image(systemName: "xmark")
                                .font(.caption.weight(.medium))
                                .foregroundStyle(.secondary)
                        }
                        .buttonStyle(.plain)
                    }
                }

                if let error = syncManager.syncError {
                    HStack(spacing: 4) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .foregroundStyle(.yellow)
                            .imageScale(.small)
                        Text(error)
                            .font(.caption)
                            .foregroundStyle(.red)
                    }
                }
            }
            .padding(12)
            #if os(macOS)
            .background(.background)
            #else
            .background(Color(.secondarySystemGroupedBackground))
            .cornerRadius(12)
            #endif
        }
    }

    @ViewBuilder
    private var statusIcon: some View {
        if syncManager.isSyncing {
            ProgressView()
                .controlSize(.regular)
        } else if syncManager.isSyncAvailable {
            Image(systemName: syncManager.syncStatus.iconName)
                .foregroundStyle(syncManager.syncStatus.tintColor)
        } else {
            Image(systemName: "icloud")
                .foregroundStyle(.blue)
        }
    }
}
