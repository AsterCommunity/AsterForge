import Foundation

public enum MacosBridgeErrorCode: Int32, CaseIterable, Sendable {
    case success = 0
    case notFound = 1
    case notAuthenticated = 2
    case permissionDenied = 3
    case versionOutOfDate = 4
    case tryAgain = 5
    case notSupported = 6
    case invalidArgument = 7
    case syncAnchorExpired = 8
    case cancelled = 9
    case providerNotFound = 10
    case `internal` = 11
}

public struct MacosBridgeFailure: Error, Equatable, Sendable {
    public let code: MacosBridgeErrorCode

    public init(code: MacosBridgeErrorCode) {
        self.code = code
    }
}

public protocol MacosBridgeRequestLease: AnyObject {
    var generation: UInt64 { get }
    func release()
}

public protocol MacosBridgeSession: AnyObject {
    var generation: UInt64 { get }
    func beginRequest() throws -> any MacosBridgeRequestLease
    func beginClosing()
    func markDisconnected()
}

public protocol MacosCancellable: AnyObject {
    func cancel()
}

public final class MacosNoopCancellation: MacosCancellable {
    public init() {}
    public func cancel() {}
}

public enum MacosCloudItemKind: Sendable {
    case file
    case directory
}

public struct MacosCloudItemSnapshot: Equatable, Sendable {
    public let identifier: String
    public let parentIdentifier: String
    public let filename: String
    public let kind: MacosCloudItemKind
    public let size: UInt64
    public let metadataVersion: Data
    public let contentVersion: Data
    public let contentTypeIdentifier: String

    public init(
        identifier: String,
        parentIdentifier: String,
        filename: String,
        kind: MacosCloudItemKind,
        size: UInt64,
        metadataVersion: Data,
        contentVersion: Data,
        contentTypeIdentifier: String
    ) throws {
        guard !identifier.isEmpty, !parentIdentifier.isEmpty else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        guard !filename.isEmpty, !filename.contains("/"), !filename.contains("\0") else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        guard (1 ... 128).contains(metadataVersion.count),
              (1 ... 128).contains(contentVersion.count)
        else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        guard !contentTypeIdentifier.isEmpty else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        guard kind == .file || size == 0 else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        self.identifier = identifier
        self.parentIdentifier = parentIdentifier
        self.filename = filename
        self.kind = kind
        self.size = size
        self.metadataVersion = metadataVersion
        self.contentVersion = contentVersion
        self.contentTypeIdentifier = contentTypeIdentifier
    }
}

public struct MacosEnumerationPage: Equatable, Sendable {
    public let items: [MacosCloudItemSnapshot]
    public let nextPage: Data?

    public init(items: [MacosCloudItemSnapshot], nextPage: Data?) throws {
        guard nextPage?.count ?? 0 <= 500 else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        self.items = items
        self.nextPage = nextPage
    }
}

public struct MacosSyncAnchor: Equatable, Sendable {
    public static let initial = MacosSyncAnchor(validatedBytes: Data())

    public let bytes: Data

    public init(bytes: Data) throws {
        guard bytes.count <= 500 else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        self.bytes = bytes
    }

    private init(validatedBytes: Data) {
        bytes = validatedBytes
    }
}

public struct MacosChangeBatch: Equatable, Sendable {
    public let updatedItems: [MacosCloudItemSnapshot]
    public let deletedItemIdentifiers: [String]
    public let syncAnchor: MacosSyncAnchor
    public let moreComing: Bool

    public init(
        updatedItems: [MacosCloudItemSnapshot],
        deletedItemIdentifiers: [String],
        syncAnchor: MacosSyncAnchor,
        moreComing: Bool
    ) throws {
        let updatedIdentifiers = Set(updatedItems.map(\.identifier))
        guard updatedIdentifiers.count == updatedItems.count else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        guard deletedItemIdentifiers.allSatisfy({ !$0.isEmpty }) else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        let deletedIdentifiers = Set(deletedItemIdentifiers)
        guard deletedIdentifiers.count == deletedItemIdentifiers.count,
              updatedIdentifiers.isDisjoint(with: deletedIdentifiers)
        else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        self.updatedItems = updatedItems
        self.deletedItemIdentifiers = deletedItemIdentifiers
        self.syncAnchor = syncAnchor
        self.moreComing = moreComing
    }
}

public struct MacosFetchedContent: Equatable, Sendable {
    public let item: MacosCloudItemSnapshot
    public let bytes: Data

    public init(item: MacosCloudItemSnapshot, bytes: Data) throws {
        guard item.kind == .file, UInt64(exactly: bytes.count) == item.size else {
            throw MacosBridgeFailure(code: .internal)
        }
        self.item = item
        self.bytes = bytes
    }
}

public protocol MacosCloudFilesDataSource: AnyObject {
    func currentSyncAnchor(containerIdentifier: String) -> MacosSyncAnchor

    @discardableResult
    func item(
        for identifier: String,
        completion: @escaping (Result<MacosCloudItemSnapshot, Error>) -> Void
    ) -> any MacosCancellable

    @discardableResult
    func enumerate(
        containerIdentifier: String,
        page: Data?,
        completion: @escaping (Result<MacosEnumerationPage, Error>) -> Void
    ) -> any MacosCancellable

    @discardableResult
    func enumerateChanges(
        containerIdentifier: String,
        from syncAnchor: MacosSyncAnchor,
        completion: @escaping (Result<MacosChangeBatch, Error>) -> Void
    ) -> any MacosCancellable

    @discardableResult
    func fetchContents(
        for identifier: String,
        requestedContentVersion: Data?,
        completion: @escaping (Result<MacosFetchedContent, Error>) -> Void
    ) -> any MacosCancellable
}
