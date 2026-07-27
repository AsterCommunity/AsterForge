import FileProvider
import Foundation

enum FixtureSystemTestPhase: String {
    case probe
    case baseline
    case recovery
    case cleanup
}

@MainActor
final class FixtureSystemTestRunner {
    private struct MaterializedRecord: Decodable {
        let schemaVersion: UInt8
        let directoryIdentifiers: [String]
        let syncAnchor: Data
    }

    private enum TestFailure: Error, CustomStringConvertible {
        case missingManager
        case missingRootURL
        case missingIdentifier(URL)
        case wrongDomain(String)
        case timeout(String)
        case unexpectedDirectory([String])
        case unexpectedContent(URL)
        case invalidMaterializedRecord

        var description: String {
            switch self {
            case .missingManager:
                "File Provider manager was not created"
            case .missingRootURL:
                "File Provider root URL was not returned"
            case let .missingIdentifier(url):
                "File Provider identifier is missing for \(url.path)"
            case let .wrongDomain(identifier):
                "URL resolved to unexpected domain \(identifier)"
            case let .timeout(operation):
                "Timed out while waiting for \(operation)"
            case let .unexpectedDirectory(names):
                "Root enumeration returned unexpected entries: \(names.sorted())"
            case let .unexpectedContent(url):
                "Hydrated bytes changed for \(url.lastPathComponent)"
            case .invalidMaterializedRecord:
                "Materialized App Group record is missing or invalid"
            }
        }
    }

    private static let readmeBytes = Data(
        "AsterForge in-memory File Provider fixture.\n".utf8
    )
    private static let helloBytes = Data(
        "Hello from the in-memory cloud.\n".utf8
    )
    private static let currentAnchor = Data("memory-fixture-v1".utf8)

    private let phase: FixtureSystemTestPhase
    private var report: MacosSystemTestReport

    init(phase: FixtureSystemTestPhase) {
        self.phase = phase
        report = MacosSystemTestReport(phase: phase.rawValue)
    }

    func run(completion: @escaping (MacosSystemTestReport) -> Void) {
        Task { @MainActor in
            do {
                switch phase {
                case .probe:
                    report.complete("host-launched")
                case .baseline:
                    try await runBaseline()
                case .recovery:
                    try await runRecovery()
                case .cleanup:
                    try await removeDomainIfPresent()
                    report.complete("domain-removed")
                }
            } catch {
                report.fail(error)
            }
            completion(report)
        }
    }

    private func runBaseline() async throws {
        try await removeDomainIfPresent()
        report.complete("previous-domain-removed")
        try await addDomain()
        report.complete("domain-added")

        let manager = try await waitForManager()
        try await waitForStabilization(manager)
        report.complete("domain-stabilized")

        let rootURL = try await userVisibleURL(manager, identifier: .rootContainer)
        report.rootURL = rootURL.path
        let didAccess = rootURL.startAccessingSecurityScopedResource()
        defer {
            if didAccess {
                rootURL.stopAccessingSecurityScopedResource()
            }
        }

        let names = try await waitForRootEnumeration(rootURL)
        guard names.contains("README.txt"), names.contains("Documents") else {
            throw TestFailure.unexpectedDirectory(names)
        }
        report.complete("root-enumerated")

        let readmeURL = rootURL.appendingPathComponent("README.txt", isDirectory: false)
        let documentsURL = rootURL.appendingPathComponent("Documents", isDirectory: true)
        let helloURL = documentsURL.appendingPathComponent("hello.txt", isDirectory: false)
        let readmeIdentifier = try await validateIdentifier(for: readmeURL)
        _ = try await validateIdentifier(for: documentsURL)

        try await requireContent(Self.readmeBytes, at: readmeURL)
        report.complete("readme-hydrated")
        try await requireContent(Self.helloBytes, at: helloURL)
        report.complete("nested-file-hydrated")

        try await waitForStabilization(manager)
        let materializedCount = try await waitForMaterializedRecord()
        report.materializedDirectoryCount = materializedCount
        report.complete("materialized-store-persisted")

        try await signalWorkingSet(manager)
        report.complete("working-set-signaled")
        try await evict(manager, identifier: readmeIdentifier)
        report.complete("readme-evicted")
    }

    private func runRecovery() async throws {
        let manager = try await waitForManager()
        try await waitForStabilization(manager)
        report.complete("domain-reopened")

        let rootURL = try await userVisibleURL(manager, identifier: .rootContainer)
        report.rootURL = rootURL.path
        let didAccess = rootURL.startAccessingSecurityScopedResource()
        defer {
            if didAccess {
                rootURL.stopAccessingSecurityScopedResource()
            }
        }

        let readmeURL = rootURL.appendingPathComponent("README.txt", isDirectory: false)
        let helloURL = rootURL
            .appendingPathComponent("Documents", isDirectory: true)
            .appendingPathComponent("hello.txt", isDirectory: false)
        try await requireContent(Self.readmeBytes, at: readmeURL)
        try await requireContent(Self.helloBytes, at: helloURL)
        report.complete("content-reopened")

        let materializedCount = try await waitForMaterializedRecord()
        report.materializedDirectoryCount = materializedCount
        report.complete("materialized-store-reopened")
        try await signalWorkingSet(manager)
        report.complete("working-set-signaled")

        try await removeDomainIfPresent()
        report.complete("domain-removed")
    }

    private func addDomain() async throws {
        let _: Void = try await waitForCallback(
            description: "domain registration"
        ) { completion in
            NSFileProviderManager.add(FixtureDomainConfiguration.domain) { error in
                if let error {
                    completion(.failure(error))
                } else {
                    completion(.success(()))
                }
            }
        }
    }

    private func removeDomainIfPresent() async throws {
        let domains = try await registeredDomains()
        guard domains.contains(where: {
            $0.identifier == FixtureDomainConfiguration.identifier
        }) else {
            return
        }
        let _: Void = try await waitForCallback(description: "domain removal") { completion in
            NSFileProviderManager.remove(FixtureDomainConfiguration.domain) { error in
                if let error {
                    completion(.failure(error))
                } else {
                    completion(.success(()))
                }
            }
        }
    }

    private func registeredDomains() async throws -> [NSFileProviderDomain] {
        try await waitForCallback(description: "registered domains") { completion in
            NSFileProviderManager.getDomainsWithCompletionHandler { domains, error in
                if let error {
                    completion(.failure(error))
                } else {
                    completion(.success(domains))
                }
            }
        }
    }

    private func waitForManager() async throws -> NSFileProviderManager {
        try await eventually(description: "File Provider manager") {
            NSFileProviderManager(for: FixtureDomainConfiguration.domain)
        }
    }

    private func userVisibleURL(
        _ manager: NSFileProviderManager,
        identifier: NSFileProviderItemIdentifier
    ) async throws -> URL {
        try await waitForCallback(description: "user-visible root URL") { completion in
            manager.getUserVisibleURL(for: identifier) { url, error in
                if let error {
                    completion(.failure(error))
                } else if let url {
                    completion(.success(url))
                } else {
                    completion(.failure(TestFailure.missingRootURL))
                }
            }
        }
    }

    private func validateIdentifier(for url: URL) async throws -> NSFileProviderItemIdentifier {
        try await waitForCallback(description: "identifier for \(url.lastPathComponent)") {
            completion in
            NSFileProviderManager.getIdentifierForUserVisibleFile(at: url) {
                identifier,
                domainIdentifier,
                error in
                if let error {
                    completion(.failure(error))
                    return
                }
                guard let identifier else {
                    completion(.failure(TestFailure.missingIdentifier(url)))
                    return
                }
                guard domainIdentifier == FixtureDomainConfiguration.identifier else {
                    completion(
                        .failure(
                            TestFailure.wrongDomain(
                            domainIdentifier?.rawValue ?? "<missing>"
                            )
                        )
                    )
                    return
                }
                completion(.success(identifier))
            }
        }
    }

    private func waitForStabilization(_ manager: NSFileProviderManager) async throws {
        let _: Void = try await waitForCallback(
            description: "domain stabilization"
        ) { completion in
            manager.waitForStabilization { error in
                if let error {
                    completion(.failure(error))
                } else {
                    completion(.success(()))
                }
            }
        }
    }

    private func signalWorkingSet(_ manager: NSFileProviderManager) async throws {
        let _: Void = try await waitForCallback(description: "working-set signal") { completion in
            MacosWorkingSetSignaler(manager: manager).signal { error in
                if let error {
                    completion(.failure(error))
                } else {
                    completion(.success(()))
                }
            }
        }
    }

    private func evict(
        _ manager: NSFileProviderManager,
        identifier: NSFileProviderItemIdentifier
    ) async throws {
        let _: Void = try await waitForCallback(description: "README.txt eviction") { completion in
            manager.evictItem(identifier: identifier) { error in
                if let error {
                    completion(.failure(error))
                } else {
                    completion(.success(()))
                }
            }
        }
    }

    private func waitForRootEnumeration(_ rootURL: URL) async throws -> [String] {
        try await eventually(description: "root enumeration") {
            guard let names = try? self.coordinatedRead(rootURL, accessor: { coordinatedURL in
                try FileManager.default.contentsOfDirectory(atPath: coordinatedURL.path)
            }) else {
                return nil
            }
            return names.contains("README.txt") && names.contains("Documents") ? names : nil
        }
    }

    private func requireContent(_ expected: Data, at url: URL) async throws {
        let actual = try await eventually(description: "hydration of \(url.lastPathComponent)") {
            try? self.coordinatedRead(url) { try Data(contentsOf: $0) }
        }
        guard actual == expected else {
            throw TestFailure.unexpectedContent(url)
        }
    }

    private func waitForMaterializedRecord() async throws -> Int {
        guard let appGroupURL = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: FixtureDomainConfiguration.appGroupIdentifier
        ) else {
            throw TestFailure.invalidMaterializedRecord
        }
        let recordURL = appGroupURL
            .appendingPathComponent("AsterForgeCloudFilesFixture/materialized", isDirectory: true)
            .appendingPathComponent("materialized-set-v1.json", isDirectory: false)
        return try await eventually(description: "materialized App Group record") {
            guard let data = try? Data(contentsOf: recordURL),
                  let record = try? JSONDecoder().decode(MaterializedRecord.self, from: data),
                  record.schemaVersion == 1,
                  !record.directoryIdentifiers.isEmpty,
                  record.syncAnchor == Self.currentAnchor
            else {
                return nil
            }
            return record.directoryIdentifiers.count
        }
    }

    private func coordinatedRead<Value>(
        _ url: URL,
        accessor: (URL) throws -> Value
    ) throws -> Value {
        let coordinator = NSFileCoordinator()
        var coordinationError: NSError?
        var result: Result<Value, Error>?
        coordinator.coordinate(readingItemAt: url, options: [], error: &coordinationError) {
            coordinatedURL in
            result = Result { try accessor(coordinatedURL) }
        }
        if let coordinationError {
            throw coordinationError
        }
        guard let result else {
            throw TestFailure.timeout("file coordination for \(url.lastPathComponent)")
        }
        return try result.get()
    }

    private func eventually<Value>(
        description: String,
        attempts: Int = 80,
        operation: () throws -> Value?
    ) async throws -> Value {
        for _ in 0 ..< attempts {
            if let value = try operation() {
                return value
            }
            try await Task.sleep(nanoseconds: 250_000_000)
        }
        throw TestFailure.timeout(description)
    }

    private func waitForCallback<Value>(
        description: String,
        operation: (@escaping (Result<Value, Error>) -> Void) -> Void
    ) async throws -> Value {
        try await withCheckedThrowingContinuation { continuation in
            let gate = MacosSystemTestCallbackGate(continuation: continuation)
            DispatchQueue.global().asyncAfter(deadline: .now() + 20) {
                gate.resolve(.failure(TestFailure.timeout(description)))
            }
            operation(gate.resolve)
        }
    }
}
