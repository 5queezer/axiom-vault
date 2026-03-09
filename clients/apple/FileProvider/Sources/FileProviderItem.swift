import FileProvider
import UniformTypeIdentifiers

class FileProviderItem: NSObject, NSFileProviderItem {
    let entry: VaultEntry
    let _parentIdentifier: NSFileProviderItemIdentifier
    let _vaultPath: String

    init(entry: VaultEntry, parentIdentifier: NSFileProviderItemIdentifier, vaultPath: String) {
        self.entry = entry
        self._parentIdentifier = parentIdentifier
        self._vaultPath = vaultPath
        super.init()
    }

    var itemIdentifier: NSFileProviderItemIdentifier {
        Self.identifier(for: _vaultPath)
    }

    var parentItemIdentifier: NSFileProviderItemIdentifier {
        _parentIdentifier
    }

    var capabilities: NSFileProviderItemCapabilities {
        if entry.isDirectory {
            return [.allowsReading, .allowsWriting, .allowsDeleting, .allowsAddingSubItems, .allowsContentEnumerating]
        }
        return [.allowsReading, .allowsWriting, .allowsDeleting]
    }

    var filename: String {
        entry.name
    }

    var contentType: UTType {
        if entry.isDirectory {
            return .folder
        }
        let ext = (entry.name as NSString).pathExtension
        return UTType(filenameExtension: ext) ?? .data
    }

    var documentSize: NSNumber? {
        entry.size.map { NSNumber(value: $0) }
    }

    var itemVersion: NSFileProviderItemVersion {
        NSFileProviderItemVersion(
            contentVersion: Data(count: 1),
            metadataVersion: Data(count: 1)
        )
    }

    // MARK: - Root item

    static let root: FileProviderItem = {
        let entry = VaultEntry(name: "", isDirectory: true, size: nil)
        return FileProviderItem(
            entry: entry,
            parentIdentifier: .rootContainer,
            vaultPath: "/"
        )
    }()

    // MARK: - Path <-> Identifier mapping

    static func identifier(for vaultPath: String) -> NSFileProviderItemIdentifier {
        if vaultPath == "/" {
            return .rootContainer
        }
        // Encode path as identifier (base64 to avoid invalid chars)
        let encoded = Data(vaultPath.utf8).base64EncodedString()
        return NSFileProviderItemIdentifier(encoded)
    }

    static func vaultPath(from identifier: NSFileProviderItemIdentifier) -> String {
        if identifier == .rootContainer {
            return "/"
        }
        guard let data = Data(base64Encoded: identifier.rawValue),
              let path = String(data: data, encoding: .utf8) else {
            return "/"
        }
        return path
    }
}
