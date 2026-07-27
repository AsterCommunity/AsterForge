import Darwin
import FileProvider
import Foundation
import UniformTypeIdentifiers

private struct MemoryCloudExampleFailure: Error, CustomStringConvertible {
    let description: String
}

@main
enum MemoryCloudDriveExample {
    static func main() {
        do {
            let command = try MacosMemoryCloudExampleCommand.parse(
                arguments: Array(CommandLine.arguments.dropFirst())
            )
            try run(command)
        } catch {
            fputs("error: \(String(describing: error))\n", stderr)
            fputs("\(MacosMemoryCloudExampleOutput.help)\n", stderr)
            exit(EXIT_FAILURE)
        }
    }

    private static func run(_ command: MacosMemoryCloudExampleCommand) throws {
        switch command {
        case let .cat(path):
            FileHandle.standardOutput.write(try MemoryCloudFixture().read(path))
        case let .changes(anchor):
            let batch = try MemoryCloudFixture().changes(anchor)
            print(
                MacosMemoryCloudExampleOutput.changes(
                    updated: batch.updatedItems.map(entry),
                    deleted: batch.deletedIdentifiers.map(\.rawValue),
                    moreComing: batch.moreComing,
                    anchor: displayAnchor(batch.syncAnchor)
                )
            )
        case .help:
            print(MacosMemoryCloudExampleOutput.help)
        case let .list(path):
            print(
                MacosMemoryCloudExampleOutput.list(
                    try MemoryCloudFixture().list(path).map(entry)
                )
            )
        case .smoke:
            print(
                MacosMemoryCloudExampleOutput.smoke(
                    steps: try MemoryCloudFixture.runSmoke()
                )
            )
        case .workingSet:
            print(
                MacosMemoryCloudExampleOutput.list(
                    try MemoryCloudFixture.materializedWorkingSet().map(entry)
                )
            )
        }
    }

    private static func entry(_ item: NSFileProviderItem) -> MacosMemoryCloudExampleEntry {
        MacosMemoryCloudExampleEntry(
            kind: item.contentType == UTType.folder ? .directory : .file,
            name: item.filename,
            size: (item.documentSize ?? nil)?.uint64Value ?? 0
        )
    }

    private static func displayAnchor(_ anchor: NSFileProviderSyncAnchor) -> String {
        let bytes = anchor as NSData as Data
        return String(data: bytes, encoding: .utf8) ?? bytes.base64EncodedString()
    }
}

private final class MemoryCloudFixture {
    private let runtime: MacosReadOnlyFileProviderRuntime
    private let temporaryDirectory: URL

    init(source: MemoryCloudDataSource? = nil) throws {
        let source = try source ?? MemoryCloudDataSource()
        temporaryDirectory = FileManager.default.temporaryDirectory.appendingPathComponent(
            "aster-forge-macos-memory-content-\(UUID().uuidString)",
            isDirectory: true
        )
        runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: try RustMacosBridgeSession(generation: 1),
            temporaryContentStore: try MacosDirectoryTemporaryContentStore(
                directory: temporaryDirectory
            )
        )
    }

    deinit {
        runtime.invalidate()
        try? FileManager.default.removeItem(at: temporaryDirectory)
    }

    func list(_ path: MacosMemoryCloudExamplePath) throws -> [NSFileProviderItem] {
        let containerIdentifier: NSFileProviderItemIdentifier
        if path.components.isEmpty {
            containerIdentifier = .rootContainer
        } else {
            let item = try resolve(path)
            guard item.contentType == UTType.folder else {
                throw MemoryCloudExampleFailure(description: "\(path) is not a directory")
            }
            containerIdentifier = item.itemIdentifier
        }
        return try enumerate(containerIdentifier)
    }

    func read(_ path: MacosMemoryCloudExamplePath) throws -> Data {
        guard !path.components.isEmpty else {
            throw MemoryCloudExampleFailure(description: "root is not a file")
        }
        let item = try resolve(path)
        guard item.contentType != UTType.folder else {
            throw MemoryCloudExampleFailure(description: "\(path) is not a file")
        }

        var result: Result<URL, Error>?
        _ = runtime.fetchContents(
            for: item.itemIdentifier,
            requestedVersion: item.itemVersion
        ) { url, _, error in
            if let error {
                result = .failure(error)
            } else if let url {
                result = .success(url)
            } else {
                result = .failure(
                    MemoryCloudExampleFailure(description: "fetch returned no content URL")
                )
            }
        }
        guard let result else {
            throw MemoryCloudExampleFailure(description: "fetch did not finish synchronously")
        }
        let url = try result.get()
        defer { try? FileManager.default.removeItem(at: url) }
        return try Data(contentsOf: url)
    }

    func changes(_ anchor: MacosMemoryCloudExampleAnchor) throws -> MemoryCloudChangeResult {
        let enumerator = try runtime.enumerator(for: .rootContainer)
        defer { enumerator.invalidate() }

        let syncAnchor: NSFileProviderSyncAnchor
        switch anchor {
        case .initial:
            syncAnchor = NSFileProviderSyncAnchor(Data())
        case .current:
            guard let currentSyncAnchor = enumerator.currentSyncAnchor else {
                throw MemoryCloudExampleFailure(
                    description: "current anchor operation is unavailable"
                )
            }
            var captured: NSFileProviderSyncAnchor?
            currentSyncAnchor { captured = $0 }
            guard let captured else {
                throw MemoryCloudExampleFailure(description: "current anchor is missing")
            }
            syncAnchor = captured
        case .expired:
            syncAnchor = NSFileProviderSyncAnchor(Data("expired".utf8))
        }

        let observer = MemoryCloudChangeObserver()
        guard let enumerateChanges = enumerator.enumerateChanges else {
            throw MemoryCloudExampleFailure(
                description: "change enumeration operation is unavailable"
            )
        }
        enumerateChanges(observer, syncAnchor)
        if let error = observer.error {
            throw error
        }
        guard let result = observer.result else {
            throw MemoryCloudExampleFailure(description: "change enumeration did not finish")
        }
        return result
    }

    static func materializedWorkingSet() throws -> [NSFileProviderItem] {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(
            "aster-forge-macos-memory-example-\(UUID().uuidString)",
            isDirectory: true
        )
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = try MacosFileMaterializedSetStore(directory: directory)
        let fixture = try MemoryCloudFixture(
            source: MemoryCloudDataSource(materializedStore: store)
        )
        let documents = try fixture.resolve(
            MacosMemoryCloudExamplePath(argument: "Documents")
        )
        try store.replace(
            with: MacosMaterializedSetSnapshot(
                directoryIdentifiers: [documents.itemIdentifier.rawValue],
                syncAnchor: try MacosSyncAnchor(bytes: Data("memory-fixture-v1".utf8))
            )
        )
        return try fixture.enumerate(.workingSet)
    }

    static func runSmoke() throws -> [String] {
        try verifyErrorCodeContract()
        try verifyRustSessionAndIdentifierOwnership()
        let fixture = try MemoryCloudFixture()

        let root = try fixture.list(.root)
        try require(
            root.map(\.filename) == ["README.txt", "Documents"],
            "root enumeration changed"
        )
        try require(
            try fixture.list(MacosMemoryCloudExamplePath(argument: "Documents"))
                .map(\.filename) == ["hello.txt"],
            "nested enumeration changed"
        )
        guard let readme = root.first(where: { $0.filename == "README.txt" }) else {
            throw MemoryCloudExampleFailure(description: "README.txt is missing")
        }
        try require(
            try fixture.item(readme.itemIdentifier).filename == "README.txt",
            "runtime item lookup changed"
        )
        try require(
            try fixture.read(MacosMemoryCloudExamplePath(argument: "README.txt"))
                == Data("AsterForge in-memory File Provider fixture.\n".utf8),
            "README.txt bytes changed"
        )
        try require(
            try fixture.read(MacosMemoryCloudExamplePath(argument: "Documents/hello.txt"))
                == Data("Hello from the in-memory cloud.\n".utf8),
            "hello.txt bytes changed"
        )

        let initial = try fixture.changes(.initial)
        try require(
            initial.updatedItems.map(\.filename) == ["README.txt", "Documents"]
                && initial.deletedIdentifiers.isEmpty
                && !initial.moreComing,
            "initial change feed changed"
        )
        let current = try fixture.changes(.current)
        try require(
            current.updatedItems.isEmpty
                && current.deletedIdentifiers.isEmpty
                && !current.moreComing,
            "current change feed should be terminal and empty"
        )
        do {
            _ = try fixture.changes(.expired)
            throw MemoryCloudExampleFailure(description: "expired anchor was accepted")
        } catch {
            let native = error as NSError
            try require(
                native.domain == NSFileProviderErrorDomain
                    && native.code == NSFileProviderError.Code.syncAnchorExpired.rawValue,
                "expired anchor error changed"
            )
        }

        try require(
            try materializedWorkingSet().map(\.filename) == ["Documents", "hello.txt"],
            "materialized working set changed"
        )
        return [
            "ffi-error-codes",
            "rust-session-and-identifier",
            "root-list",
            "nested-list",
            "item-lookup",
            "readme-fetch",
            "nested-fetch",
            "initial-changes",
            "current-changes",
            "expired-anchor",
            "materialized-working-set",
        ]
    }

    private func item(_ identifier: NSFileProviderItemIdentifier) throws -> NSFileProviderItem {
        var result: Result<NSFileProviderItem, Error>?
        _ = runtime.item(for: identifier) { item, error in
            if let error {
                result = .failure(error)
            } else if let item {
                result = .success(item)
            } else {
                result = .failure(
                    MemoryCloudExampleFailure(description: "item lookup returned no item")
                )
            }
        }
        guard let result else {
            throw MemoryCloudExampleFailure(description: "item lookup did not finish")
        }
        return try result.get()
    }

    private func resolve(_ path: MacosMemoryCloudExamplePath) throws -> NSFileProviderItem {
        var containerIdentifier = NSFileProviderItemIdentifier.rootContainer
        var resolved: NSFileProviderItem?
        for component in path.components {
            guard let item = try enumerate(containerIdentifier).first(where: {
                $0.filename == component
            }) else {
                throw MemoryCloudExampleFailure(description: "\(path) was not found")
            }
            resolved = item
            containerIdentifier = item.itemIdentifier
        }
        guard let resolved else {
            throw MemoryCloudExampleFailure(description: "root has no item snapshot")
        }
        return resolved
    }

    private func enumerate(
        _ containerIdentifier: NSFileProviderItemIdentifier
    ) throws -> [NSFileProviderItem] {
        let enumerator = try runtime.enumerator(for: containerIdentifier)
        defer { enumerator.invalidate() }
        let observer = MemoryCloudEnumerationObserver()
        enumerator.enumerateItems(
            for: observer,
            startingAt: NSFileProviderPage.initialPageSortedByName as NSFileProviderPage
        )
        if let error = observer.error {
            throw error
        }
        guard observer.finished else {
            throw MemoryCloudExampleFailure(description: "directory enumeration did not finish")
        }
        return observer.items
    }

    private static func verifyErrorCodeContract() throws {
        let rawCodes: [(AsterForgeMacosErrorCode, MacosBridgeErrorCode)] = [
            (ASTER_FORGE_MACOS_SUCCESS, .success),
            (ASTER_FORGE_MACOS_NOT_FOUND, .notFound),
            (ASTER_FORGE_MACOS_NOT_AUTHENTICATED, .notAuthenticated),
            (ASTER_FORGE_MACOS_PERMISSION_DENIED, .permissionDenied),
            (ASTER_FORGE_MACOS_VERSION_OUT_OF_DATE, .versionOutOfDate),
            (ASTER_FORGE_MACOS_TRY_AGAIN, .tryAgain),
            (ASTER_FORGE_MACOS_NOT_SUPPORTED, .notSupported),
            (ASTER_FORGE_MACOS_INVALID_ARGUMENT, .invalidArgument),
            (ASTER_FORGE_MACOS_SYNC_ANCHOR_EXPIRED, .syncAnchorExpired),
            (ASTER_FORGE_MACOS_CANCELLED, .cancelled),
            (ASTER_FORGE_MACOS_PROVIDER_NOT_FOUND, .providerNotFound),
            (ASTER_FORGE_MACOS_INTERNAL, .internal),
        ]
        for (cCode, swiftCode) in rawCodes {
            try require(
                Int32(cCode.rawValue) == swiftCode.rawValue,
                "C and Swift error codes diverged for \(swiftCode)"
            )
        }
    }

    private static func verifyRustSessionAndIdentifierOwnership() throws {
        let session = try RustMacosBridgeSession(generation: 42)
        try require(session.generation == 42, "Rust session generation changed")
        let request = try session.beginRequest()
        try require(request.generation == 42, "request generation changed")
        request.release()
        request.release()
        session.beginClosing()
        session.markDisconnected()
        try require(
            !RustMacosIdentifierCodec.encode(
                namespace: "fixture",
                root: "memory",
                item: "readme"
            ).isEmpty,
            "Rust identifier encoding returned an empty value"
        )
    }

    private static func require(
        _ condition: @autoclosure () throws -> Bool,
        _ message: String
    ) throws {
        guard try condition() else {
            throw MemoryCloudExampleFailure(description: message)
        }
    }
}

private struct MemoryCloudChangeResult {
    let updatedItems: [NSFileProviderItem]
    let deletedIdentifiers: [NSFileProviderItemIdentifier]
    let syncAnchor: NSFileProviderSyncAnchor
    let moreComing: Bool
}

private final class MemoryCloudEnumerationObserver: NSObject, NSFileProviderEnumerationObserver {
    private(set) var items: [NSFileProviderItem] = []
    private(set) var error: Error?
    private(set) var finished = false

    func didEnumerate(_ updatedItems: [NSFileProviderItem]) {
        items.append(contentsOf: updatedItems)
    }

    func finishEnumerating(upTo _: NSFileProviderPage?) {
        finished = true
    }

    func finishEnumeratingWithError(_ error: Error) {
        self.error = error
    }
}

private final class MemoryCloudChangeObserver: NSObject, NSFileProviderChangeObserver {
    private var updatedItems: [NSFileProviderItem] = []
    private var deletedIdentifiers: [NSFileProviderItemIdentifier] = []
    private(set) var error: Error?
    private(set) var result: MemoryCloudChangeResult?

    func didUpdate(_ updatedItems: [NSFileProviderItem]) {
        self.updatedItems.append(contentsOf: updatedItems)
    }

    func didDeleteItems(withIdentifiers deletedItemIdentifiers: [NSFileProviderItemIdentifier]) {
        deletedIdentifiers.append(contentsOf: deletedItemIdentifiers)
    }

    func finishEnumeratingChanges(
        upTo anchor: NSFileProviderSyncAnchor,
        moreComing: Bool
    ) {
        result = MemoryCloudChangeResult(
            updatedItems: updatedItems,
            deletedIdentifiers: deletedIdentifiers,
            syncAnchor: anchor,
            moreComing: moreComing
        )
    }

    func finishEnumeratingWithError(_ error: Error) {
        self.error = error
    }
}
