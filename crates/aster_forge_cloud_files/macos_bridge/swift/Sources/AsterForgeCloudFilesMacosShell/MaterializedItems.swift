import FileProvider
import Foundation
import UniformTypeIdentifiers

public struct MacosMaterializedSetSnapshot: Equatable, Sendable {
    public let directoryIdentifiers: Set<String>
    public let syncAnchor: MacosSyncAnchor

    public init(
        directoryIdentifiers: Set<String>,
        syncAnchor: MacosSyncAnchor
    ) throws {
        guard directoryIdentifiers.allSatisfy({ !$0.isEmpty }) else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        self.directoryIdentifiers = directoryIdentifiers
        self.syncAnchor = syncAnchor
    }
}

public protocol MacosMaterializedSetPersisting: AnyObject {
    func load() throws -> MacosMaterializedSetSnapshot
    func replace(with snapshot: MacosMaterializedSetSnapshot) throws
}

public final class MacosFileMaterializedSetStore: MacosMaterializedSetPersisting {
    private struct Record: Codable {
        let schemaVersion: UInt8
        let directoryIdentifiers: [String]
        let syncAnchor: Data
    }

    private static let schemaVersion: UInt8 = 1

    private let fileURL: URL
    private let fileManager: FileManager
    private let lock = NSLock()

    public init(
        directory: URL,
        filename: String = "materialized-set-v1.json",
        fileManager: FileManager = .default
    ) throws {
        guard directory.isFileURL,
              !filename.isEmpty,
              filename == URL(fileURLWithPath: filename).lastPathComponent
        else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        do {
            try fileManager.createDirectory(
                at: directory,
                withIntermediateDirectories: true
            )
        } catch {
            throw MacosBridgeFailure(code: .internal)
        }
        fileURL = directory.appendingPathComponent(filename, isDirectory: false)
        self.fileManager = fileManager
    }

    public func load() throws -> MacosMaterializedSetSnapshot {
        lock.lock()
        defer { lock.unlock() }
        return try loadUnlocked()
    }

    public func replace(with snapshot: MacosMaterializedSetSnapshot) throws {
        lock.lock()
        defer { lock.unlock() }
        do {
            let record = Record(
                schemaVersion: Self.schemaVersion,
                directoryIdentifiers: snapshot.directoryIdentifiers.sorted(),
                syncAnchor: snapshot.syncAnchor.bytes
            )
            let data = try JSONEncoder().encode(record)
            try data.write(to: fileURL, options: .atomic)
        } catch let error as MacosBridgeFailure {
            throw error
        } catch {
            throw MacosBridgeFailure(code: .internal)
        }
    }

    private func loadUnlocked() throws -> MacosMaterializedSetSnapshot {
        guard fileManager.fileExists(atPath: fileURL.path) else {
            return try MacosMaterializedSetSnapshot(
                directoryIdentifiers: [],
                syncAnchor: .initial
            )
        }
        do {
            let data = try Data(contentsOf: fileURL)
            let record = try JSONDecoder().decode(Record.self, from: data)
            guard record.schemaVersion == Self.schemaVersion else {
                throw MacosBridgeFailure(code: .internal)
            }
            return try MacosMaterializedSetSnapshot(
                directoryIdentifiers: Set(record.directoryIdentifiers),
                syncAnchor: MacosSyncAnchor(bytes: record.syncAnchor)
            )
        } catch {
            let quarantineURL = fileURL
                .deletingPathExtension()
                .appendingPathExtension("corrupt-\(UUID().uuidString).json")
            do {
                try fileManager.moveItem(at: fileURL, to: quarantineURL)
                return try MacosMaterializedSetSnapshot(
                    directoryIdentifiers: [],
                    syncAnchor: .initial
                )
            } catch {
                throw MacosBridgeFailure(code: .internal)
            }
        }
    }
}

public protocol MacosMaterializedItemsReading: AnyObject {
    @discardableResult
    func read(
        completion: @escaping (Result<MacosMaterializedSetSnapshot, Error>) -> Void
    ) -> any MacosCancellable
}

public final class MacosFileProviderMaterializedItemsReader: MacosMaterializedItemsReading {
    private let makeEnumerator: () -> NSFileProviderEnumerator

    public convenience init(manager: NSFileProviderManager) {
        self.init(makeEnumerator: manager.enumeratorForMaterializedItems)
    }

    public init(makeEnumerator: @escaping () -> NSFileProviderEnumerator) {
        self.makeEnumerator = makeEnumerator
    }

    @discardableResult
    public func read(
        completion: @escaping (Result<MacosMaterializedSetSnapshot, Error>) -> Void
    ) -> any MacosCancellable {
        let operation = MacosMaterializedItemsReadOperation(
            enumerator: makeEnumerator(),
            completion: completion
        )
        operation.start()
        return operation
    }
}

private final class MacosMaterializedItemsReadOperation: NSObject,
    MacosCancellable,
    NSFileProviderEnumerationObserver,
    NSFileProviderChangeObserver
{
    private let enumerator: NSFileProviderEnumerator
    private let lock = NSLock()
    private var directoryIdentifiers: Set<String> = []
    private var capturedAnchor: NSFileProviderSyncAnchor?
    private var completion: ((Result<MacosMaterializedSetSnapshot, Error>) -> Void)?
    private var seenPages: Set<Data> = []
    private var seenChangeAnchors: Set<Data> = []

    init(
        enumerator: NSFileProviderEnumerator,
        completion: @escaping (Result<MacosMaterializedSetSnapshot, Error>) -> Void
    ) {
        self.enumerator = enumerator
        self.completion = completion
    }

    func start() {
        guard let currentSyncAnchor = enumerator.currentSyncAnchor,
              enumerator.enumerateChanges != nil
        else {
            finish(.failure(MacosBridgeFailure(code: .notSupported)))
            return
        }
        currentSyncAnchor { anchor in
            guard let anchor else {
                self.finish(.failure(MacosBridgeFailure(code: .internal)))
                return
            }
            self.lock.lock()
            guard self.completion != nil else {
                self.lock.unlock()
                return
            }
            self.capturedAnchor = anchor
            self.seenChangeAnchors.insert(anchor as NSData as Data)
            self.lock.unlock()
            self.schedule {
                self.enumerator.enumerateItems(
                    for: self,
                    startingAt: Data() as NSData as NSFileProviderPage
                )
            }
        }
    }

    func cancel() {
        finish(.failure(MacosBridgeFailure(code: .cancelled)))
    }

    func didEnumerate(_ updatedItems: [NSFileProviderItem]) {
        lock.lock()
        guard completion != nil else {
            lock.unlock()
            return
        }
        for item in updatedItems where item.contentType?.conforms(to: .folder) == true {
            directoryIdentifiers.insert(item.itemIdentifier.rawValue)
        }
        let exceededLimit = directoryIdentifiers.count > macosMaximumEnumerationItems
        lock.unlock()
        if exceededLimit {
            finish(.failure(MacosBridgeFailure(code: .internal)))
        }
    }

    func finishEnumerating(upTo nextPage: NSFileProviderPage?) {
        guard isActive else { return }
        if let nextPage {
            let pageBytes = nextPage as NSData as Data
            lock.lock()
            let inserted = seenPages.insert(pageBytes).inserted
            lock.unlock()
            guard inserted else {
                finish(.failure(MacosBridgeFailure(code: .internal)))
                return
            }
            schedule {
                self.enumerator.enumerateItems(for: self, startingAt: nextPage)
            }
            return
        }
        lock.lock()
        let anchor = capturedAnchor
        lock.unlock()
        guard let anchor else {
            finish(.failure(MacosBridgeFailure(code: .internal)))
            return
        }
        guard let enumerateChanges = enumerator.enumerateChanges else {
            finish(.failure(MacosBridgeFailure(code: .notSupported)))
            return
        }
        schedule { enumerateChanges(self, anchor) }
    }

    func didUpdate(_ updatedItems: [NSFileProviderItem]) {
        lock.lock()
        guard completion != nil else {
            lock.unlock()
            return
        }
        for item in updatedItems {
            directoryIdentifiers.remove(item.itemIdentifier.rawValue)
            if item.contentType?.conforms(to: .folder) == true {
                directoryIdentifiers.insert(item.itemIdentifier.rawValue)
            }
        }
        let exceededLimit = directoryIdentifiers.count > macosMaximumEnumerationItems
        lock.unlock()
        if exceededLimit {
            finish(.failure(MacosBridgeFailure(code: .internal)))
        }
    }

    func didDeleteItems(withIdentifiers deletedItemIdentifiers: [NSFileProviderItemIdentifier]) {
        lock.lock()
        guard completion != nil else {
            lock.unlock()
            return
        }
        for identifier in deletedItemIdentifiers {
            directoryIdentifiers.remove(identifier.rawValue)
        }
        lock.unlock()
    }

    func finishEnumeratingChanges(
        upTo anchor: NSFileProviderSyncAnchor,
        moreComing: Bool
    ) {
        guard isActive else { return }
        if moreComing {
            guard let enumerateChanges = enumerator.enumerateChanges else {
                finish(.failure(MacosBridgeFailure(code: .notSupported)))
                return
            }
            let anchorBytes = anchor as NSData as Data
            lock.lock()
            let inserted = seenChangeAnchors.insert(anchorBytes).inserted
            lock.unlock()
            guard inserted else {
                finish(.failure(MacosBridgeFailure(code: .internal)))
                return
            }
            schedule { enumerateChanges(self, anchor) }
            return
        }
        do {
            let portableAnchor = try MacosSyncAnchor(bytes: anchor as NSData as Data)
            lock.lock()
            let identifiers = directoryIdentifiers
            lock.unlock()
            finish(
                .success(
                    try MacosMaterializedSetSnapshot(
                        directoryIdentifiers: identifiers,
                        syncAnchor: portableAnchor
                    )
                )
            )
        } catch {
            finish(.failure(error))
        }
    }

    func finishEnumeratingWithError(_ error: Error) {
        finish(.failure(error))
    }

    private var isActive: Bool {
        lock.lock()
        defer { lock.unlock() }
        return completion != nil
    }

    private func finish(_ result: Result<MacosMaterializedSetSnapshot, Error>) {
        lock.lock()
        guard let completion else {
            lock.unlock()
            return
        }
        self.completion = nil
        lock.unlock()
        enumerator.invalidate()
        completion(result)
    }

    private func schedule(_ action: @escaping () -> Void) {
        DispatchQueue.global(qos: .utility).async(execute: action)
    }
}

public final class MacosMaterializedSetTracker {
    private let reader: any MacosMaterializedItemsReading
    private let store: any MacosMaterializedSetPersisting
    private let errorHandler: (Error) -> Void
    private let lock = NSLock()
    private var activeToken: UUID?
    private var activeOperation: (any MacosCancellable)?
    private var refreshAgain = false
    private var completions: [() -> Void] = []
    private var invalidated = false

    public init(
        reader: any MacosMaterializedItemsReading,
        store: any MacosMaterializedSetPersisting,
        errorHandler: @escaping (Error) -> Void = { _ in }
    ) {
        self.reader = reader
        self.store = store
        self.errorHandler = errorHandler
    }

    public func refresh(completion: @escaping () -> Void) {
        lock.lock()
        guard !invalidated else {
            lock.unlock()
            completion()
            return
        }
        completions.append(completion)
        guard activeToken == nil else {
            refreshAgain = true
            lock.unlock()
            return
        }
        let token = UUID()
        activeToken = token
        lock.unlock()
        startRead(token: token)
    }

    public func invalidate() {
        lock.lock()
        guard !invalidated else {
            lock.unlock()
            return
        }
        invalidated = true
        activeToken = nil
        let operation = activeOperation
        activeOperation = nil
        let completions = self.completions
        self.completions.removeAll()
        lock.unlock()
        operation?.cancel()
        completions.forEach { $0() }
    }

    private func startRead(token: UUID) {
        let operation = reader.read { result in
            self.finishRead(result, token: token)
        }
        lock.lock()
        if activeToken == token, !invalidated {
            activeOperation = operation
            lock.unlock()
        } else {
            lock.unlock()
            operation.cancel()
        }
    }

    private func finishRead(
        _ result: Result<MacosMaterializedSetSnapshot, Error>,
        token: UUID
    ) {
        lock.lock()
        guard activeToken == token, !invalidated else {
            lock.unlock()
            return
        }
        lock.unlock()
        let persistedResult = persist(result)
        lock.lock()
        guard activeToken == token, !invalidated else {
            lock.unlock()
            return
        }

        activeOperation = nil
        if refreshAgain {
            refreshAgain = false
            let nextToken = UUID()
            activeToken = nextToken
            lock.unlock()
            if case let .failure(error) = persistedResult {
                errorHandler(error)
            }
            startRead(token: nextToken)
            return
        }
        activeToken = nil
        let completions = self.completions
        self.completions.removeAll()
        lock.unlock()
        if case let .failure(error) = persistedResult {
            errorHandler(error)
        }
        completions.forEach { $0() }
    }

    private func persist(
        _ result: Result<MacosMaterializedSetSnapshot, Error>
    ) -> Result<Void, Error> {
        switch result {
        case let .success(snapshot):
            do {
                try store.replace(with: snapshot)
                return .success(())
            } catch {
                return .failure(error)
            }
        case let .failure(error):
            return .failure(error)
        }
    }
}
