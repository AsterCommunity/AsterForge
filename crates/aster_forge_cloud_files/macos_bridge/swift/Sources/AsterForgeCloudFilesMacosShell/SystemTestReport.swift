import Foundation

struct MacosSystemTestCommand: Equatable {
    let phase: String

    static func requested(arguments: [String]) -> Self? {
        guard let marker = arguments.firstIndex(of: "--system-test"),
              arguments.indices.contains(marker + 1)
        else {
            return nil
        }
        return Self(phase: arguments[marker + 1])
    }
}

// The lock protects the continuation and exactly-once transition across callback queues.
final class MacosSystemTestCallbackGate<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<Value, Error>?

    init(continuation: CheckedContinuation<Value, Error>) {
        self.continuation = continuation
    }

    func resolve(_ result: Result<Value, Error>) {
        lock.lock()
        guard let continuation else {
            lock.unlock()
            return
        }
        self.continuation = nil
        lock.unlock()
        continuation.resume(with: result)
    }
}

struct MacosSystemTestReport: Codable, Equatable {
    let phase: String
    private(set) var completedSteps: [String] = []
    private(set) var failure: String?
    var rootURL: String?
    var materializedDirectoryCount: Int?

    var passed: Bool { failure == nil }

    mutating func complete(_ step: String) {
        guard !completedSteps.contains(step) else { return }
        completedSteps.append(step)
    }

    mutating func fail(_ error: Error) {
        guard failure == nil else { return }
        failure = String(describing: error)
    }

    func encodedLine() throws -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        let data = try encoder.encode(self)
        guard let line = String(data: data, encoding: .utf8) else {
            throw CocoaError(.fileWriteInapplicableStringEncoding)
        }
        return line
    }
}
