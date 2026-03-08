import FileProvider
import os.log

class FileProviderEnumerator: NSObject, NSFileProviderEnumerator {
    private let logger = Logger(subsystem: "com.axiomvault.macos.fileprovider", category: "enumerator")
    private let vaultPath: String
    private let parentIdentifier: NSFileProviderItemIdentifier

    init(vaultPath: String, parentIdentifier: NSFileProviderItemIdentifier) {
        self.vaultPath = vaultPath
        self.parentIdentifier = parentIdentifier
        super.init()
    }

    func invalidate() {}

    func enumerateItems(
        for observer: NSFileProviderEnumerationObserver,
        startingAt page: NSFileProviderPage
    ) {
        do {
            let entries = try VaultCore.shared.listDirectory(at: vaultPath)

            let items: [NSFileProviderItem] = entries.map { entry in
                let childPath = vaultPath == "/"
                    ? "/\(entry.name)"
                    : "\(vaultPath)/\(entry.name)"

                return FileProviderItem(
                    entry: entry,
                    parentIdentifier: parentIdentifier,
                    vaultPath: childPath
                )
            }

            observer.didEnumerate(items)
            observer.finishEnumerating(upTo: nil)
        } catch {
            logger.error("enumerateItems failed: \(error.localizedDescription)")
            observer.finishEnumeratingWithError(error)
        }
    }

    func enumerateChanges(
        for observer: NSFileProviderChangeObserver,
        from syncAnchor: NSFileProviderSyncAnchor
    ) {
        // For now, report no changes — full sync is handled by re-enumeration
        observer.finishEnumeratingChanges(upTo: syncAnchor, moreComing: false)
    }

    func currentSyncAnchor(completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void) {
        let anchor = NSFileProviderSyncAnchor(Data(count: 1))
        completionHandler(anchor)
    }
}
