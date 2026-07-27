@testable import AsterForgeCloudFilesMacosExampleSupport
import XCTest

final class MemoryCloudCommandTests: XCTestCase {
    func testNoArgumentsDefaultsToSmokeAndHelpAliasesMatch() throws {
        XCTAssertEqual(try MacosMemoryCloudExampleCommand.parse(arguments: []), .smoke)
        XCTAssertEqual(try MacosMemoryCloudExampleCommand.parse(arguments: ["help"]), .help)
        XCTAssertEqual(try MacosMemoryCloudExampleCommand.parse(arguments: ["--help"]), .help)
        XCTAssertEqual(try MacosMemoryCloudExampleCommand.parse(arguments: ["-h"]), .help)
    }

    func testListAcceptsRootAliasesAndNestedDirectory() throws {
        XCTAssertEqual(
            try MacosMemoryCloudExampleCommand.parse(arguments: ["list"]),
            .list(.root)
        )
        for alias in ["root", ".", "/"] {
            XCTAssertEqual(
                try MacosMemoryCloudExampleCommand.parse(arguments: ["list", alias]),
                .list(.root)
            )
        }
        XCTAssertEqual(
            try MacosMemoryCloudExampleCommand.parse(arguments: ["list", "Documents"]),
            .list(try MacosMemoryCloudExamplePath(argument: "Documents"))
        )
    }

    func testCatPreservesValidatedNestedPath() throws {
        let path = try MacosMemoryCloudExamplePath(argument: "Documents/hello.txt")
        XCTAssertEqual(path.description, "Documents/hello.txt")
        XCTAssertEqual(
            try MacosMemoryCloudExampleCommand.parse(
                arguments: ["cat", "Documents/hello.txt"]
            ),
            .cat(path)
        )
    }

    func testChangesDefaultsToInitialAndAcceptsEveryAnchorMode() throws {
        XCTAssertEqual(
            try MacosMemoryCloudExampleCommand.parse(arguments: ["changes"]),
            .changes(.initial)
        )
        for anchor in [
            MacosMemoryCloudExampleAnchor.initial,
            .current,
            .expired,
        ] {
            XCTAssertEqual(
                try MacosMemoryCloudExampleCommand.parse(
                    arguments: ["changes", anchor.rawValue]
                ),
                .changes(anchor)
            )
        }
    }

    func testParserRejectsMissingExtraAndUnknownArguments() {
        assertError(["cat"], .missingPath("cat"))
        assertError(["cat", "README.txt", "extra"], .unexpectedArguments("cat"))
        assertError(["list", "root", "extra"], .unexpectedArguments("list"))
        assertError(["changes", "later"], .invalidAnchor("later"))
        assertError(["working-set", "extra"], .unexpectedArguments("working-set"))
        assertError(["wat"], .unknownCommand("wat"))
    }

    func testPathRejectsEmptyAbsoluteTraversalAndEmptyComponents() {
        for path in ["", "/README.txt", "Documents/", "Documents//hello.txt", ".hidden/..", "a/./b", "a\0b"] {
            XCTAssertThrowsError(try MacosMemoryCloudExamplePath(argument: path)) { error in
                XCTAssertEqual(
                    error as? MacosMemoryCloudExampleCommandError,
                    .invalidPath(path)
                )
            }
        }
    }

    func testListAndChangeOutputAreStable() {
        let entries = [
            MacosMemoryCloudExampleEntry(kind: .file, name: "README.txt", size: 44),
            MacosMemoryCloudExampleEntry(kind: .directory, name: "Documents", size: 0),
        ]
        XCTAssertEqual(
            MacosMemoryCloudExampleOutput.list(entries),
            "file\tREADME.txt\t44 bytes\ndirectory\tDocuments"
        )
        XCTAssertEqual(
            MacosMemoryCloudExampleOutput.changes(
                updated: entries,
                deleted: [],
                moreComing: false,
                anchor: "memory-fixture-v1"
            ),
            """
            updated:
              file\tREADME.txt
              directory\tDocuments
            deleted:
              none
            more-coming: false
            anchor: memory-fixture-v1
            """
        )
    }

    func testSmokeOutputKeepsOrderedStepsAndTerminalResult() {
        XCTAssertEqual(
            MacosMemoryCloudExampleOutput.smoke(steps: ["ffi", "root-list"]),
            "ok ffi\nok root-list\nmemory cloud example passed"
        )
    }

    private func assertError(
        _ arguments: [String],
        _ expected: MacosMemoryCloudExampleCommandError
    ) {
        XCTAssertThrowsError(
            try MacosMemoryCloudExampleCommand.parse(arguments: arguments)
        ) { error in
            XCTAssertEqual(error as? MacosMemoryCloudExampleCommandError, expected)
        }
    }
}
