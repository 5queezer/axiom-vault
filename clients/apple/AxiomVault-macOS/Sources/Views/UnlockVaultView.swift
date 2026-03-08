import SwiftUI

struct UnlockVaultView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Environment(\.dismiss) private var dismiss
    @State private var selectedURL: URL?
    @State private var password = ""
    @State private var showPassword = false
    @State private var showBiometricSavePrompt = false
    @State private var pendingPassword = ""

    private let biometric = BiometricAuth.shared

    /// Whether the selected vault has a stored biometric credential
    var canUseBiometric: Bool {
        guard let url = selectedURL else { return false }
        return biometric.isBiometricAvailable
            && biometric.hasStoredPassword(for: url.path)
    }

    var body: some View {
        VStack(spacing: 20) {
            Text("Open Vault")
                .font(.title2)
                .fontWeight(.semibold)

            // Recent vaults
            if !vaultManager.vaultBookmarks.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Recent Vaults")
                        .font(.headline)
                        .foregroundStyle(.secondary)

                    ForEach(vaultManager.vaultBookmarks) { bookmark in
                        Button {
                            selectedURL = bookmark.url
                        } label: {
                            HStack {
                                Image(systemName: "lock.fill")
                                    .foregroundStyle(.secondary)
                                Text(bookmark.name)
                                Spacer()
                                if selectedURL?.path == bookmark.url?.path {
                                    Image(systemName: "checkmark")
                                        .foregroundStyle(.blue)
                                }
                            }
                            .padding(.vertical, 4)
                            .padding(.horizontal, 8)
                            .background(
                                selectedURL?.path == bookmark.url?.path
                                    ? Color.accentColor.opacity(0.1)
                                    : Color.clear,
                                in: RoundedRectangle(cornerRadius: 4)
                            )
                        }
                        .buttonStyle(.plain)
                    }
                }
                .frame(width: 400)

                Divider()
            }

            // Browse for vault
            HStack {
                Text(selectedURL?.path ?? "No vault selected")
                    .foregroundStyle(selectedURL == nil ? .secondary : .primary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer()
                Button("Browse...") { browseForVault() }
            }
            .frame(width: 400)

            // Biometric unlock button
            if canUseBiometric {
                Button(action: unlockWithBiometric) {
                    HStack {
                        Image(systemName: biometric.unlockButtonIcon)
                        Text(biometric.unlockButtonLabel)
                    }
                    .frame(width: 400)
                }
                .controlSize(.large)
                .disabled(vaultManager.isLoading)
            }

            // Password
            Group {
                if showPassword {
                    TextField("Password", text: $password)
                } else {
                    SecureField("Password", text: $password)
                }
            }
            .textFieldStyle(.roundedBorder)
            .frame(width: 400)
            .onSubmit { unlock() }

            Toggle("Show password", isOn: $showPassword)
                .toggleStyle(.checkbox)

            HStack {
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)

                Button("Unlock") { unlock() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(selectedURL == nil || password.isEmpty || vaultManager.isLoading)
            }
        }
        .padding(24)
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

    private func browseForVault() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.message = "Select a vault folder"
        panel.prompt = "Open"

        if panel.runModal() == .OK {
            selectedURL = panel.url
        }
    }

    private func unlock() {
        guard let url = selectedURL, !password.isEmpty else { return }
        Task {
            await vaultManager.openVault(at: url, password: password)
            if vaultManager.isVaultOpen {
                vaultManager.lastUnlockedVaultPath = url.path
                // Offer biometric save if biometrics available but not yet stored
                if biometric.isBiometricAvailable
                    && !biometric.hasStoredPassword(for: url.path)
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
        guard let url = selectedURL else { return }

        Task {
            do {
                guard let storedPassword = try await biometric.retrievePassword(for: url.path) else {
                    vaultManager.errorMessage = "No stored password found. Please enter your password."
                    return
                }
                await vaultManager.openVault(at: url, password: storedPassword)
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
