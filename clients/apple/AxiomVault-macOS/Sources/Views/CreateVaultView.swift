import SwiftUI

struct CreateVaultView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Environment(\.dismiss) private var dismiss
    @State private var vaultName = ""
    @State private var password = ""
    @State private var confirmPassword = ""
    @State private var selectedLocation: URL?
    @State private var showPassword = false

    private var isValid: Bool {
        !vaultName.isEmpty && password.count >= 8 && password == confirmPassword && selectedLocation != nil
    }

    var body: some View {
        VStack(spacing: 20) {
            Text("Create New Vault")
                .font(.title2)
                .fontWeight(.semibold)

            Form {
                TextField("Vault Name", text: $vaultName)
                    .textFieldStyle(.roundedBorder)

                HStack {
                    Text(selectedLocation?.path ?? "No location selected")
                        .foregroundStyle(selectedLocation == nil ? .secondary : .primary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    Button("Choose...") { chooseLocation() }
                }

                Divider()

                Group {
                    if showPassword {
                        TextField("Password", text: $password)
                        TextField("Confirm Password", text: $confirmPassword)
                    } else {
                        SecureField("Password", text: $password)
                        SecureField("Confirm Password", text: $confirmPassword)
                    }
                }
                .textFieldStyle(.roundedBorder)

                Toggle("Show password", isOn: $showPassword)
                    .toggleStyle(.checkbox)
            }
            .frame(width: 400)

            if !password.isEmpty {
                HStack(spacing: 4) {
                    if password.count < 8 {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(.red)
                        Text("At least 8 characters").foregroundStyle(.red)
                    } else if password != confirmPassword && !confirmPassword.isEmpty {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(.red)
                        Text("Passwords do not match").foregroundStyle(.red)
                    } else if password.count >= 8 && (confirmPassword.isEmpty || password == confirmPassword) {
                        Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                        Text("Password meets requirements").foregroundStyle(.green)
                    }
                }
                .font(.caption)
            }

            HStack {
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)

                Button("Create Vault") {
                    guard let location = selectedLocation else { return }
                    let vaultURL = location.appendingPathComponent(vaultName)
                    Task {
                        await vaultManager.createVault(at: vaultURL, password: password)
                        if vaultManager.isVaultOpen {
                            dismiss()
                        }
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!isValid || vaultManager.isLoading)
            }
        }
        .padding(24)
    }

    private func chooseLocation() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.message = "Choose where to create the vault"
        panel.prompt = "Select"

        if panel.runModal() == .OK {
            selectedLocation = panel.url
        }
    }
}
