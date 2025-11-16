import SwiftUI

struct CreateVaultView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Environment(\.dismiss) var dismiss

    @State private var vaultName = ""
    @State private var password = ""
    @State private var confirmPassword = ""
    @State private var showPassword = false

    var passwordsMatch: Bool {
        !password.isEmpty && password == confirmPassword
    }

    var isFormValid: Bool {
        !vaultName.isEmpty && passwordsMatch && password.count >= 8
    }

    var body: some View {
        NavigationView {
            Form {
                Section(header: Text("Vault Name")) {
                    TextField("My Vault", text: $vaultName)
                        .autocapitalization(.none)
                        .disableAutocorrection(true)
                }

                Section(header: Text("Password"), footer: passwordFooter) {
                    if showPassword {
                        TextField("Password", text: $password)
                            .autocapitalization(.none)
                        TextField("Confirm Password", text: $confirmPassword)
                            .autocapitalization(.none)
                    } else {
                        SecureField("Password", text: $password)
                        SecureField("Confirm Password", text: $confirmPassword)
                    }

                    Toggle("Show Password", isOn: $showPassword)
                }

                Section {
                    Button(action: createVault) {
                        if vaultManager.isLoading {
                            ProgressView()
                                .frame(maxWidth: .infinity)
                        } else {
                            Text("Create Vault")
                                .frame(maxWidth: .infinity)
                        }
                    }
                    .disabled(!isFormValid || vaultManager.isLoading)
                }
            }
            .navigationTitle("Create New Vault")
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

    private var passwordFooter: some View {
        VStack(alignment: .leading, spacing: 4) {
            if password.isEmpty {
                Text("Password must be at least 8 characters")
                    .foregroundColor(.secondary)
            } else if password.count < 8 {
                Text("Password too short (\(password.count)/8)")
                    .foregroundColor(.red)
            } else if !confirmPassword.isEmpty && !passwordsMatch {
                Text("Passwords do not match")
                    .foregroundColor(.red)
            } else if passwordsMatch {
                Text("Passwords match")
                    .foregroundColor(.green)
            }
        }
    }

    private func createVault() {
        Task {
            await vaultManager.createVault(name: vaultName, password: password)
            if vaultManager.isVaultOpen {
                dismiss()
            }
        }
    }
}

#Preview {
    CreateVaultView()
        .environmentObject(VaultManager())
}
