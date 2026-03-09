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

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                statusIcon
                    .font(.title2)

                VStack(alignment: .leading, spacing: 2) {
                    Text(syncManager.syncStatus.rawValue)
                        .font(.headline)

                    Text(syncManager.isSyncAvailable ? "Last sync: \(syncManager.lastSyncDescription)" : "Sync is not yet connected to the backend.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                Spacer()

                Button {
                    Task { await syncManager.sync() }
                } label: {
                    Label("Sync Now", systemImage: "arrow.triangle.2.circlepath")
                }
                .disabled(syncManager.isSyncing || !syncManager.isSyncAvailable)
                #if os(macOS)
                .buttonStyle(.bordered)
                #else
                .buttonStyle(.borderedProminent)
                .controlSize(.small)
                #endif
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
        .padding()
        #if os(macOS)
        .background(.background)
        #else
        .background(Color(.systemGroupedBackground))
        .cornerRadius(12)
        #endif
    }

    @ViewBuilder
    private var statusIcon: some View {
        if syncManager.isSyncing {
            ProgressView()
                .controlSize(.regular)
        } else {
            Image(systemName: syncManager.syncStatus.iconName)
                .foregroundStyle(syncManager.syncStatus.tintColor)
        }
    }
}
