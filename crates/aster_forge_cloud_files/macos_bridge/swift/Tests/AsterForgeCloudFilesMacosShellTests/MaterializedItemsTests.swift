import FileProvider
import Foundation
import UniformTypeIdentifiers
import XCTest
@testable import AsterForgeCloudFilesMacosShell

final class MaterializedItemsTests: XCTestCase {
    func testFileStoreStartsEmptyAndRoundTripsExactSnapshot() throws {
        let directory = temporaryDirectory()
        let store = try MacosFileMaterializedSetStore(directory: directory)

        XCTAssertEqual(
            try store.load(),
            try MacosMaterializedSetSnapshot(directoryIdentifiers: [], syncAnchor: .initial)
        )

        let snapshot = try MacosMaterializedSetSnapshot(
            directoryIdentifiers: ["documents", "photos"],
            syncAnchor: MacosSyncAnchor(bytes: Data("anchor-v2".utf8))
        )
        try store.replace(with: snapshot)
        XCTAssertEqual(try store.load(), snapshot)

        let reopened = try MacosFileMaterializedSetStore(directory: directory)
        XCTAssertEqual(try reopened.load(), snapshot)
    }

    func testFileStoreRejectsInvalidPathSnapshotAndCorruptRecord() throws {
        let directory = temporaryDirectory()
        XCTAssertThrowsError(
            try MacosFileMaterializedSetStore(directory: directory, filename: "../state.json")
        )
        XCTAssertThrowsError(
            try MacosMaterializedSetSnapshot(
                directoryIdentifiers: [""],
                syncAnchor: .initial
            )
        )

        let store = try MacosFileMaterializedSetStore(directory: directory)
        try Data("not-json".utf8).write(
            to: directory.appendingPathComponent("materialized-set-v1.json")
        )
        XCTAssertThrowsError(try store.load()) {
            XCTAssertEqual(($0 as? MacosBridgeFailure)?.code, .internal)
        }
    }

    func testReaderReconcilesPagesAndChangesThroughFinalAnchor() throws {
        let directoryA = try item(identifier: "directory-a", kind: .directory)
        let directoryB = try item(identifier: "directory-b", kind: .directory)
        let file = try item(identifier: "file", kind: .file)
        let directoryC = try item(identifier: "directory-c", kind: .directory)
        let directoryD = try item(identifier: "directory-d", kind: .directory)
        let directoryABecameFile = try item(identifier: "directory-a", kind: .file)
        let enumerator = MaterializedEnumerator(
            initialAnchor: Data("anchor-before-list".utf8),
            pages: [[directoryA, file], [directoryB]],
            changeBatches: [
                .init(
                    updatedItems: [directoryABecameFile, directoryC],
                    deletedIdentifiers: [directoryB.itemIdentifier],
                    anchor: Data("anchor-middle".utf8),
                    moreComing: true
                ),
                .init(
                    updatedItems: [directoryD],
                    deletedIdentifiers: [],
                    anchor: Data("anchor-final".utf8),
                    moreComing: false
                ),
            ]
        )
        let reader = MacosFileProviderMaterializedItemsReader { enumerator }
        var result: Result<MacosMaterializedSetSnapshot, Error>?

        _ = reader.read { result = $0 }

        let snapshot = try XCTUnwrap(result).get()
        XCTAssertEqual(snapshot.directoryIdentifiers, ["directory-c", "directory-d"])
        XCTAssertEqual(snapshot.syncAnchor.bytes, Data("anchor-final".utf8))
        XCTAssertEqual(enumerator.receivedPages.first, Data())
        XCTAssertEqual(enumerator.changeAnchors, [
            Data("anchor-before-list".utf8),
            Data("anchor-middle".utf8),
        ])
        XCTAssertEqual(enumerator.invalidateCalls, 1)
    }

    func testReaderRejectsMissingAnchorAndCancellationIgnoresLateCallback() throws {
        let missingAnchor = MaterializedEnumerator(
            initialAnchor: nil,
            pages: [],
            changeBatches: []
        )
        let missingReader = MacosFileProviderMaterializedItemsReader { missingAnchor }
        var missingResult: Result<MacosMaterializedSetSnapshot, Error>?
        _ = missingReader.read { missingResult = $0 }
        XCTAssertEqual(
            try XCTUnwrap(missingResult).failureCode,
            .internal
        )
        XCTAssertEqual(missingAnchor.invalidateCalls, 1)

        let delayed = MaterializedEnumerator(
            initialAnchor: Data("anchor".utf8),
            pages: [[]],
            changeBatches: [],
            delayAnchor: true
        )
        let delayedReader = MacosFileProviderMaterializedItemsReader { delayed }
        var completionCodes: [MacosBridgeErrorCode] = []
        let operation = delayedReader.read { result in
            completionCodes.append(result.failureCode)
        }
        operation.cancel()
        delayed.completeDelayedAnchor()

        XCTAssertEqual(completionCodes, [.cancelled])
        XCTAssertEqual(delayed.enumerateItemCalls, 0)
        XCTAssertEqual(delayed.invalidateCalls, 1)
    }

    func testReaderRejectsEnumeratorWithoutChangeMethods() throws {
        let enumerator = EnumerationOnlyMaterializedEnumerator()
        let reader = MacosFileProviderMaterializedItemsReader { enumerator }
        var result: Result<MacosMaterializedSetSnapshot, Error>?

        _ = reader.read { result = $0 }

        XCTAssertEqual(try XCTUnwrap(result).failureCode, .notSupported)
        XCTAssertEqual(enumerator.enumerateCalls, 0)
        XCTAssertEqual(enumerator.invalidateCalls, 1)
    }

    func testTrackerCoalescesRefreshesAndPersistsOnlyFinalRead() throws {
        let first = try MacosMaterializedSetSnapshot(
            directoryIdentifiers: ["first"],
            syncAnchor: MacosSyncAnchor(bytes: Data("one".utf8))
        )
        let second = try MacosMaterializedSetSnapshot(
            directoryIdentifiers: ["second"],
            syncAnchor: MacosSyncAnchor(bytes: Data("two".utf8))
        )
        let reader = ControlledMaterializedReader()
        let store = MemoryMaterializedStore()
        let tracker = MacosMaterializedSetTracker(reader: reader, store: store)
        var completions = 0

        tracker.refresh { completions += 1 }
        tracker.refresh { completions += 1 }
        XCTAssertEqual(reader.readCalls, 1)

        reader.completeNext(.success(first))
        XCTAssertEqual(reader.readCalls, 2)
        XCTAssertEqual(completions, 0)
        XCTAssertEqual(store.snapshots, [first])

        reader.completeNext(.success(second))
        XCTAssertEqual(completions, 2)
        XCTAssertEqual(store.snapshots, [first, second])
    }

    func testTrackerInvalidationCancelsActiveReadAndRejectsLateSuccess() throws {
        let reader = ControlledMaterializedReader()
        let store = MemoryMaterializedStore()
        let tracker = MacosMaterializedSetTracker(reader: reader, store: store)
        var completions = 0
        let lateSnapshot = try MacosMaterializedSetSnapshot(
            directoryIdentifiers: ["late"],
            syncAnchor: MacosSyncAnchor(bytes: Data("late".utf8))
        )

        tracker.refresh { completions += 1 }
        tracker.invalidate()
        tracker.invalidate()
        reader.completeNext(.success(lateSnapshot))

        XCTAssertEqual(reader.operations.first?.cancelCalls, 1)
        XCTAssertEqual(completions, 1)
        XCTAssertTrue(store.snapshots.isEmpty)
    }

    func testTrackerReportsPersistenceFailureAndStillCompletes() throws {
        let snapshot = try MacosMaterializedSetSnapshot(
            directoryIdentifiers: ["documents"],
            syncAnchor: .initial
        )
        let reader = ControlledMaterializedReader()
        let store = MemoryMaterializedStore(error: MacosBridgeFailure(code: .tryAgain))
        var reportedCode: MacosBridgeErrorCode?
        let tracker = MacosMaterializedSetTracker(
            reader: reader,
            store: store,
            errorHandler: { reportedCode = ($0 as? MacosBridgeFailure)?.code }
        )
        var completed = false

        tracker.refresh { completed = true }
        reader.completeNext(.success(snapshot))

        XCTAssertEqual(reportedCode, .tryAgain)
        XCTAssertTrue(completed)
    }

    func testWorkingSetSignalerUsesInjectedAppleBoundary() {
        let expected = NSError(domain: "signal", code: 7)
        var calls = 0
        let signaler = MacosWorkingSetSignaler { completion in
            calls += 1
            completion(expected)
        }
        var received: NSError?

        signaler.signal { received = $0 as NSError? }

        XCTAssertEqual(calls, 1)
        XCTAssertEqual(received, expected)
    }

    private func item(
        identifier: String,
        kind: MacosCloudItemKind
    ) throws -> NSFileProviderItem {
        MacosFileProviderItem(
            snapshot: try MacosCloudItemSnapshot(
                identifier: identifier,
                parentIdentifier: NSFileProviderItemIdentifier.rootContainer.rawValue,
                filename: "\(identifier).item",
                kind: kind,
                size: 0,
                metadataVersion: Data("metadata".utf8),
                contentVersion: Data("content".utf8),
                contentTypeIdentifier: kind == .directory
                    ? UTType.folder.identifier
                    : UTType.data.identifier
            )
        )
    }

    private func temporaryDirectory() -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("aster-forge-materialized-tests-\(UUID().uuidString)")
        addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
        return directory
    }
}

private struct MaterializedChangeBatch {
    let updatedItems: [NSFileProviderItem]
    let deletedIdentifiers: [NSFileProviderItemIdentifier]
    let anchor: Data
    let moreComing: Bool
}

private final class MaterializedEnumerator: NSObject, NSFileProviderEnumerator {
    private let initialAnchor: Data?
    private let pages: [[NSFileProviderItem]]
    private let changeBatches: [MaterializedChangeBatch]
    private let delayAnchor: Bool
    private var delayedAnchorCompletion: ((NSFileProviderSyncAnchor?) -> Void)?
    private var pageIndex = 0
    private var changeIndex = 0
    private(set) var receivedPages: [Data] = []
    private(set) var changeAnchors: [Data] = []
    private(set) var enumerateItemCalls = 0
    private(set) var invalidateCalls = 0

    init(
        initialAnchor: Data?,
        pages: [[NSFileProviderItem]],
        changeBatches: [MaterializedChangeBatch],
        delayAnchor: Bool = false
    ) {
        self.initialAnchor = initialAnchor
        self.pages = pages
        self.changeBatches = changeBatches
        self.delayAnchor = delayAnchor
    }

    func invalidate() {
        invalidateCalls += 1
    }

    func currentSyncAnchor(
        completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void
    ) {
        if delayAnchor {
            delayedAnchorCompletion = completionHandler
        } else {
            completionHandler(initialAnchor.map { $0 as NSData as NSFileProviderSyncAnchor })
        }
    }

    func enumerateItems(
        for observer: NSFileProviderEnumerationObserver,
        startingAt page: NSFileProviderPage
    ) {
        enumerateItemCalls += 1
        receivedPages.append(page as NSData as Data)
        guard pageIndex < pages.count else {
            observer.finishEnumerating(upTo: nil)
            return
        }
        observer.didEnumerate(pages[pageIndex])
        pageIndex += 1
        let nextPage = pageIndex < pages.count
            ? Data("page-\(pageIndex)".utf8) as NSData as NSFileProviderPage
            : nil
        observer.finishEnumerating(upTo: nextPage)
    }

    func enumerateChanges(
        for observer: NSFileProviderChangeObserver,
        from syncAnchor: NSFileProviderSyncAnchor
    ) {
        changeAnchors.append(syncAnchor as NSData as Data)
        guard changeIndex < changeBatches.count else {
            observer.finishEnumeratingChanges(upTo: syncAnchor, moreComing: false)
            return
        }
        let batch = changeBatches[changeIndex]
        changeIndex += 1
        observer.didUpdate(batch.updatedItems)
        observer.didDeleteItems(withIdentifiers: batch.deletedIdentifiers)
        observer.finishEnumeratingChanges(
            upTo: batch.anchor as NSData as NSFileProviderSyncAnchor,
            moreComing: batch.moreComing
        )
    }

    func completeDelayedAnchor() {
        delayedAnchorCompletion?(
            initialAnchor.map { $0 as NSData as NSFileProviderSyncAnchor }
        )
    }
}

private final class EnumerationOnlyMaterializedEnumerator: NSObject, NSFileProviderEnumerator {
    private(set) var enumerateCalls = 0
    private(set) var invalidateCalls = 0

    func invalidate() {
        invalidateCalls += 1
    }

    func enumerateItems(
        for observer: NSFileProviderEnumerationObserver,
        startingAt _: NSFileProviderPage
    ) {
        enumerateCalls += 1
        observer.finishEnumerating(upTo: nil)
    }
}

private final class ControlledOperation: MacosCancellable {
    private(set) var cancelCalls = 0
    func cancel() { cancelCalls += 1 }
}

private final class ControlledMaterializedReader: MacosMaterializedItemsReading {
    private var completions: [
        (Result<MacosMaterializedSetSnapshot, Error>) -> Void
    ] = []
    private(set) var readCalls = 0
    private(set) var operations: [ControlledOperation] = []

    func read(
        completion: @escaping (Result<MacosMaterializedSetSnapshot, Error>) -> Void
    ) -> any MacosCancellable {
        readCalls += 1
        completions.append(completion)
        let operation = ControlledOperation()
        operations.append(operation)
        return operation
    }

    func completeNext(_ result: Result<MacosMaterializedSetSnapshot, Error>) {
        guard !completions.isEmpty else { return }
        completions.removeFirst()(result)
    }
}

private final class MemoryMaterializedStore: MacosMaterializedSetPersisting {
    private let error: Error?
    private(set) var snapshots: [MacosMaterializedSetSnapshot] = []

    init(error: Error? = nil) {
        self.error = error
    }

    func load() throws -> MacosMaterializedSetSnapshot {
        if let error { throw error }
        return try snapshots.last ?? MacosMaterializedSetSnapshot(
            directoryIdentifiers: [],
            syncAnchor: .initial
        )
    }

    func replace(with snapshot: MacosMaterializedSetSnapshot) throws {
        if let error { throw error }
        snapshots.append(snapshot)
    }
}

private extension Result where Success == MacosMaterializedSetSnapshot, Failure == Error {
    var failureCode: MacosBridgeErrorCode {
        switch self {
        case .success:
            .success
        case let .failure(error):
            (error as? MacosBridgeFailure)?.code ?? .internal
        }
    }
}
