import SwiftUI

struct ContentView: View {
    @EnvironmentObject var vaultManager: VaultManager
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
        VStack(spacing: 30) {
            Image(systemName: "lock.shield.fill")
                .font(.system(size: 80))
                .foregroundColor(.blue)

            Text("AxiomVault")
                .font(.largeTitle)
                .fontWeight(.bold)

            Text("Secure, encrypted file storage")
                .font(.subheadline)
                .foregroundColor(.secondary)

            Text("Version: \(VaultCore.shared.version())")
                .font(.caption)
                .foregroundColor(.secondary)

            VStack(spacing: 16) {
                Button(action: {
                    showingCreateVault = true
                }) {
                    Label("Create New Vault", systemImage: "plus.circle.fill")
                        .frame(maxWidth: .infinity)
                        .padding()
                        .background(Color.blue)
                        .foregroundColor(.white)
                        .cornerRadius(10)
                }

                Button(action: {
                    showingOpenVault = true
                }) {
                    Label("Open Existing Vault", systemImage: "folder.fill")
                        .frame(maxWidth: .infinity)
                        .padding()
                        .background(Color.green)
                        .foregroundColor(.white)
                        .cornerRadius(10)
                }
            }
            .padding(.horizontal, 40)

            if !vaultManager.listExistingVaults().isEmpty {
                VStack(alignment: .leading) {
                    Text("Recent Vaults")
                        .font(.headline)
                        .padding(.horizontal)

                    ForEach(vaultManager.listExistingVaults(), id: \.path) { url in
                        HStack {
                            Image(systemName: "lock.fill")
                                .foregroundColor(.gray)
                            Text(url.lastPathComponent)
                            Spacer()
                        }
                        .padding()
                        .background(Color(.systemGray6))
                        .cornerRadius(8)
                        .onTapGesture {
                            showingOpenVault = true
                        }
                    }
                }
                .padding(.horizontal)
            }

            Spacer()
        }
        .padding(.top, 50)
    }
}

#Preview {
    ContentView()
        .environmentObject(VaultManager())
}
