import FileProvider
import Foundation
import UniformTypeIdentifiers
import XCTest
@testable import AsterForgeCloudFilesMacosShell

final class ShellContractTests: XCTestCase {
    func testItemSnapshotAndFileProviderMappingPreserveVersionsAndReadOnlyCapabilities() throws {
        let file = try snapshot(identifier: "file", kind: .file, size: 5)
        let mapped = MacosFileProviderItem(snapshot: file)
        XCTAssertEqual(mapped.itemIdentifier.rawValue, "file")
        XCTAssertEqual(mapped.parentItemIdentifier, .rootContainer)
        XCTAssertEqual(mapped.filename, "hello.txt")
        XCTAssertEqual(mapped.contentType, .plainText)
        XCTAssertEqual(mapped.documentSize, 5)
        XCTAssertTrue(mapped.capabilities.contains(.allowsReading))
        XCTAssertFalse(mapped.capabilities.contains(.allowsWriting))
        XCTAssertEqual(mapped.itemVersion.contentVersion, Data("content-v1".utf8))
        XCTAssertEqual(mapped.itemVersion.metadataVersion, Data("metadata-v1".utf8))

        let directory = MacosFileProviderItem(
            snapshot: try snapshot(identifier: "directory", kind: .directory, size: 0)
        )
        XCTAssertEqual(directory.contentType, .folder)
        XCTAssertNil(directory.documentSize)
        XCTAssertTrue(directory.capabilities.contains(.allowsContentEnumerating))
    }

    func testSnapshotAndPageRejectEveryNativeBoundaryViolation() throws {
        for filename in ["", "bad/name", "bad\0name"] {
            XCTAssertThrowsError(
                try MacosCloudItemSnapshot(
                    identifier: "item",
                    parentIdentifier: NSFileProviderItemIdentifier.rootContainer.rawValue,
                    filename: filename,
                    kind: .file,
                    size: 0,
                    metadataVersion: Data("m".utf8),
                    contentVersion: Data("c".utf8),
                    contentTypeIdentifier: UTType.data.identifier
                )
            )
        }
        for count in [0, 129] {
            XCTAssertThrowsError(
                try MacosCloudItemSnapshot(
                    identifier: "item",
                    parentIdentifier: NSFileProviderItemIdentifier.rootContainer.rawValue,
                    filename: "item",
                    kind: .file,
                    size: 0,
                    metadataVersion: Data(repeating: 1, count: count),
                    contentVersion: Data("c".utf8),
                    contentTypeIdentifier: UTType.data.identifier
                )
            )
        }
        XCTAssertThrowsError(
            try MacosEnumerationPage(items: [], nextPage: Data(repeating: 1, count: 501))
        )
    }

    func testEveryPortableErrorMapsToAnAcceptedFileProviderOrCocoaDomain() {
        for code in MacosBridgeErrorCode.allCases where code != .success {
            let error = MacosFileProviderErrorMapper.error(for: code)
            XCTAssertNotNil(error)
            XCTAssertTrue(
                error?.domain == NSFileProviderErrorDomain || error?.domain == NSCocoaErrorDomain
            )
        }
        XCTAssertNil(MacosFileProviderErrorMapper.error(for: .success))

        let wrapped = MacosFileProviderErrorMapper.normalize(
            NSError(domain: "FixtureDomain", code: 7)
        )
        XCTAssertEqual(wrapped.domain, NSCocoaErrorDomain)
        XCTAssertEqual(wrapped.code, NSXPCConnectionReplyInvalid)
        XCTAssertNotNil(wrapped.userInfo[NSUnderlyingErrorKey])
    }

    func testItemCancellationWinsRaceAndReleasesLeaseExactlyOnce() throws {
        let source = DelayedSource()
        let session = FakeSession()
        let runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: session,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )
        var completions = 0
        var terminalError: NSError?
        let cancelled = expectation(description: "item cancellation completes")
        let progress = runtime.item(for: NSFileProviderItemIdentifier("file")) { _, error in
            completions += 1
            terminalError = error as NSError?
            cancelled.fulfill()
        }
        progress.cancel()
        wait(for: [cancelled], timeout: 1)
        source.completeItem(.success(try snapshot(identifier: "file", kind: .file, size: 5)))

        XCTAssertEqual(completions, 1)
        XCTAssertEqual(terminalError?.domain, NSCocoaErrorDomain)
        XCTAssertEqual(terminalError?.code, NSUserCancelledError)
        XCTAssertEqual(session.accepted, 1)
        XCTAssertEqual(session.released, 1)
        XCTAssertTrue(source.itemCancellation.cancelled)
    }

    func testFetchWritesExactTemporaryFileAndRejectsWrongRequestedVersion() throws {
        let file = try snapshot(identifier: "file", kind: .file, size: 5)
        let source = try ImmediateSource(
            item: file,
            page: try MacosEnumerationPage(items: [file], nextPage: nil),
            content: try MacosFetchedContent(item: file, bytes: Data("hello".utf8))
        )
        let session = FakeSession()
        let runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: session,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )
        var fetchedURL: URL?
        var fetchedItem: NSFileProviderItem?
        var fetchedError: Error?
        let progress = runtime.fetchContents(
            for: NSFileProviderItemIdentifier("file"),
            requestedVersion: NSFileProviderItemVersion(
                contentVersion: file.contentVersion,
                metadataVersion: file.metadataVersion
            )
        ) { url, item, error in
            fetchedURL = url
            fetchedItem = item
            fetchedError = error
        }
        XCTAssertEqual(progress.completedUnitCount, 1)
        XCTAssertNil(fetchedError)
        XCTAssertEqual(try fetchedURL.map { try Data(contentsOf: $0) }, Data("hello".utf8))
        XCTAssertEqual(fetchedItem?.itemIdentifier.rawValue, "file")
        XCTAssertEqual(session.released, 1)

        var mismatchError: NSError?
        _ = runtime.fetchContents(
            for: NSFileProviderItemIdentifier("file"),
            requestedVersion: NSFileProviderItemVersion(
                contentVersion: Data("old".utf8),
                metadataVersion: file.metadataVersion
            )
        ) { _, _, error in mismatchError = error as NSError? }
        XCTAssertEqual(mismatchError?.domain, NSFileProviderErrorDomain)
        XCTAssertEqual(mismatchError?.code, NSFileProviderError.Code.versionNoLongerAvailable.rawValue)
        XCTAssertEqual(session.released, 2)
    }

    func testEnumerationUsesOpaquePageAndInvalidationRejectsLaterWork() throws {
        let file = try snapshot(identifier: "file", kind: .file, size: 5)
        let page = try MacosEnumerationPage(items: [file], nextPage: Data("next".utf8))
        let source = try ImmediateSource(
            item: file,
            page: page,
            content: try MacosFetchedContent(item: file, bytes: Data("hello".utf8))
        )
        let session = FakeSession()
        let runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: session,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )
        let enumerator = try XCTUnwrap(
            runtime.enumerator(for: .rootContainer)
                as? MacosReadOnlyFileProviderEnumerator
        )
        let observer = EnumerationObserver()
        enumerator.enumerateItems(
            for: observer,
            startingAt: NSFileProviderPage.initialPageSortedByName as NSFileProviderPage
        )
        XCTAssertNil(source.lastPage)
        XCTAssertEqual(observer.items.map(\.itemIdentifier.rawValue), ["file"])
        XCTAssertEqual(observer.nextPage.map { $0 as NSData as Data }, Data("next".utf8))
        XCTAssertEqual(session.released, 1)

        runtime.invalidate()
        XCTAssertTrue(session.closing)
        XCTAssertTrue(session.disconnected)
        XCTAssertThrowsError(
            try runtime.enumerator(for: .rootContainer)
        )
    }

    func testChangeEnumerationPreservesOpaqueAnchorUpdatesDeletesAndMoreComing() throws {
        let file = try snapshot(identifier: "file", kind: .file, size: 5)
        let currentAnchor = try MacosSyncAnchor(bytes: Data("anchor-v2".utf8))
        let changeBatch = try MacosChangeBatch(
            updatedItems: [file],
            deletedItemIdentifiers: ["removed"],
            syncAnchor: currentAnchor,
            moreComing: true
        )
        let source = try ImmediateSource(
            item: file,
            page: MacosEnumerationPage(items: [], nextPage: nil),
            content: MacosFetchedContent(item: file, bytes: Data("hello".utf8)),
            currentAnchor: currentAnchor,
            changeBatch: changeBatch
        )
        let session = FakeSession()
        let runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: session,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )
        let enumerator = try XCTUnwrap(
            runtime.enumerator(for: .workingSet) as? MacosReadOnlyFileProviderEnumerator
        )
        var reportedAnchor: Data?
        enumerator.currentSyncAnchor { reportedAnchor = $0.map { $0 as NSData as Data } }
        XCTAssertEqual(reportedAnchor, currentAnchor.bytes)

        let observer = ChangeObserver()
        enumerator.enumerateChanges(
            for: observer,
            from: Data("anchor-v1".utf8) as NSData as NSFileProviderSyncAnchor
        )

        XCTAssertEqual(source.lastChangeAnchor?.bytes, Data("anchor-v1".utf8))
        XCTAssertEqual(observer.updatedItems.map(\.itemIdentifier.rawValue), ["file"])
        XCTAssertEqual(observer.deletedIdentifiers.map(\.rawValue), ["removed"])
        XCTAssertEqual(observer.syncAnchor.map { $0 as NSData as Data }, currentAnchor.bytes)
        XCTAssertEqual(observer.moreComing, true)
        XCTAssertNil(observer.error)
        XCTAssertEqual(session.released, 1)
    }

    private func snapshot(
        identifier: String,
        kind: MacosCloudItemKind,
        size: UInt64
    ) throws -> MacosCloudItemSnapshot {
        try MacosCloudItemSnapshot(
            identifier: identifier,
            parentIdentifier: NSFileProviderItemIdentifier.rootContainer.rawValue,
            filename: kind == .file ? "hello.txt" : "folder",
            kind: kind,
            size: size,
            metadataVersion: Data("metadata-v1".utf8),
            contentVersion: Data("content-v1".utf8),
            contentTypeIdentifier: kind == .file ? UTType.plainText.identifier : UTType.folder.identifier
        )
    }

}

private final class FakeLease: MacosBridgeRequestLease {
    let generation: UInt64
    private let releaseAction: () -> Void
    private let lock = NSLock()
    private var didRelease = false

    init(generation: UInt64, releaseAction: @escaping () -> Void) {
        self.generation = generation
        self.releaseAction = releaseAction
    }

    func release() {
        lock.lock()
        guard !didRelease else {
            lock.unlock()
            return
        }
        didRelease = true
        lock.unlock()
        releaseAction()
    }
}

private final class FakeSession: MacosBridgeSession {
    let generation: UInt64 = 1
    var accepted = 0
    var released = 0
    var closing = false
    var disconnected = false

    func beginRequest() throws -> any MacosBridgeRequestLease {
        guard !closing else { throw MacosBridgeFailure(code: .providerNotFound) }
        accepted += 1
        return FakeLease(generation: generation) { self.released += 1 }
    }

    func beginClosing() { closing = true }
    func markDisconnected() { disconnected = true }
}

private final class Cancellation: MacosCancellable {
    var cancelled = false
    func cancel() { cancelled = true }
}

private final class DelayedSource: MacosCloudFilesDataSource {
    let itemCancellation = Cancellation()
    private var itemCompletion: ((Result<MacosCloudItemSnapshot, Error>) -> Void)?

    func currentSyncAnchor(containerIdentifier _: String) -> MacosSyncAnchor { .initial }

    func item(
        for _: String,
        completion: @escaping (Result<MacosCloudItemSnapshot, Error>) -> Void
    ) -> any MacosCancellable {
        itemCompletion = completion
        return itemCancellation
    }

    func completeItem(_ result: Result<MacosCloudItemSnapshot, Error>) {
        itemCompletion?(result)
    }

    func enumerate(
        containerIdentifier _: String,
        page _: Data?,
        completion _: @escaping (Result<MacosEnumerationPage, Error>) -> Void
    ) -> any MacosCancellable {
        MacosNoopCancellation()
    }

    func enumerateChanges(
        containerIdentifier _: String,
        from _: MacosSyncAnchor,
        completion: @escaping (Result<MacosChangeBatch, Error>) -> Void
    ) -> any MacosCancellable {
        do {
            completion(
                .success(
                    try MacosChangeBatch(
                        updatedItems: [],
                        deletedItemIdentifiers: [],
                        syncAnchor: .initial,
                        moreComing: false
                    )
                )
            )
        } catch {
            completion(.failure(error))
        }
        return MacosNoopCancellation()
    }

    func fetchContents(
        for _: String,
        requestedContentVersion _: Data?,
        completion _: @escaping (Result<MacosFetchedContent, Error>) -> Void
    ) -> any MacosCancellable {
        MacosNoopCancellation()
    }
}

private final class ImmediateSource: MacosCloudFilesDataSource {
    let storedItem: MacosCloudItemSnapshot
    let storedPage: MacosEnumerationPage
    let storedContent: MacosFetchedContent
    let storedCurrentAnchor: MacosSyncAnchor
    let storedChangeBatch: MacosChangeBatch
    var lastPage: Data?
    var lastChangeAnchor: MacosSyncAnchor?

    init(
        item: MacosCloudItemSnapshot,
        page: MacosEnumerationPage,
        content: MacosFetchedContent,
        currentAnchor: MacosSyncAnchor = .initial,
        changeBatch: MacosChangeBatch? = nil
    ) throws {
        storedItem = item
        storedPage = page
        storedContent = content
        storedCurrentAnchor = currentAnchor
        storedChangeBatch = try changeBatch ?? MacosChangeBatch(
            updatedItems: [],
            deletedItemIdentifiers: [],
            syncAnchor: currentAnchor,
            moreComing: false
        )
    }

    func currentSyncAnchor(containerIdentifier _: String) -> MacosSyncAnchor {
        storedCurrentAnchor
    }

    func item(
        for _: String,
        completion: @escaping (Result<MacosCloudItemSnapshot, Error>) -> Void
    ) -> any MacosCancellable {
        completion(.success(storedItem))
        return MacosNoopCancellation()
    }

    func enumerate(
        containerIdentifier _: String,
        page: Data?,
        completion: @escaping (Result<MacosEnumerationPage, Error>) -> Void
    ) -> any MacosCancellable {
        lastPage = page
        completion(.success(storedPage))
        return MacosNoopCancellation()
    }

    func enumerateChanges(
        containerIdentifier _: String,
        from syncAnchor: MacosSyncAnchor,
        completion: @escaping (Result<MacosChangeBatch, Error>) -> Void
    ) -> any MacosCancellable {
        lastChangeAnchor = syncAnchor
        completion(.success(storedChangeBatch))
        return MacosNoopCancellation()
    }

    func fetchContents(
        for _: String,
        requestedContentVersion _: Data?,
        completion: @escaping (Result<MacosFetchedContent, Error>) -> Void
    ) -> any MacosCancellable {
        completion(.success(storedContent))
        return MacosNoopCancellation()
    }
}

private final class EnumerationObserver: NSObject, NSFileProviderEnumerationObserver {
    var items: [NSFileProviderItem] = []
    var nextPage: NSFileProviderPage?
    var error: Error?

    func didEnumerate(_ updatedItems: [NSFileProviderItem]) {
        items.append(contentsOf: updatedItems)
    }

    func finishEnumerating(upTo nextPage: NSFileProviderPage?) {
        self.nextPage = nextPage
    }

    func finishEnumeratingWithError(_ error: Error) {
        self.error = error
    }
}

private final class ChangeObserver: NSObject, NSFileProviderChangeObserver {
    var updatedItems: [NSFileProviderItem] = []
    var deletedIdentifiers: [NSFileProviderItemIdentifier] = []
    var syncAnchor: NSFileProviderSyncAnchor?
    var moreComing: Bool?
    var error: Error?

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
        syncAnchor = anchor
        self.moreComing = moreComing
    }

    func finishEnumeratingWithError(_ error: Error) {
        self.error = error
    }
}
