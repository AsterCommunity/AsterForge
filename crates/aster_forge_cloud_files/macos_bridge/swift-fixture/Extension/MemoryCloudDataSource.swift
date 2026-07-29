import FileProvider
import Foundation
import UniformTypeIdentifiers

final class MemoryCloudDataSource: MacosCloudFilesDataSource {
    private struct Entry {
        let snapshot: MacosCloudItemSnapshot
        let content: Data?
    }

    private let entries: [String: Entry]
    private let children: [String: [MacosCloudItemSnapshot]]
    private let currentAnchor: MacosSyncAnchor
    private let materializedStore: (any MacosMaterializedSetPersisting)?
    private let stagingDirectory: URL

    init(materializedStore: (any MacosMaterializedSetPersisting)? = nil) throws {
        self.materializedStore = materializedStore
        stagingDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("aster-forge-memory-cloud-staging", isDirectory: true)
        try FileManager.default.createDirectory(
            at: stagingDirectory,
            withIntermediateDirectories: true
        )
        let namespace = "aster-forge-fixture"
        let root = "memory-cloud"
        let rootIdentifier = NSFileProviderItemIdentifier.rootContainer.rawValue
        let readmeIdentifier = try RustMacosIdentifierCodec.encode(
            namespace: namespace,
            root: root,
            item: "readme"
        )
        let documentsIdentifier = try RustMacosIdentifierCodec.encode(
            namespace: namespace,
            root: root,
            item: "documents"
        )
        let helloIdentifier = try RustMacosIdentifierCodec.encode(
            namespace: namespace,
            root: root,
            item: "hello"
        )
        let readmeContent = Data("AsterForge in-memory File Provider fixture.\n".utf8)
        let helloContent = Data("Hello from the in-memory cloud.\n".utf8)
        currentAnchor = try MacosSyncAnchor(bytes: Data("memory-fixture-v1".utf8))

        let rootItem = try Self.snapshot(
            identifier: rootIdentifier,
            parentIdentifier: rootIdentifier,
            filename: FixtureDomainConfiguration.displayName,
            kind: .directory,
            content: nil,
            revision: "root-v1"
        )
        let readme = try Self.snapshot(
            identifier: readmeIdentifier,
            parentIdentifier: rootIdentifier,
            filename: "README.txt",
            kind: .file,
            content: readmeContent,
            revision: "readme-v1"
        )
        let documents = try Self.snapshot(
            identifier: documentsIdentifier,
            parentIdentifier: rootIdentifier,
            filename: "Documents",
            kind: .directory,
            content: nil,
            revision: "documents-v1"
        )
        let hello = try Self.snapshot(
            identifier: helloIdentifier,
            parentIdentifier: documentsIdentifier,
            filename: "hello.txt",
            kind: .file,
            content: helloContent,
            revision: "hello-v1"
        )

        entries = [
            rootIdentifier: Entry(snapshot: rootItem, content: nil),
            readmeIdentifier: Entry(snapshot: readme, content: readmeContent),
            documentsIdentifier: Entry(snapshot: documents, content: nil),
            helloIdentifier: Entry(snapshot: hello, content: helloContent),
        ]
        children = [
            rootIdentifier: [readme, documents],
            documentsIdentifier: [hello],
            NSFileProviderItemIdentifier.workingSet.rawValue: [readme, documents, hello],
            NSFileProviderItemIdentifier.trashContainer.rawValue: [],
        ]
    }

    func currentSyncAnchor(containerIdentifier _: String) -> MacosSyncAnchor {
        currentAnchor
    }

    func item(
        for identifier: String,
        completion: @escaping (Result<MacosCloudItemSnapshot, Error>) -> Void
    ) -> any MacosCancellable {
        if let entry = entries[identifier] {
            completion(.success(entry.snapshot))
        } else {
            completion(.failure(MacosBridgeFailure(code: .notFound)))
        }
        return MacosNoopCancellation()
    }

    func enumerate(
        containerIdentifier: String,
        page: Data?,
        completion: @escaping (Result<MacosEnumerationPage, Error>) -> Void
    ) -> any MacosCancellable {
        do {
            let items = try items(for: containerIdentifier)
            completion(.success(try MacosEnumerationPage(items: page == nil ? items : [], nextPage: nil)))
        } catch {
            completion(.failure(error))
        }
        return MacosNoopCancellation()
    }

    func enumerateChanges(
        containerIdentifier: String,
        from syncAnchor: MacosSyncAnchor,
        completion: @escaping (Result<MacosChangeBatch, Error>) -> Void
    ) -> any MacosCancellable {
        do {
            let items = try items(for: containerIdentifier)
            if syncAnchor == currentAnchor {
                completion(
                    .success(
                        try MacosChangeBatch(
                            updatedItems: [],
                            deletedItemIdentifiers: [],
                            syncAnchor: currentAnchor,
                            moreComing: false
                        )
                    )
                )
            } else if syncAnchor.bytes.isEmpty {
                completion(
                    .success(
                        try MacosChangeBatch(
                            updatedItems: items,
                            deletedItemIdentifiers: [],
                            syncAnchor: currentAnchor,
                            moreComing: false
                        )
                    )
                )
            } else {
                completion(.failure(MacosBridgeFailure(code: .syncAnchorExpired)))
            }
        } catch {
            completion(.failure(error))
        }
        return MacosNoopCancellation()
    }

    private func items(for containerIdentifier: String) throws -> [MacosCloudItemSnapshot] {
        guard let items = children[containerIdentifier] else {
            throw MacosBridgeFailure(code: .notFound)
        }
        guard containerIdentifier == NSFileProviderItemIdentifier.workingSet.rawValue,
              let materializedStore
        else {
            return items
        }
        let materializedDirectories = try materializedStore.load().directoryIdentifiers
        return items.filter {
            materializedDirectories.contains($0.identifier)
                || materializedDirectories.contains($0.parentIdentifier)
        }
    }

    func fetchContents(
        for identifier: String,
        requestedContentVersion: Data?,
        completion: @escaping (Result<MacosFetchedContent, Error>) -> Void
    ) -> any MacosCancellable {
        guard let entry = entries[identifier], let content = entry.content else {
            completion(.failure(MacosBridgeFailure(code: .notFound)))
            return MacosNoopCancellation()
        }
        if let requestedContentVersion,
           requestedContentVersion != entry.snapshot.contentVersion
        {
            completion(.failure(MacosBridgeFailure(code: .versionOutOfDate)))
            return MacosNoopCancellation()
        }
        do {
            let stagingURL = stagingDirectory
                .appendingPathComponent(UUID().uuidString, isDirectory: false)
            try content.write(to: stagingURL, options: .atomic)
            completion(
                .success(
                    try MacosFetchedContent(item: entry.snapshot, stagingURL: stagingURL)
                )
            )
        } catch {
            completion(.failure(error))
        }
        return MacosNoopCancellation()
    }

    func discardFetchedContents(at stagingURL: URL) {
        guard stagingURL.deletingLastPathComponent() == stagingDirectory else { return }
        try? FileManager.default.removeItem(at: stagingURL)
    }

    private static func snapshot(
        identifier: String,
        parentIdentifier: String,
        filename: String,
        kind: MacosCloudItemKind,
        content: Data?,
        revision: String
    ) throws -> MacosCloudItemSnapshot {
        try MacosCloudItemSnapshot(
            identifier: identifier,
            parentIdentifier: parentIdentifier,
            filename: filename,
            kind: kind,
            size: UInt64(content?.count ?? 0),
            metadataVersion: Data("metadata-\(revision)".utf8),
            contentVersion: Data("content-\(revision)".utf8),
            contentTypeIdentifier: kind == .directory
                ? UTType.folder.identifier
                : UTType.plainText.identifier
        )
    }
}
