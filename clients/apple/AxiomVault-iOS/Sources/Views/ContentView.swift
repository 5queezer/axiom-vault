import SwiftUI

struct ContentView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @EnvironmentObject var syncManager: SyncManager
    @State private var showingCreateVault = false
    @State private var showingOpenVault = false

    var body: some View {
        NavigationView {
            Group {
                if vaultManager.isVaultOpen {
                    VaultBrowserView()
                } else {
                    VaultSelectionView(
                        showingCreateVault: $showingCreateVault,
                        showingOpenVault: $showingOpenVault
                    )
                }
            }
            .navigationTitle(vaultManager.isVaultOpen ? "Vault" : "AxiomVault")
            .toolbar {
                if vaultManager.isVaultOpen {
                    ToolbarItem(placement: .navigationBarTrailing) {
                        Button("Close") {
                            vaultManager.closeVault()
                        }
                    }
                }
            }
        }
        .sheet(isPresented: $showingCreateVault) {
            CreateVaultView()
        }
        .sheet(isPresented: $showingOpenVault) {
            OpenVaultView()
        }
        .onAppear {
            syncManager.setActiveVault(vaultManager.vaultInfo?.vaultId)
        }
        .onChange(of: vaultManager.vaultInfo?.vaultId) { newValue in
            syncManager.setActiveVault(newValue)
        }
        .alert("Error", isPresented: .constant(vaultManager.errorMessage != nil)) {
            Button("OK") {
                vaultManager.errorMessage = nil
            }
        } message: {
            Text(vaultManager.errorMessage ?? "")
        }
    }
}

struct VaultSelectionView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Binding var showingCreateVault: Bool
    @Binding var showingOpenVault: Bool

    var body: some View {
        VStack(spacing: 24) {
            Spacer()

            Image(systemName: "lock.shield.fill")
                .font(.system(size: 72))
                .foregroundStyle(.blue.gradient)

            Text("Secure, encrypted file storage")
                .font(.subheadline)
                .foregroundColor(.secondary)

            VStack(spacing: 12) {
                Button(action: {
                    showingOpenVault = true
                }) {
                    Label("Open Existing Vault", systemImage: "folder.fill")
                        .font(.headline)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 14)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)

                Button(action: {
                    showingCreateVault = true
                }) {
                    Label("Create New Vault", systemImage: "plus.circle.fill")
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 14)
                }
                .buttonStyle(.bordered)
                .controlSize(.large)
            }
            .padding(.horizontal, 40)

            if !vaultManager.listExistingVaults().isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Recent Vaults")
                        .font(.headline)
                        .padding(.horizontal)

                    ForEach(vaultManager.listExistingVaults(), id: \.path) { url in
                        HStack {
                            Image(systemName: "lock.shield")
                                .foregroundColor(.blue)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(url.lastPathComponent)
                                    .font(.body)
                                Text(url.deletingLastPathComponent().path)
                                    .font(.caption2)
                                    .foregroundColor(.secondary)
                                    .lineLimit(1)
                            }
                            Spacer()
                            Image(systemName: "chevron.right")
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                        .padding(12)
                        .background(Color(.systemGray6))
                        .cornerRadius(10)
                        .onTapGesture {
                            showingOpenVault = true
                        }
                    }
                }
                .padding(.horizontal)
            }

            Spacer()
        }
    }
}
