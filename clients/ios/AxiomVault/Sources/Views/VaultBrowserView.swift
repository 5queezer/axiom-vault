import SwiftUI

struct VaultBrowserView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @State private var showingAddFile = false
    @State private var showingCreateDirectory = false
    @State private var showingVaultInfo = false
    @State private var showingChangePassword = false
    @State private var selectedEntry: VaultEntry?
    @State private var showingExportSheet = false

    var body: some View {
        VStack(spacing: 0) {
            // Path breadcrumb
            pathBreadcrumb

            // Content list
            if vaultManager.isLoading && vaultManager.entries.isEmpty {
                ProgressView("Loading...")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if vaultManager.entries.isEmpty {
                emptyFolderView
            } else {
                entryList
            }
        }
        .navigationTitle(currentFolderName)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .navigationBarTrailing) {
                Menu {
                    Button(action: { showingAddFile = true }) {
                        Label("Add File", systemImage: "doc.badge.plus")
                    }

                    Button(action: { showingCreateDirectory = true }) {
                        Label("New Folder", systemImage: "folder.badge.plus")
                    }

                    Divider()

                    Button(action: { showingVaultInfo = true }) {
                        Label("Vault Info", systemImage: "info.circle")
                    }

                    Button(action: { showingChangePassword = true }) {
                        Label("Change Password", systemImage: "key.fill")
                    }

                    Divider()

                    Button(action: {
                        Task {
                            await vaultManager.refreshEntries()
                        }
                    }) {
                        Label("Refresh", systemImage: "arrow.clockwise")
                    }
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
            }
        }
        .sheet(isPresented: $showingAddFile) {
            AddFileView()
        }
        .sheet(isPresented: $showingCreateDirectory) {
            CreateDirectoryView()
        }
        .sheet(isPresented: $showingVaultInfo) {
            VaultInfoView()
        }
        .sheet(isPresented: $showingChangePassword) {
            ChangePasswordView()
        }
        .onAppear {
            if vaultManager.entries.isEmpty {
                Task {
                    await vaultManager.refreshEntries()
                }
            }
        }
    }

    private var currentFolderName: String {
        if vaultManager.currentPath == "/" {
            return "Root"
        }
        return (vaultManager.currentPath as NSString).lastPathComponent
    }

    private var pathBreadcrumb: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 4) {
                ForEach(vaultManager.pathStack.indices, id: \.self) { index in
                    let path = vaultManager.pathStack[index]
                    let name = path == "/" ? "Root" : (path as NSString).lastPathComponent

                    if index > 0 {
                        Image(systemName: "chevron.right")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }

                    Button(name) {
                        // Navigate to this path level
                        Task {
                            // Remove all paths after this index
                            await MainActor.run {
                                while vaultManager.pathStack.count > index + 1 {
                                    vaultManager.pathStack.removeLast()
                                }
                                vaultManager.currentPath = path
                            }
                            await vaultManager.refreshEntries()
                        }
                    }
                    .buttonStyle(.plain)
                    .font(.caption)
                    .foregroundColor(index == vaultManager.pathStack.count - 1 ? .primary : .blue)
                }
            }
            .padding(.horizontal)
            .padding(.vertical, 8)
        }
        .background(Color(.systemGray6))
    }

    private var emptyFolderView: some View {
        VStack(spacing: 20) {
            Image(systemName: "folder")
                .font(.system(size: 60))
                .foregroundColor(.secondary)

            Text("This folder is empty")
                .font(.headline)
                .foregroundColor(.secondary)

            HStack(spacing: 16) {
                Button(action: { showingAddFile = true }) {
                    Label("Add File", systemImage: "doc.badge.plus")
                }
                .buttonStyle(.bordered)

                Button(action: { showingCreateDirectory = true }) {
                    Label("New Folder", systemImage: "folder.badge.plus")
                }
                .buttonStyle(.bordered)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var entryList: some View {
        List {
            // Back button if not at root
            if vaultManager.currentPath != "/" {
                Button(action: {
                    Task {
                        await vaultManager.navigateTo(directory: "..")
                    }
                }) {
                    HStack {
                        Image(systemName: "arrow.left")
                            .foregroundColor(.blue)
                        Text("Back")
                            .foregroundColor(.blue)
                    }
                }
            }

            ForEach(vaultManager.entries) { entry in
                entryRow(entry)
            }
            .onDelete(perform: deleteEntries)
        }
        .listStyle(.insetGrouped)
        .refreshable {
            await vaultManager.refreshEntries()
        }
    }

    private func entryRow(_ entry: VaultEntry) -> some View {
        HStack {
            Image(systemName: entry.isDirectory ? "folder.fill" : "doc.fill")
                .foregroundColor(entry.isDirectory ? .blue : .gray)
                .font(.title2)

            VStack(alignment: .leading) {
                Text(entry.name)
                    .font(.body)

                if let size = entry.size, !entry.isDirectory {
                    Text(formatSize(size))
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }

            Spacer()

            if entry.isDirectory {
                Image(systemName: "chevron.right")
                    .foregroundColor(.secondary)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture {
            if entry.isDirectory {
                Task {
                    await vaultManager.navigateTo(directory: entry.name)
                }
            } else {
                selectedEntry = entry
                showingExportSheet = true
            }
        }
    }

    private func deleteEntries(at offsets: IndexSet) {
        Task {
            for index in offsets {
                let entry = vaultManager.entries[index]
                await vaultManager.deleteEntry(entry)
            }
        }
    }

    private func formatSize(_ bytes: Int64) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        return formatter.string(fromByteCount: bytes)
    }
}

struct AddFileView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Environment(\.dismiss) var dismiss
    @State private var showingDocumentPicker = false

    var body: some View {
        NavigationView {
            VStack(spacing: 20) {
                Image(systemName: "doc.badge.plus")
                    .font(.system(size: 60))
                    .foregroundColor(.blue)

                Text("Add File to Vault")
                    .font(.headline)

                Text("Select a file from your device to encrypt and store in the vault")
                    .font(.subheadline)
                    .foregroundColor(.secondary)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal)

                Button(action: { showingDocumentPicker = true }) {
                    Label("Choose File", systemImage: "folder")
                        .frame(maxWidth: .infinity)
                        .padding()
                        .background(Color.blue)
                        .foregroundColor(.white)
                        .cornerRadius(10)
                }
                .padding(.horizontal, 40)

                Spacer()
            }
            .padding(.top, 40)
            .navigationTitle("Add File")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        dismiss()
                    }
                }
            }
        }
        .fileImporter(
            isPresented: $showingDocumentPicker,
            allowedContentTypes: [.item],
            allowsMultipleSelection: false
        ) { result in
            switch result {
            case .success(let urls):
                if let url = urls.first {
                    Task {
                        // Start accessing the security-scoped resource
                        guard url.startAccessingSecurityScopedResource() else {
                            return
                        }
                        defer { url.stopAccessingSecurityScopedResource() }

                        await vaultManager.addFile(from: url)
                        dismiss()
                    }
                }
            case .failure:
                break
            }
        }
    }
}

struct CreateDirectoryView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Environment(\.dismiss) var dismiss
    @State private var directoryName = ""

    var body: some View {
        NavigationView {
            Form {
                Section(header: Text("Folder Name")) {
                    TextField("New Folder", text: $directoryName)
                        .autocapitalization(.none)
                }

                Section {
                    Button(action: createDirectory) {
                        if vaultManager.isLoading {
                            ProgressView()
                                .frame(maxWidth: .infinity)
                        } else {
                            Text("Create Folder")
                                .frame(maxWidth: .infinity)
                        }
                    }
                    .disabled(directoryName.isEmpty || vaultManager.isLoading)
                }
            }
            .navigationTitle("New Folder")
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

    private func createDirectory() {
        Task {
            await vaultManager.createDirectory(name: directoryName)
            dismiss()
        }
    }
}

struct VaultInfoView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Environment(\.dismiss) var dismiss

    var body: some View {
        NavigationView {
            List {
                if let info = vaultManager.vaultInfo {
                    Section(header: Text("Vault Details")) {
                        LabeledContent("Vault ID", value: info.vaultId)
                        LabeledContent("Path", value: info.rootPath)
                        LabeledContent("Version", value: "\(info.version)")
                    }

                    Section(header: Text("Statistics")) {
                        LabeledContent("Files", value: "\(info.fileCount)")
                        LabeledContent("Total Size", value: formatSize(info.totalSize))
                    }
                } else {
                    Text("Loading vault information...")
                }
            }
            .navigationTitle("Vault Info")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        dismiss()
                    }
                }
            }
        }
        .onAppear {
            Task {
                await vaultManager.refreshVaultInfo()
            }
        }
    }

    private func formatSize(_ bytes: Int64) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        return formatter.string(fromByteCount: bytes)
    }
}

struct ChangePasswordView: View {
    @EnvironmentObject var vaultManager: VaultManager
    @Environment(\.dismiss) var dismiss

    @State private var currentPassword = ""
    @State private var newPassword = ""
    @State private var confirmPassword = ""
    @State private var showPasswords = false

    var passwordsMatch: Bool {
        !newPassword.isEmpty && newPassword == confirmPassword
    }

    var isFormValid: Bool {
        !currentPassword.isEmpty && passwordsMatch && newPassword.count >= 8
    }

    var body: some View {
        NavigationView {
            Form {
                Section(header: Text("Current Password")) {
                    if showPasswords {
                        TextField("Current Password", text: $currentPassword)
                            .autocapitalization(.none)
                    } else {
                        SecureField("Current Password", text: $currentPassword)
                    }
                }

                Section(header: Text("New Password"), footer: passwordFooter) {
                    if showPasswords {
                        TextField("New Password", text: $newPassword)
                            .autocapitalization(.none)
                        TextField("Confirm Password", text: $confirmPassword)
                            .autocapitalization(.none)
                    } else {
                        SecureField("New Password", text: $newPassword)
                        SecureField("Confirm Password", text: $confirmPassword)
                    }

                    Toggle("Show Passwords", isOn: $showPasswords)
                }

                Section {
                    Button(action: changePassword) {
                        if vaultManager.isLoading {
                            ProgressView()
                                .frame(maxWidth: .infinity)
                        } else {
                            Text("Change Password")
                                .frame(maxWidth: .infinity)
                        }
                    }
                    .disabled(!isFormValid || vaultManager.isLoading)
                }
            }
            .navigationTitle("Change Password")
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
            if newPassword.isEmpty {
                Text("Password must be at least 8 characters")
                    .foregroundColor(.secondary)
            } else if newPassword.count < 8 {
                Text("Password too short (\(newPassword.count)/8)")
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

    private func changePassword() {
        Task {
            await vaultManager.changePassword(old: currentPassword, new: newPassword)
            if vaultManager.errorMessage == nil {
                dismiss()
            }
        }
    }
}

#Preview {
    VaultBrowserView()
        .environmentObject(VaultManager())
}
