import SwiftUI

struct MenuBarView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        if vaultManager.isVaultOpen {
            Label(vaultManager.currentVaultName ?? "Vault", systemImage: "lock.open.fill")

            if let info = vaultManager.vaultInfo {
                Text("\(info.fileCount) files, \(ByteCountFormatter.string(fromByteCount: info.totalSize, countStyle: .file))")
            }

            Divider()

            Button("Lock Vault") {
                vaultManager.closeVault()
            }
            .keyboardShortcut("l")
        } else {
            Text("No vault open")
                .foregroundStyle(.secondary)

            Divider()

            Button("Open AxiomVault") {
                NSApplication.shared.activate(ignoringOtherApps: true)
                if let window = NSApplication.shared.windows.first {
                    window.makeKeyAndOrderFront(nil)
                }
            }
        }

        Divider()

        Button("Quit AxiomVault") {
            vaultManager.closeVault()
            NSApplication.shared.terminate(nil)
        }
        .keyboardShortcut("q")
    }
}
