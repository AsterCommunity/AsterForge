import Foundation
import FileProvider
@testable import AsterForgeCloudFilesMacosShell

extension MacosFetchedContent {
    init(item: MacosCloudItemSnapshot, bytes: Data) throws {
        let url = Foundation.FileManager().temporaryDirectory
            .appendingPathComponent("aster-forge-fetched-test-\(UUID().uuidString)")
        try bytes.write(to: url)
        try self.init(item: item, stagingURL: url)
    }
}

final class TestIdentifierDecoder: MacosPersistentIdentifierDecoding {
    func decodeItemIdentifier(_ identifier: String) throws -> MacosScopedItemIdentity? {
        if [
            NSFileProviderItemIdentifier.rootContainer.rawValue,
            NSFileProviderItemIdentifier.workingSet.rawValue,
            NSFileProviderItemIdentifier.trashContainer.rawValue,
        ].contains(identifier) {
            return nil
        }
        guard !identifier.isEmpty, !identifier.contains("invalid") else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        return try MacosScopedItemIdentity(
            namespace: "test-namespace",
            root: "test-root",
            item: identifier
        )
    }
}

let testScope = (namespace: "test-namespace", root: "test-root")
