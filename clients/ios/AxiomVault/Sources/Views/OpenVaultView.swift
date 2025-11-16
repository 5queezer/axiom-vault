import SwiftUI

struct OpenVaultView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Environment(\.dismiss) var dismiss

    @State private var selectedVault: URL?
    @State private var password = ""
    @State private var showPassword = false

    var existingVaults: [URL] {
        vaultManager.listExistingVaults()
    }

    var isFormValid: Bool {
        selectedVault != nil && !password.isEmpty
    }

    var body: some View {
        NavigationView {
            Form {
                Section(header: Text("Select Vault")) {
                    if existingVaults.isEmpty {
                        Text("No vaults found")
                            .foregroundColor(.secondary)
                    } else {
                        ForEach(existingVaults, id: \.path) { vault in
                            HStack {
                                Image(systemName: "lock.fill")
                                    .foregroundColor(selectedVault == vault ? .blue : .gray)

                                Text(vault.lastPathComponent)

                                Spacer()

                                if selectedVault == vault {
                                    Image(systemName: "checkmark")
                                        .foregroundColor(.blue)
                                }
                            }
                            .contentShape(Rectangle())
                            .onTapGesture {
                                selectedVault = vault
                            }
                        }
                    }
                }

                Section(header: Text("Password")) {
                    if showPassword {
                        TextField("Password", text: $password)
                            .autocapitalization(.none)
                    } else {
                        SecureField("Password", text: $password)
                    }

                    Toggle("Show Password", isOn: $showPassword)
                }

                Section {
                    Button(action: openVault) {
                        if vaultManager.isLoading {
                            ProgressView()
                                .frame(maxWidth: .infinity)
                        } else {
                            Text("Open Vault")
                                .frame(maxWidth: .infinity)
                        }
                    }
                    .disabled(!isFormValid || vaultManager.isLoading)
                }
            }
            .navigationTitle("Open Vault")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        dismiss()
                    }
                }
            }
        }
    }

    private func openVault() {
        guard let vault = selectedVault else { return }

        Task {
            await vaultManager.openVault(at: vault.path, password: password)
            if vaultManager.isVaultOpen {
                dismiss()
            }
        }
    }
}

#Preview {
    OpenVaultView()
        .environmentObject(VaultManager())
}
