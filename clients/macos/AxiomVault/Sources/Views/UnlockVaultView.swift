import SwiftUI

struct UnlockVaultView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Environment(\.dismiss) private var dismiss
    @State private var selectedURL: URL?
    @State private var password = ""
    @State private var showPassword = false

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
                dismiss()
            }
        }
    }
}
