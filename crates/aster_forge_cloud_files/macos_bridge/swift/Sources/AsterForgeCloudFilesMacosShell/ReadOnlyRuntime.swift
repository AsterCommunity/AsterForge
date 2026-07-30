import FileProvider
import Foundation

public final class MacosReadOnlyFileProviderRuntime {
    private let dataSource: any MacosCloudFilesDataSource
    private let session: any MacosBridgeSession
    private let validator: MacosBackendResponseValidator
    private let cancellations = MacosCancellationRegistry()
    private let stateLock = NSLock()
    private var invalidated = false

    public init(
        dataSource: any MacosCloudFilesDataSource,
        session: any MacosBridgeSession,
        scope: (namespace: String, root: String),
        identifierDecoder: any MacosPersistentIdentifierDecoding
    ) {
        self.dataSource = dataSource
        self.session = session
        validator = MacosBackendResponseValidator(
            scope: scope,
            identifierDecoder: identifierDecoder
        )
    }

    public func invalidate() {
        stateLock.lock()
        guard !invalidated else {
            stateLock.unlock()
            return
        }
        invalidated = true
        stateLock.unlock()
        cancellations.invalidate()
        session.beginClosing()
        session.markDisconnected()
    }

    public func item(
        for identifier: NSFileProviderItemIdentifier,
        completionHandler: @escaping (NSFileProviderItem?, Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        guard let lease = beginRequest(completion: { completionHandler(nil, $0) }) else {
            return progress
        }
        let terminal = MacosTerminalGate()
        var operationIdentifier: UUID?
        let cancellation = MacosCancellationSlot {
            terminal.finish {
                if let operationIdentifier {
                    self.cancellations.remove(operationIdentifier)
                }
                lease.release()
                completionHandler(nil, MacosFileProviderErrorMapper.error(for: .cancelled))
            }
        }
        guard let insertedIdentifier = cancellations.insert(cancellation) else {
            cancellation.cancel()
            return progress
        }
        operationIdentifier = insertedIdentifier
        progress.cancellationHandler = { cancellation.cancel() }
        let operation = dataSource.item(for: identifier.rawValue) { result in
            terminal.finish {
                cancellation.complete()
                self.cancellations.remove(insertedIdentifier)
                lease.release()
                progress.completedUnitCount = 1
                switch result {
                case let .success(snapshot):
                    do {
                        try self.validator.validateItem(
                            snapshot,
                            requestedIdentifier: identifier.rawValue
                        )
                        completionHandler(MacosFileProviderItem(snapshot: snapshot), nil)
                    } catch {
                        completionHandler(nil, MacosFileProviderErrorMapper.normalize(error))
                    }
                case let .failure(error):
                    completionHandler(nil, MacosFileProviderErrorMapper.normalize(error))
                }
            }
        }
        cancellation.install(operation)
        return progress
    }

    public func fetchContents(
        for identifier: NSFileProviderItemIdentifier,
        requestedVersion: NSFileProviderItemVersion?,
        completionHandler: @escaping (URL?, NSFileProviderItem?, Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        guard let lease = beginRequest(completion: { completionHandler(nil, nil, $0) }) else {
            return progress
        }
        let terminal = MacosTerminalGate()
        var operationIdentifier: UUID?
        let cancellation = MacosCancellationSlot {
            terminal.finish {
                if let operationIdentifier {
                    self.cancellations.remove(operationIdentifier)
                }
                lease.release()
                completionHandler(nil, nil, MacosFileProviderErrorMapper.error(for: .cancelled))
            }
        }
        guard let insertedIdentifier = cancellations.insert(cancellation) else {
            cancellation.cancel()
            return progress
        }
        operationIdentifier = insertedIdentifier
        progress.cancellationHandler = { cancellation.cancel() }
        let requestedContentVersion = requestedVersion?.contentVersion
        let operation = dataSource.fetchContents(
            for: identifier.rawValue,
            requestedContentVersion: requestedContentVersion
        ) { result in
            switch result {
            case let .success(content):
                do {
                    try self.validator.validateFetchedContent(
                        content,
                        requestedIdentifier: identifier.rawValue,
                        requestedContentVersion: requestedContentVersion
                    )
                    if !terminal.finish({
                        cancellation.complete()
                        self.cancellations.remove(insertedIdentifier)
                        lease.release()
                        progress.completedUnitCount = 1
                        completionHandler(
                            content.stagingURL,
                            MacosFileProviderItem(snapshot: content.item),
                            nil
                        )
                    }) {
                        self.dataSource.discardFetchedContents(at: content.stagingURL)
                    }
                } catch {
                    self.dataSource.discardFetchedContents(at: content.stagingURL)
                    terminal.finish {
                        cancellation.complete()
                        self.cancellations.remove(insertedIdentifier)
                        lease.release()
                        completionHandler(nil, nil, MacosFileProviderErrorMapper.normalize(error))
                    }
                }
            case let .failure(error):
                terminal.finish {
                    cancellation.complete()
                    self.cancellations.remove(insertedIdentifier)
                    lease.release()
                    completionHandler(nil, nil, MacosFileProviderErrorMapper.normalize(error))
                }
            }
        }
        cancellation.install(operation)
        return progress
    }

    public func enumerator(
        for containerIdentifier: NSFileProviderItemIdentifier
    ) throws -> NSFileProviderEnumerator {
        guard isAccepting else {
            throw MacosBridgeFailure(code: .providerNotFound)
        }
        return try MacosReadOnlyFileProviderEnumerator(
            dataSource: dataSource,
            session: session,
            containerIdentifier: containerIdentifier,
            runtimeCancellations: cancellations,
            validator: validator
        )
    }

    private var isAccepting: Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return !invalidated
    }

    private func beginRequest(completion: (NSError) -> Void) -> (any MacosBridgeRequestLease)? {
        guard isAccepting else {
            completion(
                MacosFileProviderErrorMapper.normalize(
                    MacosBridgeFailure(code: .providerNotFound)
                )
            )
            return nil
        }
        do {
            return try session.beginRequest()
        } catch {
            completion(MacosFileProviderErrorMapper.normalize(error))
            return nil
        }
    }
}

public final class MacosReadOnlyFileProviderEnumerator: NSObject, NSFileProviderEnumerator {
    private let dataSource: any MacosCloudFilesDataSource
    private let session: any MacosBridgeSession
    private let containerIdentifier: NSFileProviderItemIdentifier
    private let cancellations = MacosCancellationRegistry()
    private let runtimeCancellations: MacosCancellationRegistry
    private let validator: MacosBackendResponseValidator
    private let validationState: MacosEnumerationValidationState

    init(
        dataSource: any MacosCloudFilesDataSource,
        session: any MacosBridgeSession,
        containerIdentifier: NSFileProviderItemIdentifier,
        runtimeCancellations: MacosCancellationRegistry,
        validator: MacosBackendResponseValidator
    ) throws {
        self.dataSource = dataSource
        self.session = session
        self.containerIdentifier = containerIdentifier
        self.runtimeCancellations = runtimeCancellations
        self.validator = validator
        validationState = try validator.makeEnumerationState(
            containerIdentifier: containerIdentifier.rawValue
        )
        super.init()
    }

    public func invalidate() {
        cancellations.invalidate()
    }

    public func enumerateItems(
        for observer: NSFileProviderEnumerationObserver,
        startingAt page: NSFileProviderPage
    ) {
        let backendPage: Data? = isInitialPage(page) ? nil : (page as NSData as Data)
        let lease: any MacosBridgeRequestLease
        do {
            lease = try session.beginRequest()
        } catch {
            observer.finishEnumeratingWithError(MacosFileProviderErrorMapper.normalize(error))
            return
        }
        let terminal = MacosTerminalGate()
        var operationIdentifier: UUID?
        var runtimeOperationIdentifier: UUID?
        let slot = MacosCancellationSlot {
            terminal.finish {
                lease.release()
                if let operationIdentifier {
                    self.cancellations.remove(operationIdentifier)
                }
                if let runtimeOperationIdentifier {
                    self.runtimeCancellations.remove(runtimeOperationIdentifier)
                }
                observer.finishEnumeratingWithError(
                    MacosFileProviderErrorMapper.normalize(
                        MacosBridgeFailure(code: .cancelled)
                    )
                )
            }
        }
        guard let insertedIdentifier = cancellations.insert(slot) else {
            slot.cancel()
            return
        }
        guard let runtimeIdentifier = runtimeCancellations.insert(slot) else {
            cancellations.remove(insertedIdentifier)
            slot.cancel()
            return
        }
        runtimeOperationIdentifier = runtimeIdentifier
        operationIdentifier = insertedIdentifier
        let operation = dataSource.enumerate(
            containerIdentifier: containerIdentifier.rawValue,
            page: backendPage
        ) { result in
            terminal.finish {
                slot.complete()
                lease.release()
                self.cancellations.remove(insertedIdentifier)
                self.runtimeCancellations.remove(runtimeIdentifier)
                switch result {
                case let .success(page):
                    do {
                        try self.validationState.accept(page, validator: self.validator)
                        observer.didEnumerate(page.items.map(MacosFileProviderItem.init(snapshot:)))
                        observer.finishEnumerating(
                            upTo: page.nextPage.map { $0 as NSData as NSFileProviderPage }
                        )
                    } catch {
                        observer.finishEnumeratingWithError(
                            MacosFileProviderErrorMapper.normalize(error)
                        )
                    }
                case let .failure(error):
                    observer.finishEnumeratingWithError(
                        MacosFileProviderErrorMapper.normalize(error)
                    )
                }
            }
        }
        slot.install(operation)
    }

    public func enumerateChanges(
        for observer: NSFileProviderChangeObserver,
        from syncAnchor: NSFileProviderSyncAnchor
    ) {
        let portableAnchor: MacosSyncAnchor
        do {
            portableAnchor = try MacosSyncAnchor(bytes: syncAnchor as NSData as Data)
        } catch {
            observer.finishEnumeratingWithError(MacosFileProviderErrorMapper.normalize(error))
            return
        }
        let lease: any MacosBridgeRequestLease
        do {
            lease = try session.beginRequest()
        } catch {
            observer.finishEnumeratingWithError(MacosFileProviderErrorMapper.normalize(error))
            return
        }
        let terminal = MacosTerminalGate()
        var operationIdentifier: UUID?
        var runtimeOperationIdentifier: UUID?
        let slot = MacosCancellationSlot {
            terminal.finish {
                lease.release()
                if let operationIdentifier {
                    self.cancellations.remove(operationIdentifier)
                }
                if let runtimeOperationIdentifier {
                    self.runtimeCancellations.remove(runtimeOperationIdentifier)
                }
                observer.finishEnumeratingWithError(
                    MacosFileProviderErrorMapper.normalize(
                        MacosBridgeFailure(code: .cancelled)
                    )
                )
            }
        }
        guard let insertedIdentifier = cancellations.insert(slot) else {
            slot.cancel()
            return
        }
        guard let runtimeIdentifier = runtimeCancellations.insert(slot) else {
            cancellations.remove(insertedIdentifier)
            slot.cancel()
            return
        }
        runtimeOperationIdentifier = runtimeIdentifier
        operationIdentifier = insertedIdentifier
        let operation = dataSource.enumerateChanges(
            containerIdentifier: containerIdentifier.rawValue,
            from: portableAnchor
        ) { result in
            terminal.finish {
                slot.complete()
                lease.release()
                self.cancellations.remove(insertedIdentifier)
                self.runtimeCancellations.remove(runtimeIdentifier)
                switch result {
                case let .success(batch):
                    do {
                        try self.validator.validateChanges(batch)
                    } catch {
                        observer.finishEnumeratingWithError(
                            MacosFileProviderErrorMapper.normalize(error)
                        )
                        return
                    }
                    if !batch.updatedItems.isEmpty {
                        observer.didUpdate(
                            batch.updatedItems.map(MacosFileProviderItem.init(snapshot:))
                        )
                    }
                    if !batch.deletedItemIdentifiers.isEmpty {
                        observer.didDeleteItems(
                            withIdentifiers: batch.deletedItemIdentifiers.map {
                                NSFileProviderItemIdentifier($0)
                            }
                        )
                    }
                    observer.finishEnumeratingChanges(
                        upTo: NSFileProviderSyncAnchor(batch.syncAnchor.bytes),
                        moreComing: batch.moreComing
                    )
                case let .failure(error):
                    observer.finishEnumeratingWithError(
                        MacosFileProviderErrorMapper.normalize(error)
                    )
                }
            }
        }
        slot.install(operation)
    }

    public func currentSyncAnchor(
        completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void
    ) {
        let anchor = dataSource.currentSyncAnchor(
            containerIdentifier: containerIdentifier.rawValue
        )
        completionHandler(NSFileProviderSyncAnchor(anchor.bytes))
    }

    private func isInitialPage(_ page: NSFileProviderPage) -> Bool {
        page == NSFileProviderPage.initialPageSortedByDate as NSFileProviderPage
            || page == NSFileProviderPage.initialPageSortedByName as NSFileProviderPage
    }
}
