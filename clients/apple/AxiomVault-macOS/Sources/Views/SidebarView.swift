import SwiftUI

struct SidebarView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Binding var showCreateVault: Bool
    @Binding var showUnlockVault: Bool

    var body: some View {
        List {
            if vaultManager.isVaultOpen {
                Section("Active Vault") {
                    Label(
                        vaultManager.currentVaultName ?? "Vault",
                        systemImage: "lock.open.fill"
                    )
                    .foregroundStyle(.green)

                    if let info = vaultManager.vaultInfo {
                        LabeledContent("Files", value: "\(info.fileCount)")
                        LabeledContent("Size", value: ByteCountFormatter.string(
                            fromByteCount: info.totalSize,
                            countStyle: .file
                        ))
                    }

                    Button("Lock Vault", systemImage: "lock.fill") {
                        vaultManager.closeVault()
                    }
                    .foregroundStyle(.red)
                }
            }

            Section("Recent Vaults") {
                if vaultManager.vaultBookmarks.isEmpty {
                    Text("No recent vaults")
                        .foregroundStyle(.secondary)
                        .font(.caption)
                } else {
                    ForEach(vaultManager.vaultBookmarks) { bookmark in
                        HStack {
                            Label(bookmark.name, systemImage: "lock.fill")
                            Spacer()
                            if let date = bookmark.lastOpened {
                                Text(date, style: .relative)
                                    .font(.caption2)
                                    .foregroundStyle(.tertiary)
                            }
                        }
                        .contextMenu {
                            Button("Remove from Recents") {
                                vaultManager.removeBookmark(bookmark)
                            }
                        }
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom) {
            HStack(spacing: 8) {
                Button {
                    showCreateVault = true
                } label: {
                    Image(systemName: "plus")
                }

                Button {
                    showUnlockVault = true
                } label: {
                    Image(systemName: "folder")
                }

                Spacer()
            }
            .padding(12)
            .buttonStyle(.borderless)
        }
        .frame(minWidth: 200)
    }
}
