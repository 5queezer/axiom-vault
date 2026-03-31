import SwiftUI

/// Settings view for configuring cloud sync behavior.
///
/// Provides controls for auto-sync toggle, interval selection, and
/// conflict resolution strategy. Also shows the sync history log.
struct SyncSettingsView: View {
    @EnvironmentObject var syncManager: SyncManager
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        #if os(macOS)
        macOSLayout
        #else
        iOSLayout
        #endif
    }

    // MARK: - macOS layout

    #if os(macOS)
    private var macOSLayout: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 16) {
                Text("Sync Settings")
                    .font(.headline)

                syncSettingsForm

                Divider()

                syncHistorySection

                HStack {
                    Spacer()
                    Button("Done") { dismiss() }
                        .keyboardShortcut(.defaultAction)
                }
            }
            .padding(24)
        }
        .frame(minWidth: 480, minHeight: 400)
    }
    #endif

    // MARK: - iOS layout

    #if os(iOS)
    private var iOSLayout: some View {
        NavigationView {
            Form {
                syncProviderSection
                syncStatusSection
                syncConfigSection
                conflictSection
                syncHistoryFormSection
            }
            .navigationTitle("Sync Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }

    private var syncProviderSection: some View {
        Section {
            ForEach(SyncProvider.allCases) { provider in
                HStack {
                    Image(systemName: provider.iconName)
                        .foregroundStyle(provider == .none ? Color.secondary : Color.blue)
                        .frame(width: 24)

                    VStack(alignment: .leading, spacing: 2) {
                        Text(provider.displayName)
                        Text(provider.description)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }

                    Spacer()

                    if syncManager.syncProvider == provider {
                        Image(systemName: "checkmark")
                            .foregroundStyle(.blue)
                    }
                }
                .contentShape(Rectangle())
                .accessibilityIdentifier("syncProvider_\(provider.rawValue)")
                .onTapGesture {
                    syncManager.syncProvider = provider
                }
            }
        } header: {
            Text("Sync Provider")
        } footer: {
            if syncManager.syncProvider.needsSetup {
                Text("\(syncManager.syncProvider.displayName) requires additional setup. Configuration will be available in a future update.")
            }
        }
    }

    private var syncStatusSection: some View {
        Section {
            HStack {
                if syncManager.isSyncing {
                    ProgressView()
                        .padding(.trailing, 4)
                } else {
                    Image(systemName: syncManager.syncStatus.iconName)
                        .foregroundStyle(syncManager.syncStatus.tintColor)
                }
                Text(syncManager.syncStatus.rawValue)

                Spacer()

                Button("Sync Now") {
                    Task { await syncManager.sync() }
                }
                .disabled(syncManager.isSyncing || !syncManager.isSyncAvailable)
            }

            LabeledContent("Last Sync", value: syncManager.lastSyncDescription)
        } header: {
            Text("Status")
        }
    }

    private var syncConfigSection: some View {
        Section {
            Toggle("Auto-Sync", isOn: $syncManager.autoSyncEnabled)
                .disabled(!syncManager.isSyncAvailable)

            if syncManager.autoSyncEnabled {
                Picker("Sync Interval", selection: $syncManager.syncInterval) {
                    ForEach(SyncInterval.allCases) { interval in
                        Text(interval.displayName).tag(interval)
                    }
                }
            }
        } header: {
            Text("Auto-Sync")
        } footer: {
            Text(syncManager.availabilityMessage)
        }
    }

    private var conflictSection: some View {
        Section {
            Picker("Conflict Resolution", selection: $syncManager.conflictStrategy) {
                ForEach(ConflictResolutionStrategy.allCases) { strategy in
                    VStack(alignment: .leading) {
                        Text(strategy.displayName)
                    }
                    .tag(strategy)
                }
            }
        } header: {
            Text("Conflicts")
        } footer: {
            Text(syncManager.conflictStrategy.description)
        }
    }

    private var syncHistoryFormSection: some View {
        Section {
            if syncManager.syncLog.isEmpty {
                Text("No sync activity yet")
                    .foregroundStyle(.secondary)
            } else {
                ForEach(syncManager.syncLog.prefix(10)) { entry in
                    syncLogRow(entry)
                }

                if syncManager.syncLog.count > 0 {
                    Button("Clear Log", role: .destructive) {
                        syncManager.clearSyncLog()
                    }
                }
            }
        } header: {
            Text("Sync History")
        }
    }
    #endif

    // MARK: - Shared components

    private var syncSettingsForm: some View {
        Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 12) {
            GridRow {
                Text("Provider")
                    .foregroundStyle(.secondary)
                Picker("", selection: $syncManager.syncProvider) {
                    ForEach(SyncProvider.allCases) { provider in
                        Label(provider.displayName, systemImage: provider.iconName)
                            .tag(provider)
                    }
                }
                .labelsHidden()
                .frame(maxWidth: 200)
            }

            if syncManager.syncProvider.needsSetup {
                GridRow {
                    Color.clear
                        .gridCellUnsizedAxes([.horizontal, .vertical])
                    Text("\(syncManager.syncProvider.displayName) requires additional setup. Configuration will be available in a future update.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

            Divider()
                .gridCellColumns(2)

            GridRow {
                Text("Status")
                    .foregroundStyle(.secondary)
                HStack(spacing: 6) {
                    if syncManager.isSyncing {
                        ProgressView()
                            .controlSize(.small)
                    } else {
                        Image(systemName: syncManager.syncStatus.iconName)
                            .foregroundStyle(syncManager.syncStatus.tintColor)
                    }
                    Text(syncManager.syncStatus.rawValue)

                    Spacer()

                    Button("Sync Now") {
                        Task { await syncManager.sync() }
                    }
                    .disabled(syncManager.isSyncing || !syncManager.isSyncAvailable)
                }
            }

            GridRow {
                Text("Last Sync")
                    .foregroundStyle(.secondary)
                Text(syncManager.lastSyncDescription)
            }

            GridRow {
                Text("Availability")
                    .foregroundStyle(.secondary)
                Text(syncManager.availabilityMessage)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Divider()
                .gridCellColumns(2)

            GridRow {
                Text("Auto-Sync")
                    .foregroundStyle(.secondary)
                Toggle("", isOn: $syncManager.autoSyncEnabled)
                    .labelsHidden()
                    .disabled(!syncManager.isSyncAvailable)
            }

            if syncManager.autoSyncEnabled {
                GridRow {
                    Text("Interval")
                        .foregroundStyle(.secondary)
                    Picker("", selection: $syncManager.syncInterval) {
                        ForEach(SyncInterval.allCases) { interval in
                            Text(interval.displayName).tag(interval)
                        }
                    }
                    .labelsHidden()
                    .frame(maxWidth: 200)
                }
            }

            Divider()
                .gridCellColumns(2)

            GridRow {
                Text("Conflicts")
                    .foregroundStyle(.secondary)
                Picker("", selection: $syncManager.conflictStrategy) {
                    ForEach(ConflictResolutionStrategy.allCases) { strategy in
                        Text(strategy.displayName).tag(strategy)
                    }
                }
                .labelsHidden()
                .frame(maxWidth: 200)
            }

            GridRow {
                Color.clear
                    .gridCellUnsizedAxes([.horizontal, .vertical])
                Text(syncManager.conflictStrategy.description)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var syncHistorySection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Sync History")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                Spacer()
                if !syncManager.syncLog.isEmpty {
                    Button("Clear") {
                        syncManager.clearSyncLog()
                    }
                    .font(.caption)
                }
            }

            if syncManager.syncLog.isEmpty {
                Text("No sync activity yet")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.vertical, 12)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 4) {
                        ForEach(syncManager.syncLog) { entry in
                            syncLogRow(entry)
                        }
                    }
                }
                .frame(maxHeight: 150)
            }
        }
    }

    private func syncLogRow(_ entry: SyncLogEntry) -> some View {
        HStack(spacing: 8) {
            Image(systemName: entry.status.iconName)
                .foregroundStyle(entry.status.tintColor)
                .imageScale(.small)
                .frame(width: 16)

            Text(entry.message)
                .font(.caption)
                .lineLimit(1)

            Spacer()

            if entry.filesChanged > 0 {
                Text("\(entry.filesChanged) files")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }

            Text(entry.date, style: .time)
                .font(.caption2)
                .foregroundStyle(.tertiary)
        }
        .padding(.vertical, 2)
    }
}
