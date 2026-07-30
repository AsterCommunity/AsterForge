import Foundation

enum MacosMemoryCloudExampleCommandError: Error, Equatable, CustomStringConvertible {
    case invalidAnchor(String)
    case invalidPath(String)
    case missingPath(String)
    case unexpectedArguments(String)
    case unknownCommand(String)

    var description: String {
        switch self {
        case let .invalidAnchor(anchor):
            "invalid change anchor: \(anchor)"
        case let .invalidPath(path):
            "invalid memory-cloud path: \(path)"
        case let .missingPath(command):
            "\(command) requires a path"
        case let .unexpectedArguments(command):
            "\(command) received unexpected arguments"
        case let .unknownCommand(command):
            "unknown command: \(command)"
        }
    }
}

struct MacosMemoryCloudExamplePath: Equatable, Sendable, CustomStringConvertible {
    static let root = Self(components: [])

    let components: [String]

    init(argument: String) throws {
        if argument == "root" || argument == "." || argument == "/" {
            self = .root
            return
        }
        let parts = argument.split(separator: "/", omittingEmptySubsequences: false)
        guard !argument.isEmpty,
              !argument.hasPrefix("/"),
              !argument.hasSuffix("/"),
              !argument.contains("\0"),
              parts.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." })
        else {
            throw MacosMemoryCloudExampleCommandError.invalidPath(argument)
        }
        components = parts.map(String.init)
    }

    private init(components: [String]) {
        self.components = components
    }

    var description: String {
        components.isEmpty ? "root" : components.joined(separator: "/")
    }
}

enum MacosMemoryCloudExampleAnchor: String, Equatable, Sendable {
    case initial
    case current
    case expired
}

enum MacosMemoryCloudExampleCommand: Equatable, Sendable {
    case cat(MacosMemoryCloudExamplePath)
    case changes(MacosMemoryCloudExampleAnchor)
    case help
    case list(MacosMemoryCloudExamplePath)
    case smoke
    case workingSet

    static func parse(arguments: [String]) throws -> Self {
        guard let command = arguments.first else {
            return .smoke
        }
        let operands = Array(arguments.dropFirst())
        switch command {
        case "cat":
            guard let path = operands.first else {
                throw MacosMemoryCloudExampleCommandError.missingPath(command)
            }
            guard operands.count == 1 else {
                throw MacosMemoryCloudExampleCommandError.unexpectedArguments(command)
            }
            return try .cat(MacosMemoryCloudExamplePath(argument: path))
        case "changes":
            guard operands.count <= 1 else {
                throw MacosMemoryCloudExampleCommandError.unexpectedArguments(command)
            }
            let value = operands.first ?? MacosMemoryCloudExampleAnchor.initial.rawValue
            guard let anchor = MacosMemoryCloudExampleAnchor(rawValue: value) else {
                throw MacosMemoryCloudExampleCommandError.invalidAnchor(value)
            }
            return .changes(anchor)
        case "help", "--help", "-h":
            guard operands.isEmpty else {
                throw MacosMemoryCloudExampleCommandError.unexpectedArguments(command)
            }
            return .help
        case "list":
            guard operands.count <= 1 else {
                throw MacosMemoryCloudExampleCommandError.unexpectedArguments(command)
            }
            return try .list(MacosMemoryCloudExamplePath(argument: operands.first ?? "root"))
        case "smoke":
            guard operands.isEmpty else {
                throw MacosMemoryCloudExampleCommandError.unexpectedArguments(command)
            }
            return .smoke
        case "working-set":
            guard operands.isEmpty else {
                throw MacosMemoryCloudExampleCommandError.unexpectedArguments(command)
            }
            return .workingSet
        default:
            throw MacosMemoryCloudExampleCommandError.unknownCommand(command)
        }
    }
}

enum MacosMemoryCloudExampleEntryKind: String, Equatable, Sendable {
    case directory
    case file
}

struct MacosMemoryCloudExampleEntry: Equatable, Sendable {
    let kind: MacosMemoryCloudExampleEntryKind
    let name: String
    let size: UInt64
}

enum MacosMemoryCloudExampleOutput {
    static let help = """
    Usage: macos_memory_cloud_drive [COMMAND]

      smoke                         run the complete standalone contract
      list [root|PATH]              list a memory-cloud directory
      cat PATH                      print one memory-cloud file
      changes [initial|current|expired]
                                    enumerate the root change feed
      working-set                   show the materialized working set
      help                          show this help
    """

    static func list(_ entries: [MacosMemoryCloudExampleEntry]) -> String {
        entries.map { entry in
            switch entry.kind {
            case .directory:
                "directory\t\(entry.name)"
            case .file:
                "file\t\(entry.name)\t\(entry.size) bytes"
            }
        }.joined(separator: "\n")
    }

    static func changes(
        updated: [MacosMemoryCloudExampleEntry],
        deleted: [String],
        moreComing: Bool,
        anchor: String
    ) -> String {
        var lines = ["updated:"]
        lines.append(contentsOf: updated.map { "  \($0.kind.rawValue)\t\($0.name)" })
        if updated.isEmpty {
            lines.append("  none")
        }
        lines.append("deleted:")
        lines.append(contentsOf: deleted.map { "  \($0)" })
        if deleted.isEmpty {
            lines.append("  none")
        }
        lines.append("more-coming: \(moreComing)")
        lines.append("anchor: \(anchor)")
        return lines.joined(separator: "\n")
    }

    static func smoke(steps: [String]) -> String {
        (steps.map { "ok \($0)" } + ["memory cloud example passed"]).joined(separator: "\n")
    }
}
