import FileProvider
import Foundation

final class FileProviderExtension: NSObject, NSFileProviderReplicatedExtension {
    private let runtime: MacosReadOnlyFileProviderRuntime
    private let materializedTracker: MacosMaterializedSetTracker?

    required init(domain: NSFileProviderDomain) {
        let session: any MacosBridgeSession
        do {
            session = try RustMacosBridgeSession(
                generation: UInt64.random(in: 1 ... UInt64.max)
            )
        } catch {
            session = UnavailableMacosBridgeSession(error: error)
        }

        let manager = NSFileProviderManager(for: domain)
        let temporaryStore: any MacosTemporaryContentStore
        do {
            guard let manager else {
                throw MacosBridgeFailure(code: .providerNotFound)
            }
            temporaryStore = try MacosDirectoryTemporaryContentStore(
                directory: manager.temporaryDirectoryURL()
            )
        } catch {
            temporaryStore = UnavailableTemporaryContentStore(error: error)
        }

        let materializedStore: (any MacosMaterializedSetPersisting)?
        if let manager,
           let appGroupURL = FileManager.default.containerURL(
               forSecurityApplicationGroupIdentifier: FixtureDomainConfiguration.appGroupIdentifier
           )
        {
            materializedStore = try? MacosFileMaterializedSetStore(
                directory: appGroupURL.appendingPathComponent(
                    "AsterForgeCloudFilesFixture/materialized",
                    isDirectory: true
                )
            )
        } else {
            materializedStore = nil
        }

        let dataSource: any MacosCloudFilesDataSource
        do {
            dataSource = try MemoryCloudDataSource(materializedStore: materializedStore)
        } catch {
            dataSource = UnavailableMemoryCloudDataSource(error: error)
        }
        if let manager, let materializedStore {
            materializedTracker = MacosMaterializedSetTracker(
                reader: MacosFileProviderMaterializedItemsReader(manager: manager),
                store: materializedStore
            )
        } else {
            materializedTracker = nil
        }
        runtime = MacosReadOnlyFileProviderRuntime(
            dataSource: dataSource,
            session: session,
            temporaryContentStore: temporaryStore
        )
        super.init()
    }

    func invalidate() {
        materializedTracker?.invalidate()
        runtime.invalidate()
    }

    func materializedItemsDidChange(completionHandler: @escaping () -> Void) {
        guard let materializedTracker else {
            completionHandler()
            return
        }
        materializedTracker.refresh(completion: completionHandler)
    }

    func item(
        for identifier: NSFileProviderItemIdentifier,
        request _: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, Error?) -> Void
    ) -> Progress {
        runtime.item(for: identifier, completionHandler: completionHandler)
    }

    func fetchContents(
        for itemIdentifier: NSFileProviderItemIdentifier,
        version requestedVersion: NSFileProviderItemVersion?,
        request _: NSFileProviderRequest,
        completionHandler: @escaping (URL?, NSFileProviderItem?, Error?) -> Void
    ) -> Progress {
        runtime.fetchContents(
            for: itemIdentifier,
            requestedVersion: requestedVersion,
            completionHandler: completionHandler
        )
    }

    func createItem(
        basedOn _: NSFileProviderItem,
        fields _: NSFileProviderItemFields,
        contents _: URL?,
        options _: NSFileProviderCreateItemOptions,
        request _: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void
    ) -> Progress {
        unsupported { completionHandler(nil, [], false, $0) }
    }

    func modifyItem(
        _: NSFileProviderItem,
        baseVersion _: NSFileProviderItemVersion,
        changedFields _: NSFileProviderItemFields,
        contents _: URL?,
        options _: NSFileProviderModifyItemOptions,
        request _: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void
    ) -> Progress {
        unsupported { completionHandler(nil, [], false, $0) }
    }

    func deleteItem(
        identifier _: NSFileProviderItemIdentifier,
        baseVersion _: NSFileProviderItemVersion,
        options _: NSFileProviderDeleteItemOptions,
        request _: NSFileProviderRequest,
        completionHandler: @escaping (Error?) -> Void
    ) -> Progress {
        unsupported(completion: completionHandler)
    }

    func enumerator(
        for containerItemIdentifier: NSFileProviderItemIdentifier,
        request _: NSFileProviderRequest
    ) throws -> NSFileProviderEnumerator {
        try runtime.enumerator(
            for: containerItemIdentifier
        )
    }

    private func unsupported(completion: (Error) -> Void) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        progress.completedUnitCount = 1
        completion(MacosFileProviderErrorMapper.normalize(MacosBridgeFailure(code: .notSupported)))
        return progress
    }
}

final class UnavailableMemoryCloudDataSource: MacosCloudFilesDataSource {
    private let error: Error

    init(error: Error) {
        self.error = error
    }

    func currentSyncAnchor(containerIdentifier _: String) -> MacosSyncAnchor {
        .initial
    }

    func item(
        for _: String,
        completion: @escaping (Result<MacosCloudItemSnapshot, Error>) -> Void
    ) -> any MacosCancellable {
        completion(.failure(error))
        return MacosNoopCancellation()
    }

    func enumerate(
        containerIdentifier _: String,
        page _: Data?,
        completion: @escaping (Result<MacosEnumerationPage, Error>) -> Void
    ) -> any MacosCancellable {
        completion(.failure(error))
        return MacosNoopCancellation()
    }

    func enumerateChanges(
        containerIdentifier _: String,
        from _: MacosSyncAnchor,
        completion: @escaping (Result<MacosChangeBatch, Error>) -> Void
    ) -> any MacosCancellable {
        completion(.failure(error))
        return MacosNoopCancellation()
    }

    func fetchContents(
        for _: String,
        requestedContentVersion _: Data?,
        completion: @escaping (Result<MacosFetchedContent, Error>) -> Void
    ) -> any MacosCancellable {
        completion(.failure(error))
        return MacosNoopCancellation()
    }
}
