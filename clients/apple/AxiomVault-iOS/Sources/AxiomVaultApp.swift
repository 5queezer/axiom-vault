import SwiftUI

@main
struct AxiomVaultApp: App {
    @StateObject private var vaultManager = VaultManager()
    @StateObject private var syncManager = SyncManager()

    init() {
        do {
            try VaultCore.shared.initialize()
        } catch {
            print("Failed to initialize VaultCore: \(error)")
        }
    }

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(vaultManager)
                .environmentObject(syncManager)
        }
    }
}
