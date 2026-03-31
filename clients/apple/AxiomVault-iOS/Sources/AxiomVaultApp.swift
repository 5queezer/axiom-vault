import SwiftUI

@main
struct AxiomVaultApp: App {
    @StateObject private var vaultManager = VaultManager()
    @StateObject private var syncManager = SyncManager()
    @Environment(\.scenePhase) private var scenePhase

    init() {
        #if DEBUG
        if CommandLine.arguments.contains("--reset-state") {
            Self.resetTestState()
        }
        #endif

        do {
            try VaultCore.shared.initialize()
        } catch {
            print("Failed to initialize VaultCore: \(error)")
        }
    }

    #if DEBUG
    /// Remove test vaults so each UI test run starts from a clean state.
    private static func resetTestState() {
        let fm = FileManager.default
        guard let docs = fm.urls(for: .documentDirectory, in: .userDomainMask).first else { return }
        let vaultsDir = docs.appendingPathComponent("Vaults")
        try? fm.removeItem(at: vaultsDir)
    }
    #endif

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(vaultManager)
                .environmentObject(syncManager)
                .overlay(
                    PrivacyOverlayView(isActive: scenePhase != .active)
                )
        }
    }
}

/// A privacy overlay that obscures vault content when the app enters the
/// task switcher or moves to the background, preventing sensitive data
/// from appearing in app-switcher snapshots.
private struct PrivacyOverlayView: View {
    let isActive: Bool

    var body: some View {
        Group {
            if isActive {
                ZStack {
                    Color(.systemBackground)
                        .ignoresSafeArea()
                    VStack(spacing: 16) {
                        Image(systemName: "lock.shield.fill")
                            .font(.system(size: 48))
                            .foregroundColor(.secondary)
                        Text("Axiom Vault")
                            .font(.title2)
                            .fontWeight(.semibold)
                            .foregroundColor(.secondary)
                    }
                }
                .transition(.opacity)
            }
        }
        .animation(.easeInOut(duration: 0.15), value: isActive)
    }
}
