import SwiftUI
import UniformTypeIdentifiers

struct VaultBrowserView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @State private var showNewFolder = false
    @State private var newFolderName = ""
    @State private var showVaultInfo = false
    @State private var showChangePassword = false
    @State private var selectedEntries: Set<UUID> = []
    @State private var sortOrder = [KeyPathComparator(\VaultEntry.name)]
    @State private var isDragTargeted = false

    var body: some View {
        VStack(spacing: 0) {
            // Breadcrumb path bar
            BreadcrumbBar(pathStack: vaultManager.pathStack) { index in
                Task { await vaultManager.navigateToStackIndex(index) }
            }

            Divider()

            // File list
            if vaultManager.entries.isEmpty && !vaultManager.isLoading {
                emptyState
            } else {
                fileTable
            }
        }
        .toolbar {
            ToolbarItemGroup {
                Button("Add Files", systemImage: "doc.badge.plus") {
                    addFiles()
                }

                Button("Add Folder", systemImage: "folder.badge.plus") {
                    addFolder()
                }

                Button("New Folder", systemImage: "folder.badge.gearshape") {
                    showNewFolder = true
                }

                Button("Refresh", systemImage: "arrow.clockwise") {
                    Task { await vaultManager.refreshState() }
                }

                Menu {
                    Button("Vault Info", systemImage: "info.circle") {
                        showVaultInfo = true
                    }
                    Button("Change Password", systemImage: "key") {
                        showChangePassword = true
                    }
                    Divider()
                    Picker("Auto-Lock", selection: $vaultManager.autoLockDuration) {
                        ForEach(AutoLockDuration.allCases, id: \.self) { duration in
                            Text(duration.displayName).tag(duration)
                        }
                    }
                    Divider()
                    Button("Lock Vault", systemImage: "lock.fill") {
                        vaultManager.closeVault()
                    }
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
            }
        }
        .sheet(isPresented: $showNewFolder) {
            NewFolderSheet(name: $newFolderName) {
                Task {
                    await vaultManager.createDirectory(name: newFolderName)
                    newFolderName = ""
                }
            }
        }
        .sheet(isPresented: $showVaultInfo) {
            VaultInfoSheet()
        }
        .sheet(isPresented: $showChangePassword) {
            ChangePasswordSheet()
        }
        .onDrop(of: [.fileURL], isTargeted: $isDragTargeted) { providers in
            handleDrop(providers)
            return true
        }
        .overlay {
            if isDragTargeted {
                RoundedRectangle(cornerRadius: 8)
                    .strokeBorder(Color.accentColor, style: StrokeStyle(lineWidth: 3, dash: [8]))
                    .background(Color.accentColor.opacity(0.1))
                    .allowsHitTesting(false)
            }
        }
        .onAppear {
            Task { await vaultManager.refreshEntries() }
        }
    }

    private func handleDrop(_ providers: [NSItemProvider]) {
        var urls: [URL] = []
        let group = DispatchGroup()

        for provider in providers {
            group.enter()
            provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier, options: nil) { item, _ in
                defer { group.leave() }
                guard let data = item as? Data,
                      let url = URL(dataRepresentation: data, relativeTo: nil)
                else { return }
                urls.append(url)
            }
        }

        group.notify(queue: .main) {
            guard !urls.isEmpty else { return }
            Task { await vaultManager.addFiles(from: urls) }
        }
    }

    private var emptyState: some View {
        VStack(spacing: 16) {
            Image(systemName: "folder")
                .font(.system(size: 48))
                .foregroundStyle(.secondary)

            Text("This folder is empty")
                .font(.title3)
                .foregroundStyle(.secondary)

            HStack(spacing: 12) {
                Button("Add Files") { addFiles() }
                    .buttonStyle(.borderedProminent)
                Button("New Folder") { showNewFolder = true }
                    .buttonStyle(.bordered)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var fileTable: some View {
        Table(vaultManager.entries, selection: $selectedEntries, sortOrder: $sortOrder) {
            TableColumn("Name", sortUsing: KeyPathComparator(\VaultEntry.name)) { entry in
                HStack(spacing: 8) {
                    Image(systemName: entry.isDirectory ? "folder.fill" : fileIcon(for: entry.name))
                        .foregroundStyle(entry.isDirectory ? .blue : .secondary)
                        .frame(width: 20)

                    Text(entry.name)
                        .lineLimit(1)
                }
                .onTapGesture(count: 2) {
                    handleDoubleClick(entry)
                }
            }

            TableColumn("Size") { entry in
                if let size = entry.size, !entry.isDirectory {
                    Text(ByteCountFormatter.string(fromByteCount: size, countStyle: .file))
                        .foregroundStyle(.secondary)
                } else if entry.isDirectory {
                    Text("--")
                        .foregroundStyle(.tertiary)
                }
            }
            .width(min: 80, ideal: 100, max: 120)
        }
        .contextMenu(forSelectionType: UUID.self) { ids in
            let selected = vaultManager.entries.filter { ids.contains($0.id) }

            if selected.count == 1, let entry = selected.first {
                if entry.isDirectory {
                    Button("Open") { handleDoubleClick(entry) }
                } else {
                    Button("Export...") { exportFile(entry) }
                }
            }

            if !selected.isEmpty {
                Divider()
                Button("Delete", role: .destructive) {
                    Task {
                        for entry in selected {
                            await vaultManager.deleteEntry(entry)
                        }
                    }
                }
            }
        } primaryAction: { ids in
            if let id = ids.first,
               let entry = vaultManager.entries.first(where: { $0.id == id }) {
                handleDoubleClick(entry)
            }
        }
        .onChange(of: sortOrder) { newOrder in
            vaultManager.entries.sort(using: newOrder)
        }
    }

    private func handleDoubleClick(_ entry: VaultEntry) {
        if entry.isDirectory {
            Task { await vaultManager.navigateTo(directory: entry.name) }
        } else {
            exportFile(entry)
        }
    }

    private func addFiles() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = true
        panel.canChooseFiles = true
        panel.message = "Select files or folders to add to the vault"

        guard panel.runModal() == .OK else { return }
        Task { await vaultManager.addFiles(from: panel.urls) }
    }

    private func addFolder() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.message = "Select folders to add to the vault"

        guard panel.runModal() == .OK else { return }
        Task { await vaultManager.addFiles(from: panel.urls) }
    }

    private func exportFile(_ entry: VaultEntry) {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.message = "Choose export destination"
        panel.prompt = "Export"

        guard panel.runModal() == .OK, let url = panel.url else { return }
        Task { await vaultManager.extractFile(entry: entry, to: url) }
    }

    private func fileIcon(for name: String) -> String {
        let ext = (name as NSString).pathExtension.lowercased()
        switch ext {
        case "pdf": return "doc.fill"
        case "jpg", "jpeg", "png", "gif", "heic", "webp": return "photo.fill"
        case "mp4", "mov", "avi", "mkv": return "film.fill"
        case "mp3", "wav", "aac", "flac", "m4a": return "music.note"
        case "zip", "tar", "gz", "7z", "rar": return "archivebox.fill"
        case "txt", "md", "rtf": return "doc.text.fill"
        case "swift", "rs", "py", "js", "ts", "c", "h": return "chevron.left.forwardslash.chevron.right"
        default: return "doc.fill"
        }
    }
}

// MARK: - Breadcrumb bar

struct BreadcrumbBar: View {
    let pathStack: [String]
    let onNavigate: (Int) -> Void

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 4) {
                ForEach(Array(pathStack.enumerated()), id: \.offset) { index, path in
                    if index > 0 {
                        Image(systemName: "chevron.right")
                            .font(.caption2)
                            .foregroundStyle(.tertiary)
                    }

                    Button {
                        onNavigate(index)
                    } label: {
                        Text(index == 0 ? "Root" : (path as NSString).lastPathComponent)
                            .font(.callout)
                    }
                    .buttonStyle(.borderless)
                    .foregroundStyle(index == pathStack.count - 1 ? .primary : .secondary)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
        }
        .background(.bar)
    }
}

// MARK: - Sheets

struct NewFolderSheet: View {
    @Binding var name: String
    let onCreate: () -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 16) {
            Text("New Folder")
                .font(.headline)

            TextField("Folder name", text: $name)
                .textFieldStyle(.roundedBorder)
                .frame(width: 300)
                .onSubmit {
                    guard !name.isEmpty else { return }
                    onCreate()
                    dismiss()
                }

            HStack {
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Create") {
                    onCreate()
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(name.isEmpty)
            }
        }
        .padding(24)
    }
}

struct VaultInfoSheet: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Vault Information")
                .font(.headline)

            if let info = vaultManager.vaultInfo {
                Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 8) {
                    GridRow {
                        Text("Vault ID").foregroundStyle(.secondary)
                        Text(info.vaultId).textSelection(.enabled)
                    }
                    GridRow {
                        Text("Path").foregroundStyle(.secondary)
                        Text(info.rootPath).textSelection(.enabled)
                    }
                    GridRow {
                        Text("Version").foregroundStyle(.secondary)
                        Text("\(info.version)")
                    }
                    GridRow {
                        Text("Files").foregroundStyle(.secondary)
                        Text("\(info.fileCount)")
                    }
                    GridRow {
                        Text("Total Size").foregroundStyle(.secondary)
                        Text(ByteCountFormatter.string(fromByteCount: info.totalSize, countStyle: .file))
                    }
                }
            }

            HStack {
                Spacer()
                Button("Done") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(minWidth: 400)
        .onAppear {
            Task { await vaultManager.refreshVaultInfo() }
        }
    }
}

struct ChangePasswordSheet: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Environment(\.dismiss) private var dismiss
    @State private var currentPassword = ""
    @State private var newPassword = ""
    @State private var confirmPassword = ""

    private var isValid: Bool {
        !currentPassword.isEmpty && newPassword.count >= 8 && newPassword == confirmPassword
    }

    var body: some View {
        VStack(spacing: 16) {
            Text("Change Vault Password")
                .font(.headline)

            Form {
                SecureField("Current Password", text: $currentPassword)
                SecureField("New Password", text: $newPassword)
                SecureField("Confirm New Password", text: $confirmPassword)
            }
            .frame(width: 350)

            if !newPassword.isEmpty && newPassword.count < 8 {
                Text("Password must be at least 8 characters")
                    .font(.caption)
                    .foregroundStyle(.red)
            }

            if !confirmPassword.isEmpty && newPassword != confirmPassword {
                Text("Passwords do not match")
                    .font(.caption)
                    .foregroundStyle(.red)
            }

            HStack {
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Change Password") {
                    Task {
                        await vaultManager.changePassword(old: currentPassword, new: newPassword)
                        dismiss()
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!isValid)
            }
        }
        .padding(24)
    }
}
