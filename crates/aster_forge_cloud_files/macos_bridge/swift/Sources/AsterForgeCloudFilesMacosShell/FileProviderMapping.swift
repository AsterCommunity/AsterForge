import FileProvider
import Foundation
import UniformTypeIdentifiers

public final class MacosFileProviderItem: NSObject, NSFileProviderItem {
    public let snapshot: MacosCloudItemSnapshot

    public init(snapshot: MacosCloudItemSnapshot) {
        self.snapshot = snapshot
        super.init()
    }

    public var itemIdentifier: NSFileProviderItemIdentifier {
        NSFileProviderItemIdentifier(snapshot.identifier)
    }

    public var parentItemIdentifier: NSFileProviderItemIdentifier {
        NSFileProviderItemIdentifier(snapshot.parentIdentifier)
    }

    public var filename: String {
        snapshot.filename
    }

    public var contentType: UTType {
        if snapshot.kind == .directory {
            return .folder
        }
        return UTType(snapshot.contentTypeIdentifier) ?? .data
    }

    public var capabilities: NSFileProviderItemCapabilities {
        switch snapshot.kind {
        case .file:
            return [.allowsReading]
        case .directory:
            return [.allowsReading, .allowsContentEnumerating]
        }
    }

    public var documentSize: NSNumber? {
        snapshot.kind == .file ? NSNumber(value: snapshot.size) : nil
    }

    public var itemVersion: NSFileProviderItemVersion {
        NSFileProviderItemVersion(
            contentVersion: snapshot.contentVersion,
            metadataVersion: snapshot.metadataVersion
        )
    }
}

public enum MacosFileProviderErrorMapper {
    public static func error(for code: MacosBridgeErrorCode) -> NSError? {
        switch code {
        case .success:
            return nil
        case .notFound:
            return NSFileProviderError(.noSuchItem) as NSError
        case .notAuthenticated:
            return NSFileProviderError(.notAuthenticated) as NSError
        case .permissionDenied:
            return NSError(domain: NSCocoaErrorDomain, code: NSFileReadNoPermissionError)
        case .versionOutOfDate:
            return NSFileProviderError(.versionNoLongerAvailable) as NSError
        case .tryAgain:
            return NSFileProviderError(.serverUnreachable) as NSError
        case .notSupported:
            return NSError(domain: NSCocoaErrorDomain, code: NSFeatureUnsupportedError)
        case .invalidArgument:
            return NSError(domain: NSCocoaErrorDomain, code: NSFileWriteInvalidFileNameError)
        case .syncAnchorExpired:
            return NSFileProviderError(.syncAnchorExpired) as NSError
        case .cancelled:
            return NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError)
        case .providerNotFound:
            return NSFileProviderError(.providerNotFound) as NSError
        case .internal:
            return NSError(domain: NSCocoaErrorDomain, code: NSXPCConnectionReplyInvalid)
        }
    }

    public static func normalize(_ error: Error) -> NSError {
        if let bridge = error as? MacosBridgeFailure {
            return Self.error(for: bridge.code)
                ?? NSError(domain: NSCocoaErrorDomain, code: NSXPCConnectionReplyInvalid)
        }
        let native = error as NSError
        if native.domain == NSFileProviderErrorDomain || native.domain == NSCocoaErrorDomain {
            return native
        }
        return NSError(
            domain: NSCocoaErrorDomain,
            code: NSXPCConnectionReplyInvalid,
            userInfo: [NSUnderlyingErrorKey: native]
        )
    }
}
