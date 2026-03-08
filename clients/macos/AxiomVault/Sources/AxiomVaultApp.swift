import SwiftUI

@main
struct AxiomVaultApp: App {
    @StateObject private var vaultManager = VaultManager()

    init() {
        do {
            try VaultCore.shared.initialize()
        } catch {
            print("Failed to initialize VaultCore: \(error)")
        }
    }

    var body: some Scene {
        WindowGroup {
            MainView()
                .environmentObject(vaultManager)
                .frame(minWidth: 700, minHeight: 500)
        }
        .windowStyle(.titleBar)
        .windowToolbarStyle(.unified(showsTitle: true))
        .commands {
            CommandGroup(after: .newItem) {
                Button("Create New Vault...") {
                    NotificationCenter.default.post(name: .createVault, object: nil)
                }
                .keyboardShortcut("n", modifiers: [.command, .shift])

                Button("Open Vault...") {
                    NotificationCenter.default.post(name: .openVault, object: nil)
                }
                .keyboardShortcut("o", modifiers: .command)
            }

            CommandGroup(after: .sidebar) {
                Button("Lock Vault") {
                    vaultManager.closeVault()
                }
                .keyboardShortcut("l", modifiers: [.command, .shift])
                .disabled(!vaultManager.isVaultOpen)
            }
        }

        MenuBarExtra("AxiomVault", systemImage: vaultManager.isVaultOpen ? "lock.open.fill" : "lock.fill") {
            MenuBarView()
                .environmentObject(vaultManager)
        }

        Settings {
            SettingsView()
                .environmentObject(vaultManager)
        }
    }
}

extension Notification.Name {
    static let createVault = Notification.Name("com.axiomvault.createVault")
    static let openVault = Notification.Name("com.axiomvault.openVault")
}
