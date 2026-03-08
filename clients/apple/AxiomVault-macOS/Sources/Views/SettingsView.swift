import SwiftUI

struct SettingsView: View {
    @EnvironmentObject var vaultManager: VaultManager
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

            aboutView
                .tabItem {
                    Label("About", systemImage: "info.circle")
                }
        }
        .frame(width: 450, height: 250)
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
                Toggle(
                    "Unlock with \(BiometricAuth.shared.biometricName)",
                    isOn: Binding(
                        get: { BiometricAuth.shared.hasStoredPassword(for: vaultPath) },
                        set: { enabled in
                            if !enabled {
                                vaultManager.disableBiometric(for: vaultPath)
                            }
                            // Enabling requires the password, so it's done at unlock time
                        }
                    )
                )
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

            Text("Version 0.1.0")
                .foregroundStyle(.secondary)

            Text("Core: v\(VaultCore.shared.version())")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
