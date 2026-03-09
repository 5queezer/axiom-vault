import SwiftUI

struct OpenVaultView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Environment(\.dismiss) var dismiss

    @State private var selectedVault: URL?
    @State private var password = ""
    @State private var showPassword = false
    @State private var showBiometricSavePrompt = false
    @State private var pendingPassword = ""

    private let biometric = BiometricAuth.shared

    var existingVaults: [URL] {
        vaultManager.listExistingVaults()
    }

    var isFormValid: Bool {
        selectedVault != nil && !password.isEmpty
    }

    /// Whether the selected vault has a stored biometric credential
    var canUseBiometric: Bool {
        guard let vault = selectedVault else { return false }
        return biometric.isBiometricAvailable
            && biometric.hasStoredPassword(for: vault.path)
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

                if canUseBiometric {
                    Section {
                        Button(action: unlockWithBiometric) {
                            HStack {
                                Image(systemName: biometric.unlockButtonIcon)
                                Text(biometric.unlockButtonLabel)
                            }
                            .frame(maxWidth: .infinity)
                        }
                        .disabled(vaultManager.isLoading)
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
            .onDisappear {
                pendingPassword = ""
            }
            .alert(
                "Enable \(biometric.biometricName)?",
                isPresented: $showBiometricSavePrompt
            ) {
                Button("Enable") {
                    vaultManager.enableBiometric(
                        password: pendingPassword,
                        vaultPath: vaultManager.lastUnlockedVaultPath ?? ""
                    )
                    pendingPassword = ""
                    dismiss()
                }
                Button("Not Now", role: .cancel) {
                    pendingPassword = ""
                    dismiss()
                }
            } message: {
                Text("Unlock this vault with \(biometric.biometricName) next time?")
            }
        }
    }

    private func openVault() {
        guard let vault = selectedVault else { return }

        Task {
            await vaultManager.openVault(at: vault.path, password: password)
            if vaultManager.isVaultOpen {
                vaultManager.lastUnlockedVaultPath = vault.path
                // Offer biometric save if biometrics available but not yet stored
                if biometric.isBiometricAvailable
                    && !biometric.hasStoredPassword(for: vault.path)
                {
                    pendingPassword = password
                    password = ""
                    showBiometricSavePrompt = true
                } else {
                    dismiss()
                }
            }
        }
    }

    private func unlockWithBiometric() {
        guard let vault = selectedVault else { return }

        Task {
            do {
                guard let storedPassword = try await biometric.retrievePassword(for: vault.path) else {
                    vaultManager.errorMessage = "No stored password found. Please enter your password."
                    return
                }
                await vaultManager.openVault(at: vault.path, password: storedPassword)
                if vaultManager.isVaultOpen {
                    dismiss()
                }
            } catch {
                // Biometric failed — user can fall back to password entry
                vaultManager.errorMessage = "Biometric authentication failed. Please enter your password."
            }
        }
    }
}
