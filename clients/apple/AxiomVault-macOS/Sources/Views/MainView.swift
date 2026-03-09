import SwiftUI

struct MainView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @EnvironmentObject var syncManager: SyncManager
    @State private var showCreateVault = false
    @State private var showUnlockVault = false

    var body: some View {
        NavigationSplitView {
            SidebarView(
                showCreateVault: $showCreateVault,
                showUnlockVault: $showUnlockVault
            )
        } detail: {
            if vaultManager.isVaultOpen {
                VaultBrowserView()
            } else {
                WelcomeView(
                    showCreateVault: $showCreateVault,
                    showUnlockVault: $showUnlockVault
                )
            }
        }
        .sheet(isPresented: $showCreateVault) {
            CreateVaultView()
        }
        .sheet(isPresented: $showUnlockVault) {
            UnlockVaultView()
        }
        .alert("Error", isPresented: .init(
            get: { vaultManager.errorMessage != nil },
            set: { if !$0 { vaultManager.errorMessage = nil } }
        )) {
            Button("OK") { vaultManager.errorMessage = nil }
        } message: {
            Text(vaultManager.errorMessage ?? "")
        }
        .onAppear {
            syncManager.setActiveVault(vaultManager.vaultInfo?.vaultId)
        }
        .onChange(of: vaultManager.vaultInfo?.vaultId) { _, newValue in
            syncManager.setActiveVault(newValue)
        }
        .onReceive(NotificationCenter.default.publisher(for: .createVault)) { _ in
            showCreateVault = true
        }
        .onReceive(NotificationCenter.default.publisher(for: .openVault)) { _ in
            showUnlockVault = true
        }
    }
}

struct WelcomeView: View {
    @Binding var showCreateVault: Bool
    @Binding var showUnlockVault: Bool

    var body: some View {
        VStack(spacing: 24) {
            Image(systemName: "lock.shield.fill")
                .font(.system(size: 64))
                .foregroundStyle(.secondary)

            Text("AxiomVault")
                .font(.largeTitle)
                .fontWeight(.bold)

            Text("Encrypted file storage with transparent Finder integration")
                .font(.title3)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)

            HStack(spacing: 16) {
                Button {
                    showCreateVault = true
                } label: {
                    Label("Create Vault", systemImage: "plus.circle.fill")
                        .frame(width: 160)
                }
                .controlSize(.large)
                .buttonStyle(.borderedProminent)

                Button {
                    showUnlockVault = true
                } label: {
                    Label("Open Vault", systemImage: "lock.open.fill")
                        .frame(width: 160)
                }
                .controlSize(.large)
                .buttonStyle(.bordered)
            }

            Text("v\(VaultCore.shared.version())")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
