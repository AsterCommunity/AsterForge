@testable import AsterForgeCloudFilesMacosShell
import Foundation
import XCTest

final class SystemTestReportTests: XCTestCase {
    func testCommandParsesPhaseAfterMarkerAmongOtherArguments() {
        let command = MacosSystemTestCommand.requested(
            arguments: ["fixture", "--other", "value", "--system-test", "probe"]
        )

        XCTAssertEqual(command, MacosSystemTestCommand(phase: "probe"))
    }

    func testCommandRejectsMissingMarkerOrPhase() {
        XCTAssertNil(MacosSystemTestCommand.requested(arguments: ["fixture"]))
        XCTAssertNil(
            MacosSystemTestCommand.requested(arguments: ["fixture", "--system-test"])
        )
    }

    func testReportRecordsOrderedUniqueStepsAndRoundTripsExactFields() throws {
        var report = MacosSystemTestReport(phase: "baseline")
        report.complete("domain-added")
        report.complete("root-enumerated")
        report.complete("domain-added")
        report.rootURL = "file:///fixture-root/"
        report.materializedDirectoryCount = 1

        XCTAssertTrue(report.passed)
        XCTAssertEqual(report.completedSteps, ["domain-added", "root-enumerated"])

        let encoded = try XCTUnwrap(report.encodedLine().data(using: .utf8))
        XCTAssertEqual(try JSONDecoder().decode(MacosSystemTestReport.self, from: encoded), report)
    }

    func testReportPreservesFirstFailureAndRejectsPassedState() {
        var report = MacosSystemTestReport(phase: "recovery")
        report.fail(TestFailure.first)
        report.fail(TestFailure.second)

        XCTAssertFalse(report.passed)
        XCTAssertEqual(report.failure, "first")
    }

    func testCallbackGateResolvesOnlyTheFirstConcurrentResult() async throws {
        let value = try await withCheckedThrowingContinuation { continuation in
            let gate = MacosSystemTestCallbackGate<Int>(continuation: continuation)
            DispatchQueue.concurrentPerform(iterations: 32) { value in
                gate.resolve(.success(value))
            }
        }

        XCTAssertTrue((0 ..< 32).contains(value))
    }

    private enum TestFailure: String, Error, CustomStringConvertible {
        case first
        case second

        var description: String { rawValue }
    }
}
