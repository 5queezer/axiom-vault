import SwiftUI

@main
struct AxiomVaultApp: App {
    @StateObject private var vaultManager = VaultManager()

    init() {
        // Initialize the Rust FFI layer
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
        }
    }
}
