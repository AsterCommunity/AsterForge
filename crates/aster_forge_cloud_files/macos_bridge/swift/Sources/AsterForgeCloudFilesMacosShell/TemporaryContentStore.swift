import Foundation

public protocol MacosTemporaryContentStore: AnyObject {
    func write(_ bytes: Data) throws -> URL
    func removeIfPresent(_ url: URL)
}

public final class MacosDirectoryTemporaryContentStore: MacosTemporaryContentStore {
    private let directory: URL

    public init(directory: URL) throws {
        self.directory = directory
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
    }

    public func write(_ bytes: Data) throws -> URL {
        let destination = directory.appendingPathComponent(UUID().uuidString, isDirectory: false)
        try bytes.write(to: destination, options: [.atomic])
        return destination
    }

    public func removeIfPresent(_ url: URL) {
        try? FileManager.default.removeItem(at: url)
    }
}
