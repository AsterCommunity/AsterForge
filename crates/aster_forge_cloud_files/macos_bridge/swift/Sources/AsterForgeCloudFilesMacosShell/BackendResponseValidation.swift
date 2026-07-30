import FileProvider
import Foundation

final class MacosBackendResponseValidator {
    private let scope: (namespace: String, root: String)
    private let identifierDecoder: any MacosPersistentIdentifierDecoding

    init(
        scope: (namespace: String, root: String),
        identifierDecoder: any MacosPersistentIdentifierDecoding
    ) {
        self.scope = scope
        self.identifierDecoder = identifierDecoder
    }

    func validateItem(
        _ item: MacosCloudItemSnapshot,
        requestedIdentifier: String
    ) throws {
        guard item.identifier == requestedIdentifier else {
            throw MacosBridgeFailure(code: .internal)
        }
        try validateSnapshot(item)
        if requestedIdentifier == NSFileProviderItemIdentifier.rootContainer.rawValue {
            guard item.parentIdentifier == requestedIdentifier,
                  item.kind == .directory
            else {
                throw MacosBridgeFailure(code: .internal)
            }
        }
    }

    func makeEnumerationState(containerIdentifier: String) throws -> MacosEnumerationValidationState {
        try validateIdentifier(containerIdentifier, allowSystemContainer: true)
        return MacosEnumerationValidationState(containerIdentifier: containerIdentifier)
    }

    func validateChanges(_ batch: MacosChangeBatch) throws {
        try batch.updatedItems.forEach(validateSnapshot)
        try batch.deletedItemIdentifiers.forEach {
            try validateIdentifier($0, allowSystemContainer: false)
        }
    }

    func validateFetchedContent(
        _ content: MacosFetchedContent,
        requestedIdentifier: String,
        requestedContentVersion: Data?
    ) throws {
        try validateItem(content.item, requestedIdentifier: requestedIdentifier)
        guard content.item.kind == .file else {
            throw MacosBridgeFailure(code: .internal)
        }
        if let requestedContentVersion,
           content.item.contentVersion != requestedContentVersion
        {
            throw MacosBridgeFailure(code: .versionOutOfDate)
        }
    }

    fileprivate func validateSnapshot(_ item: MacosCloudItemSnapshot) throws {
        try validateIdentifier(item.identifier, allowSystemContainer: true)
        try validateIdentifier(item.parentIdentifier, allowSystemContainer: true)
        guard item.identifier != NSFileProviderItemIdentifier.workingSet.rawValue,
              item.identifier != NSFileProviderItemIdentifier.trashContainer.rawValue
        else {
            throw MacosBridgeFailure(code: .internal)
        }
    }

    fileprivate func validateIdentifier(
        _ identifier: String,
        allowSystemContainer: Bool
    ) throws {
        guard !identifier.isEmpty,
              identifier.utf8.count <= macosMaximumIdentifierBytes
        else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        let systemIdentifiers = [
            NSFileProviderItemIdentifier.rootContainer.rawValue,
            NSFileProviderItemIdentifier.workingSet.rawValue,
            NSFileProviderItemIdentifier.trashContainer.rawValue,
        ]
        if systemIdentifiers.contains(identifier) {
            guard allowSystemContainer else {
                throw MacosBridgeFailure(code: .internal)
            }
            return
        }
        let decoded = try identifierDecoder.decodeItemIdentifier(identifier)
        guard let decoded,
              decoded.namespace == scope.namespace,
              decoded.root == scope.root
        else {
            throw MacosBridgeFailure(code: .internal)
        }
    }
}

final class MacosEnumerationValidationState {
    private let lock = NSLock()
    private let containerIdentifier: String
    private var identifiers: Set<String> = []
    private var filenames: Set<String> = []
    private var cursors: Set<Data> = []
    private var retainedBytes = 0
    private var finished = false

    init(containerIdentifier: String) {
        self.containerIdentifier = containerIdentifier
    }

    func accept(_ page: MacosEnumerationPage, validator: MacosBackendResponseValidator) throws {
        lock.lock()
        defer { lock.unlock() }
        guard !finished,
              identifiers.count + page.items.count <= macosMaximumEnumerationItems
        else {
            throw MacosBridgeFailure(code: .internal)
        }
        var pageIdentifiers: Set<String> = []
        var pageFilenames: Set<String> = []
        var addedBytes = 0
        for item in page.items {
            try validator.validateSnapshot(item)
            if containerIdentifier != NSFileProviderItemIdentifier.workingSet.rawValue,
               containerIdentifier != NSFileProviderItemIdentifier.trashContainer.rawValue
            {
                guard item.parentIdentifier == containerIdentifier else {
                    throw MacosBridgeFailure(code: .internal)
                }
            }
            guard !identifiers.contains(item.identifier),
                  !filenames.contains(item.filename),
                  pageIdentifiers.insert(item.identifier).inserted,
                  pageFilenames.insert(item.filename).inserted
            else {
                throw MacosBridgeFailure(code: .internal)
            }
            addedBytes += item.identifier.utf8.count + item.filename.utf8.count
        }
        if let cursor = page.nextPage {
            guard cursors.insert(cursor).inserted else {
                throw MacosBridgeFailure(code: .internal)
            }
            addedBytes += cursor.count
        } else {
            finished = true
        }
        guard retainedBytes + addedBytes <= macosMaximumEnumerationStateBytes else {
            throw MacosBridgeFailure(code: .internal)
        }
        retainedBytes += addedBytes
        identifiers.formUnion(pageIdentifiers)
        filenames.formUnion(pageFilenames)
    }
}
