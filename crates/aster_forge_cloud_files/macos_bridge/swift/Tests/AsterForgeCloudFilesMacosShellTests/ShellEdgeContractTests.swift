import FileProvider
import Foundation
import UniformTypeIdentifiers
import XCTest
@testable import AsterForgeCloudFilesMacosShell

final class ShellEdgeContractTests: XCTestCase {
    func testSnapshotVersionAndEnumerationPageLimitsAreInclusive() throws {
        for count in [1, 128] {
            let snapshot = try makeSnapshot(
                identifier: "file-\(count)",
                metadataVersion: Data(repeating: 1, count: count),
                contentVersion: Data(repeating: 2, count: count)
            )
            XCTAssertEqual(snapshot.metadataVersion.count, count)
            XCTAssertEqual(snapshot.contentVersion.count, count)
        }

        for count in [0, 129] {
            XCTAssertThrowsError(
                try makeSnapshot(
                    identifier: "invalid-\(count)",
                    metadataVersion: Data(repeating: 1, count: count)
                )
            ) { XCTAssertEqual(($0 as? MacosBridgeFailure)?.code, .invalidArgument) }
            XCTAssertThrowsError(
                try makeSnapshot(
                    identifier: "invalid-content-\(count)",
                    contentVersion: Data(repeating: 1, count: count)
                )
            ) { XCTAssertEqual(($0 as? MacosBridgeFailure)?.code, .invalidArgument) }
        }

        for count in [0, 499, 500] {
            let page = try MacosEnumerationPage(
                items: [],
                nextPage: Data(repeating: 3, count: count)
            )
            XCTAssertEqual(page.nextPage?.count, count)
        }
        XCTAssertThrowsError(
            try MacosEnumerationPage(items: [], nextPage: Data(repeating: 3, count: 501))
        ) { XCTAssertEqual(($0 as? MacosBridgeFailure)?.code, .invalidArgument) }

        for count in [0, 499, 500] {
            let anchor = try MacosSyncAnchor(bytes: Data(repeating: 4, count: count))
            XCTAssertEqual(anchor.bytes.count, count)
        }
        XCTAssertThrowsError(
            try MacosSyncAnchor(bytes: Data(repeating: 4, count: 501))
        ) { XCTAssertEqual(($0 as? MacosBridgeFailure)?.code, .invalidArgument) }
    }

    func testChangeBatchRejectsDuplicateEmptyAndConflictingIdentifiers() throws {
        let item = try makeSnapshot(identifier: "item")
        let duplicate = try makeSnapshot(identifier: "item")
        let anchor = try MacosSyncAnchor(bytes: Data("anchor".utf8))

        XCTAssertThrowsError(
            try MacosChangeBatch(
                updatedItems: [item, duplicate],
                deletedItemIdentifiers: [],
                syncAnchor: anchor,
                moreComing: false
            )
        )
        XCTAssertThrowsError(
            try MacosChangeBatch(
                updatedItems: [],
                deletedItemIdentifiers: ["", "removed"],
                syncAnchor: anchor,
                moreComing: false
            )
        )
        XCTAssertThrowsError(
            try MacosChangeBatch(
                updatedItems: [],
                deletedItemIdentifiers: ["removed", "removed"],
                syncAnchor: anchor,
                moreComing: false
            )
        )
        XCTAssertThrowsError(
            try MacosChangeBatch(
                updatedItems: [item],
                deletedItemIdentifiers: [item.identifier],
                syncAnchor: anchor,
                moreComing: false
            )
        )
    }

    func testSnapshotRejectsInvalidIdentityTypeAndDirectorySize() {
        XCTAssertThrowsError(try makeSnapshot(identifier: ""))
        XCTAssertThrowsError(try makeSnapshot(parentIdentifier: ""))
        XCTAssertThrowsError(try makeSnapshot(contentTypeIdentifier: ""))
        XCTAssertThrowsError(try makeSnapshot(kind: .directory, size: 1))
    }

    func testFetchedContentRequiresAFileAndExactSize() throws {
        let file = try makeSnapshot(size: 5)
        XCTAssertNoThrow(try MacosFetchedContent(item: file, bytes: Data("hello".utf8)))
        XCTAssertThrowsError(
            try MacosFetchedContent(item: file, bytes: Data("four".utf8))
        ) { XCTAssertEqual(($0 as? MacosBridgeFailure)?.code, .internal) }

        let directory = try makeSnapshot(kind: .directory, size: 0)
        XCTAssertThrowsError(try MacosFetchedContent(item: directory, bytes: Data())) {
            XCTAssertEqual(($0 as? MacosBridgeFailure)?.code, .internal)
        }
    }

    func testEveryPortableErrorMapsToItsExactAppleError() {
        let expected: [(MacosBridgeErrorCode, String, Int)] = [
            (.notFound, NSFileProviderErrorDomain, NSFileProviderError.Code.noSuchItem.rawValue),
            (
                .notAuthenticated,
                NSFileProviderErrorDomain,
                NSFileProviderError.Code.notAuthenticated.rawValue
            ),
            (.permissionDenied, NSCocoaErrorDomain, NSFileReadNoPermissionError),
            (
                .versionOutOfDate,
                NSFileProviderErrorDomain,
                NSFileProviderError.Code.versionNoLongerAvailable.rawValue
            ),
            (
                .tryAgain,
                NSFileProviderErrorDomain,
                NSFileProviderError.Code.serverUnreachable.rawValue
            ),
            (.notSupported, NSCocoaErrorDomain, NSFeatureUnsupportedError),
            (.invalidArgument, NSCocoaErrorDomain, NSFileWriteInvalidFileNameError),
            (
                .syncAnchorExpired,
                NSFileProviderErrorDomain,
                NSFileProviderError.Code.syncAnchorExpired.rawValue
            ),
            (.cancelled, NSCocoaErrorDomain, NSUserCancelledError),
            (
                .providerNotFound,
                NSFileProviderErrorDomain,
                NSFileProviderError.Code.providerNotFound.rawValue
            ),
            (.`internal`, NSCocoaErrorDomain, NSXPCConnectionReplyInvalid),
        ]

        for (code, domain, rawCode) in expected {
            let error = MacosFileProviderErrorMapper.error(for: code)
            XCTAssertEqual(error?.domain, domain, "wrong domain for \(code)")
            XCTAssertEqual(error?.code, rawCode, "wrong code for \(code)")
        }
    }

    func testSynchronousCompletionThenCancellationStaysCompleted() throws {
        let item = try makeSnapshot(size: 5)
        let source = try EdgeDataSource(item: item)
        let session = EdgeSession()
        let runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: session,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )
        var completions = 0
        var result: NSFileProviderItem?
        var terminalError: Error?

        let progress = runtime.item(for: NSFileProviderItemIdentifier("file")) { item, error in
            completions += 1
            result = item
            terminalError = error
        }
        progress.cancel()

        XCTAssertEqual(completions, 1)
        XCTAssertEqual(result?.itemIdentifier.rawValue, "file")
        XCTAssertNil(terminalError)
        XCTAssertFalse(source.itemCancellation.cancelled)
        XCTAssertEqual(session.released, 1)
    }

    func testTerminalGateAllowsExactlyOneConcurrentWinner() {
        for _ in 0 ..< 100 {
            let gate = MacosTerminalGate()
            let queue = DispatchQueue(label: "terminal-gate", attributes: .concurrent)
            let group = DispatchGroup()
            let lock = NSLock()
            var winners = 0

            for _ in 0 ..< 16 {
                group.enter()
                queue.async {
                    gate.finish {
                        lock.lock()
                        winners += 1
                        lock.unlock()
                    }
                    group.leave()
                }
            }
            group.wait()
            XCTAssertEqual(winners, 1)
        }
    }

    func testSessionFailureAndRuntimeInvalidationAreDeterministic() throws {
        let item = try makeSnapshot(size: 5)
        let source = try EdgeDataSource(item: item)
        let rejectedSession = EdgeSession(beginError: MacosBridgeFailure(code: .tryAgain))
        let rejectedRuntime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: rejectedSession,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )
        var rejection: NSError?
        _ = rejectedRuntime.item(for: NSFileProviderItemIdentifier("file")) { _, error in
            rejection = error as NSError?
        }
        XCTAssertEqual(rejection?.domain, NSFileProviderErrorDomain)
        XCTAssertEqual(rejection?.code, NSFileProviderError.Code.serverUnreachable.rawValue)
        XCTAssertEqual(rejectedSession.accepted, 0)

        let session = EdgeSession()
        let runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: session,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )
        runtime.invalidate()
        runtime.invalidate()
        XCTAssertEqual(session.beginClosingCalls, 1)
        XCTAssertEqual(session.markDisconnectedCalls, 1)

        var invalidatedError: NSError?
        _ = runtime.item(for: NSFileProviderItemIdentifier("file")) { _, error in
            invalidatedError = error as NSError?
        }
        XCTAssertEqual(
            invalidatedError?.code,
            NSFileProviderError.Code.providerNotFound.rawValue
        )
        XCTAssertEqual(session.accepted, 0)
    }

    func testDataSourceFailuresReleaseTheirLeases() throws {
        let item = try makeSnapshot(size: 5)
        let source = try EdgeDataSource(
            item: item,
            itemResult: .failure(MacosBridgeFailure(code: .notFound)),
            fetchedContent: nil
        )
        let session = EdgeSession()
        let runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: session,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )

        var itemError: NSError?
        _ = runtime.item(for: NSFileProviderItemIdentifier("missing")) { _, error in
            itemError = error as NSError?
        }
        XCTAssertEqual(itemError?.code, NSFileProviderError.Code.noSuchItem.rawValue)

        var fetchError: NSError?
        _ = runtime.fetchContents(
            for: NSFileProviderItemIdentifier("file"),
            requestedVersion: nil
        ) { _, _, error in fetchError = error as NSError? }
        XCTAssertEqual(fetchError?.code, NSFileProviderError.Code.noSuchItem.rawValue)
        XCTAssertEqual(session.accepted, 2)
        XCTAssertEqual(session.released, 2)
    }

    func testCancelledFetchRemovesLateTemporaryContent() throws {
        let item = try makeSnapshot(size: 5)
        let content = try MacosFetchedContent(item: item, bytes: Data("hello".utf8))
        let source = try EdgeDataSource(
            item: item,
            fetchedContent: content,
            delayFetch: true
        )
        let session = EdgeSession()
        let runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: session,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )
        var completions = 0
        var terminalError: NSError?
        let cancelled = expectation(description: "fetch cancellation completes")

        let progress = runtime.fetchContents(
            for: NSFileProviderItemIdentifier("file"),
            requestedVersion: nil
        ) { _, _, error in
            completions += 1
            terminalError = error as NSError?
            cancelled.fulfill()
        }
        progress.cancel()
        wait(for: [cancelled], timeout: 1)
        source.completeDelayedFetch()

        XCTAssertEqual(completions, 1)
        XCTAssertEqual(terminalError?.code, NSUserCancelledError)
        XCTAssertTrue(source.fetchCancellation.cancelled)
        XCTAssertEqual(source.discardedURLs, [content.stagingURL])
        XCTAssertEqual(session.released, 1)
    }

    func testOpaqueEnumerationPageRoundTripsAndInvalidationCompletesPendingWork() throws {
        let item = try makeSnapshot(size: 5)
        let page = try MacosEnumerationPage(items: [item], nextPage: nil)
        let source = try EdgeDataSource(item: item, page: page, delayEnumeration: true)
        let session = EdgeSession()
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
        let observer = EdgeEnumerationObserver()
        let opaquePage = Data(repeating: 7, count: 500)

        enumerator.enumerateItems(
            for: observer,
            startingAt: opaquePage as NSData as NSFileProviderPage
        )
        XCTAssertEqual(source.lastEnumerationPage, opaquePage)
        XCTAssertEqual(session.accepted, 1)
        XCTAssertEqual(session.released, 0)

        enumerator.invalidate()
        source.completeDelayedEnumeration()

        XCTAssertTrue(source.enumerationCancellation.cancelled)
        XCTAssertEqual(observer.finishCalls, 1)
        XCTAssertEqual((observer.error as NSError?)?.code, NSUserCancelledError)
        XCTAssertEqual(session.released, 1)
    }

    func testRuntimeInvalidationCancelsPendingEnumeratorOperation() throws {
        let item = try makeSnapshot(size: 5)
        let source = try EdgeDataSource(item: item, delayEnumeration: true)
        let session = EdgeSession()
        let runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: session,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )
        let enumerator = try XCTUnwrap(
            runtime.enumerator(for: .rootContainer) as? MacosReadOnlyFileProviderEnumerator
        )
        let observer = EdgeEnumerationObserver()

        enumerator.enumerateItems(
            for: observer,
            startingAt: NSFileProviderPage.initialPageSortedByName as NSFileProviderPage
        )
        runtime.invalidate()
        source.completeDelayedEnumeration()

        XCTAssertTrue(source.enumerationCancellation.cancelled)
        XCTAssertEqual(observer.finishCalls, 1)
        XCTAssertEqual((observer.error as NSError?)?.code, NSUserCancelledError)
        XCTAssertEqual(session.released, 1)
    }

    func testRuntimeRejectsMismatchedItemAndEnumerationResponses() throws {
        let requested = try makeSnapshot(identifier: "requested", size: 5)
        let wrong = try makeSnapshot(identifier: "wrong", size: 5)
        let source = try EdgeDataSource(
            item: requested,
            itemResult: .success(wrong),
            page: MacosEnumerationPage(items: [wrong], nextPage: nil)
        )
        let session = EdgeSession()
        let runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: session,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )
        var itemError: NSError?

        _ = runtime.item(for: NSFileProviderItemIdentifier("requested")) { _, error in
            itemError = error as NSError?
        }
        XCTAssertEqual(itemError?.domain, NSCocoaErrorDomain)

        let enumerator = try XCTUnwrap(
            runtime.enumerator(for: NSFileProviderItemIdentifier("requested"))
                as? MacosReadOnlyFileProviderEnumerator
        )
        let observer = EdgeEnumerationObserver()
        enumerator.enumerateItems(
            for: observer,
            startingAt: NSFileProviderPage.initialPageSortedByName as NSFileProviderPage
        )
        XCTAssertEqual(observer.finishCalls, 1)
        XCTAssertNotNil(observer.error)
    }

    func testChangeEnumerationInvalidationCancelsBackendAndIgnoresLateCompletion() throws {
        let item = try makeSnapshot(size: 5)
        let anchor = try MacosSyncAnchor(bytes: Data("current".utf8))
        let batch = try MacosChangeBatch(
            updatedItems: [item],
            deletedItemIdentifiers: ["removed"],
            syncAnchor: anchor,
            moreComing: false
        )
        let source = try EdgeDataSource(
            item: item,
            changeResult: .success(batch),
            delayChanges: true
        )
        let session = EdgeSession()
        let runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: session,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )
        let enumerator = try XCTUnwrap(
            runtime.enumerator(for: .workingSet) as? MacosReadOnlyFileProviderEnumerator
        )
        let observer = EdgeChangeObserver()
        let startingAnchor = Data("previous".utf8)

        enumerator.enumerateChanges(
            for: observer,
            from: startingAnchor as NSData as NSFileProviderSyncAnchor
        )
        XCTAssertEqual(source.lastChangeAnchor?.bytes, startingAnchor)
        XCTAssertEqual(session.accepted, 1)
        XCTAssertEqual(session.released, 0)

        enumerator.invalidate()
        source.completeDelayedChanges()

        XCTAssertTrue(source.changeCancellation.cancelled)
        XCTAssertEqual(observer.finishCalls, 1)
        XCTAssertEqual((observer.error as NSError?)?.code, NSUserCancelledError)
        XCTAssertTrue(observer.updatedItems.isEmpty)
        XCTAssertTrue(observer.deletedIdentifiers.isEmpty)
        XCTAssertEqual(session.released, 1)
    }

    func testChangeEnumerationMapsExpiredAnchorAndReleasesLease() throws {
        let item = try makeSnapshot(size: 5)
        let source = try EdgeDataSource(
            item: item,
            changeResult: .failure(MacosBridgeFailure(code: .syncAnchorExpired))
        )
        let session = EdgeSession()
        let runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: session,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )
        let enumerator = try XCTUnwrap(
            runtime.enumerator(for: .workingSet) as? MacosReadOnlyFileProviderEnumerator
        )
        let observer = EdgeChangeObserver()

        enumerator.enumerateChanges(
            for: observer,
            from: Data("expired".utf8) as NSData as NSFileProviderSyncAnchor
        )

        XCTAssertEqual(observer.finishCalls, 1)
        XCTAssertEqual(
            (observer.error as NSError?)?.code,
            NSFileProviderError.Code.syncAnchorExpired.rawValue
        )
        XCTAssertEqual(session.accepted, 1)
        XCTAssertEqual(session.released, 1)
    }

    func testOversizedNativeChangeAnchorIsRejectedBeforeBackendLease() throws {
        let item = try makeSnapshot(size: 5)
        let source = try EdgeDataSource(item: item)
        let session = EdgeSession()
        let runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: source,
            session: session,
            scope: testScope,
            identifierDecoder: TestIdentifierDecoder()
        )
        let enumerator = try XCTUnwrap(
            runtime.enumerator(for: .workingSet) as? MacosReadOnlyFileProviderEnumerator
        )
        let observer = EdgeChangeObserver()

        enumerator.enumerateChanges(
            for: observer,
            from: Data(repeating: 1, count: 501) as NSData as NSFileProviderSyncAnchor
        )

        XCTAssertEqual(observer.finishCalls, 1)
        XCTAssertEqual((observer.error as NSError?)?.code, NSFileWriteInvalidFileNameError)
        XCTAssertEqual(session.accepted, 0)
        XCTAssertEqual(session.released, 0)
        XCTAssertNil(source.lastChangeAnchor)
    }

    private func makeSnapshot(
        identifier: String = "file",
        parentIdentifier: String = NSFileProviderItemIdentifier.rootContainer.rawValue,
        kind: MacosCloudItemKind = .file,
        size: UInt64 = 0,
        metadataVersion: Data = Data("metadata".utf8),
        contentVersion: Data = Data("content".utf8),
        contentTypeIdentifier: String = UTType.data.identifier
    ) throws -> MacosCloudItemSnapshot {
        try MacosCloudItemSnapshot(
            identifier: identifier,
            parentIdentifier: parentIdentifier,
            filename: kind == .file ? "file.txt" : "folder",
            kind: kind,
            size: size,
            metadataVersion: metadataVersion,
            contentVersion: contentVersion,
            contentTypeIdentifier: contentTypeIdentifier
        )
    }
}

private final class EdgeCancellation: MacosCancellable {
    private(set) var cancelled = false
    func cancel() { cancelled = true }
}

private final class EdgeLease: MacosBridgeRequestLease {
    let generation: UInt64
    private let lock = NSLock()
    private var released = false
    private let onRelease: () -> Void

    init(generation: UInt64, onRelease: @escaping () -> Void) {
        self.generation = generation
        self.onRelease = onRelease
    }

    func release() {
        lock.lock()
        guard !released else {
            lock.unlock()
            return
        }
        released = true
        lock.unlock()
        onRelease()
    }
}

private final class EdgeSession: MacosBridgeSession {
    let generation: UInt64 = 9
    private let beginError: Error?
    private(set) var accepted = 0
    private(set) var released = 0
    private(set) var beginClosingCalls = 0
    private(set) var markDisconnectedCalls = 0

    init(beginError: Error? = nil) {
        self.beginError = beginError
    }

    func beginRequest() throws -> any MacosBridgeRequestLease {
        if let beginError { throw beginError }
        accepted += 1
        return EdgeLease(generation: generation) { self.released += 1 }
    }

    func beginClosing() { beginClosingCalls += 1 }
    func markDisconnected() { markDisconnectedCalls += 1 }
}

private final class EdgeDataSource: MacosCloudFilesDataSource {
    let itemCancellation = EdgeCancellation()
    let fetchCancellation = EdgeCancellation()
    let enumerationCancellation = EdgeCancellation()
    let changeCancellation = EdgeCancellation()
    private let itemResult: Result<MacosCloudItemSnapshot, Error>
    private let page: MacosEnumerationPage
    private let fetchedContent: MacosFetchedContent?
    private let delayFetch: Bool
    private let delayEnumeration: Bool
    private let changeResult: Result<MacosChangeBatch, Error>
    private let delayChanges: Bool
    private var fetchCompletion: ((Result<MacosFetchedContent, Error>) -> Void)?
    private var enumerationCompletion: ((Result<MacosEnumerationPage, Error>) -> Void)?
    private var changeCompletion: ((Result<MacosChangeBatch, Error>) -> Void)?
    private(set) var lastEnumerationPage: Data?
    private(set) var lastChangeAnchor: MacosSyncAnchor?
    private(set) var discardedURLs: [URL] = []

    init(
        item: MacosCloudItemSnapshot,
        itemResult: Result<MacosCloudItemSnapshot, Error>? = nil,
        page: MacosEnumerationPage? = nil,
        fetchedContent: MacosFetchedContent? = nil,
        delayFetch: Bool = false,
        delayEnumeration: Bool = false,
        changeResult: Result<MacosChangeBatch, Error>? = nil,
        delayChanges: Bool = false
    ) throws {
        self.itemResult = itemResult ?? .success(item)
        if let page {
            self.page = page
        } else {
            self.page = try MacosEnumerationPage(items: [item], nextPage: nil)
        }
        self.fetchedContent = fetchedContent
        self.delayFetch = delayFetch
        self.delayEnumeration = delayEnumeration
        self.changeResult = try changeResult ?? .success(
            MacosChangeBatch(
                updatedItems: [],
                deletedItemIdentifiers: [],
                syncAnchor: .initial,
                moreComing: false
            )
        )
        self.delayChanges = delayChanges
    }

    func currentSyncAnchor(containerIdentifier _: String) -> MacosSyncAnchor {
        switch changeResult {
        case let .success(batch):
            batch.syncAnchor
        case .failure:
            .initial
        }
    }

    func item(
        for _: String,
        completion: @escaping (Result<MacosCloudItemSnapshot, Error>) -> Void
    ) -> any MacosCancellable {
        completion(itemResult)
        return itemCancellation
    }

    func enumerate(
        containerIdentifier _: String,
        page: Data?,
        completion: @escaping (Result<MacosEnumerationPage, Error>) -> Void
    ) -> any MacosCancellable {
        lastEnumerationPage = page
        if delayEnumeration {
            enumerationCompletion = completion
        } else {
            completion(.success(self.page))
        }
        return enumerationCancellation
    }

    func enumerateChanges(
        containerIdentifier _: String,
        from syncAnchor: MacosSyncAnchor,
        completion: @escaping (Result<MacosChangeBatch, Error>) -> Void
    ) -> any MacosCancellable {
        lastChangeAnchor = syncAnchor
        if delayChanges {
            changeCompletion = completion
        } else {
            completion(changeResult)
        }
        return changeCancellation
    }

    func fetchContents(
        for _: String,
        requestedContentVersion _: Data?,
        completion: @escaping (Result<MacosFetchedContent, Error>) -> Void
    ) -> any MacosCancellable {
        guard let fetchedContent else {
            completion(.failure(MacosBridgeFailure(code: .notFound)))
            return fetchCancellation
        }
        if delayFetch {
            fetchCompletion = completion
        } else {
            completion(.success(fetchedContent))
        }
        return fetchCancellation
    }

    func completeDelayedFetch() {
        guard let fetchedContent else { return }
        fetchCompletion?(.success(fetchedContent))
    }

    func completeDelayedEnumeration() {
        enumerationCompletion?(.success(page))
    }

    func completeDelayedChanges() {
        changeCompletion?(changeResult)
    }

    func discardFetchedContents(at stagingURL: URL) {
        discardedURLs.append(stagingURL)
        try? Foundation.FileManager().removeItem(at: stagingURL)
    }
}

private final class EdgeEnumerationObserver: NSObject, NSFileProviderEnumerationObserver {
    private(set) var finishCalls = 0
    private(set) var error: Error?

    func didEnumerate(_: [NSFileProviderItem]) {}

    func finishEnumerating(upTo _: NSFileProviderPage?) {
        finishCalls += 1
    }

    func finishEnumeratingWithError(_ error: Error) {
        finishCalls += 1
        self.error = error
    }
}

private final class EdgeChangeObserver: NSObject, NSFileProviderChangeObserver {
    private(set) var updatedItems: [NSFileProviderItem] = []
    private(set) var deletedIdentifiers: [NSFileProviderItemIdentifier] = []
    private(set) var finishCalls = 0
    private(set) var error: Error?

    func didUpdate(_ updatedItems: [NSFileProviderItem]) {
        self.updatedItems.append(contentsOf: updatedItems)
    }

    func didDeleteItems(withIdentifiers deletedItemIdentifiers: [NSFileProviderItemIdentifier]) {
        deletedIdentifiers.append(contentsOf: deletedItemIdentifiers)
    }

    func finishEnumeratingChanges(
        upTo _: NSFileProviderSyncAnchor,
        moreComing _: Bool
    ) {
        finishCalls += 1
    }

    func finishEnumeratingWithError(_ error: Error) {
        finishCalls += 1
        self.error = error
    }
}
