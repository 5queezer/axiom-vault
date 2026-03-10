import SwiftUI

struct SettingsView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @EnvironmentObject var syncManager: SyncManager
    @AppStorage("autoLockMinutes") private var autoLockMinutes = 15
    @AppStorage("showInMenuBar") private var showInMenuBar = true

    var body: some View {
        TabView {
            generalSettings
                .tabItem {
                    Label("General", systemImage: "gear")
                }

            securitySettings
                .tabItem {
                    Label("Security", systemImage: "lock.shield")
                }

            syncSettings
                .tabItem {
                    Label("Sync", systemImage: "arrow.triangle.2.circlepath.icloud")
                }

            aboutView
                .tabItem {
                    Label("About", systemImage: "info.circle")
                }
        }
        .frame(width: 450, height: 300)
    }

    private var generalSettings: some View {
        Form {
            Toggle("Show in menu bar", isOn: $showInMenuBar)
        }
        .formStyle(.grouped)
        .padding()
    }

    private var securitySettings: some View {
        Form {
            Picker("Auto-lock after", selection: $autoLockMinutes) {
                Text("5 minutes").tag(5)
                Text("15 minutes").tag(15)
                Text("30 minutes").tag(30)
                Text("1 hour").tag(60)
                Text("Never").tag(0)
            }

            if BiometricAuth.shared.isBiometricAvailable, let vaultPath = vaultManager.lastUnlockedVaultPath {
                let isEnabled = BiometricAuth.shared.hasStoredPassword(for: vaultPath)
                Toggle(
                    "Unlock with \(BiometricAuth.shared.biometricName)",
                    isOn: Binding(
                        get: { isEnabled },
                        set: { enabled in
                            if !enabled {
                                vaultManager.disableBiometric(for: vaultPath)
                            }
                        }
                    )
                )
                .disabled(!isEnabled)
                if !isEnabled {
                    Text("Unlock with your password to enable \(BiometricAuth.shared.biometricName)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .formStyle(.grouped)
        .padding()
    }

    private var syncSettings: some View {
        Form {
            Toggle("Auto-sync", isOn: $syncManager.autoSyncEnabled)
                .disabled(!syncManager.isSyncAvailable)

            if syncManager.autoSyncEnabled {
                Picker("Sync interval", selection: $syncManager.syncInterval) {
                    ForEach(SyncInterval.allCases) { interval in
                        Text(interval.displayName).tag(interval)
                    }
                }
            }

            Picker("Conflict resolution", selection: $syncManager.conflictStrategy) {
                ForEach(ConflictResolutionStrategy.allCases) { strategy in
                    Text(strategy.displayName).tag(strategy)
                }
            }
        }
        .formStyle(.grouped)
        .padding()
    }

    private var aboutView: some View {
        VStack(spacing: 12) {
            Image(systemName: "lock.shield.fill")
                .font(.system(size: 48))
                .foregroundStyle(.blue)

            Text("AxiomVault")
                .font(.title2)
                .fontWeight(.bold)

            Text("Version \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown")")
                .foregroundStyle(.secondary)

            Text("Core: v\(VaultCore.shared.version())")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
