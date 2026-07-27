import Foundation

final class RustMacosBridgeRequestLease: MacosBridgeRequestLease {
    let generation: UInt64
    private let lock = NSLock()
    private var handle: AsterForgeMacosRequestHandle?

    init(handle: AsterForgeMacosRequestHandle, generation: UInt64) {
        self.handle = handle
        self.generation = generation
    }

    func release() {
        lock.lock()
        guard let handle else {
            lock.unlock()
            return
        }
        self.handle = nil
        lock.unlock()
        aster_forge_cloud_files_macos_request_release(handle)
    }

    deinit {
        release()
    }
}

final class RustMacosBridgeSession: MacosBridgeSession {
    let generation: UInt64
    private let lock = NSLock()
    private var handle: AsterForgeMacosSessionHandle?

    init(generation: UInt64) throws {
        guard generation != 0 else {
            throw MacosBridgeFailure(code: .invalidArgument)
        }
        let result = aster_forge_cloud_files_macos_session_create(generation)
        guard Self.code(result.code) == .success, result.handle.raw != nil else {
            throw MacosBridgeFailure(code: Self.code(result.code))
        }
        self.generation = generation
        handle = result.handle
    }

    func beginRequest() throws -> any MacosBridgeRequestLease {
        lock.lock()
        guard let handle else {
            lock.unlock()
            throw MacosBridgeFailure(code: .providerNotFound)
        }
        let result = aster_forge_cloud_files_macos_session_begin_request(handle, generation)
        lock.unlock()
        guard Self.code(result.code) == .success, result.handle.raw != nil else {
            throw MacosBridgeFailure(code: Self.code(result.code))
        }
        return RustMacosBridgeRequestLease(handle: result.handle, generation: generation)
    }

    func beginClosing() {
        withHandle { handle in
            _ = aster_forge_cloud_files_macos_session_begin_closing(handle)
        }
    }

    func markDisconnected() {
        withHandle { handle in
            _ = aster_forge_cloud_files_macos_session_mark_disconnected(handle)
        }
    }

    deinit {
        lock.lock()
        guard let handle else {
            lock.unlock()
            return
        }
        self.handle = nil
        lock.unlock()
        _ = aster_forge_cloud_files_macos_session_begin_closing(handle)
        _ = aster_forge_cloud_files_macos_session_mark_disconnected(handle)
        aster_forge_cloud_files_macos_session_release(handle)
    }

    private func withHandle(_ action: (AsterForgeMacosSessionHandle) -> Void) {
        lock.lock()
        guard let handle else {
            lock.unlock()
            return
        }
        action(handle)
        lock.unlock()
    }

    private static func code(_ code: AsterForgeMacosErrorCode) -> MacosBridgeErrorCode {
        MacosBridgeErrorCode(rawValue: Int32(code.rawValue)) ?? .internal
    }
}

enum RustMacosIdentifierCodec {
    static func encode(namespace: String, root: String, item: String) throws -> String {
        let namespaceBytes = Data(namespace.utf8)
        let rootBytes = Data(root.utf8)
        let itemBytes = Data(item.utf8)
        let result = namespaceBytes.withUnsafeBytes { namespaceBuffer in
            rootBytes.withUnsafeBytes { rootBuffer in
                itemBytes.withUnsafeBytes { itemBuffer in
                    aster_forge_cloud_files_macos_identifier_encode(
                        namespaceBuffer.bindMemory(to: UInt8.self).baseAddress,
                        namespaceBuffer.count,
                        rootBuffer.bindMemory(to: UInt8.self).baseAddress,
                        rootBuffer.count,
                        itemBuffer.bindMemory(to: UInt8.self).baseAddress,
                        itemBuffer.count
                    )
                }
            }
        }
        defer { aster_forge_cloud_files_macos_buffer_release(result.buffer) }
        let code = MacosBridgeErrorCode(rawValue: Int32(result.code.rawValue)) ?? .internal
        guard code == .success, let pointer = result.buffer.ptr else {
            throw MacosBridgeFailure(code: code)
        }
        let data = Data(bytes: pointer, count: result.buffer.len)
        guard let identifier = String(data: data, encoding: .utf8) else {
            throw MacosBridgeFailure(code: .internal)
        }
        return identifier
    }
}

final class UnavailableMacosBridgeSession: MacosBridgeSession {
    let generation: UInt64 = 1
    private let error: Error

    init(error: Error) {
        self.error = error
    }

    func beginRequest() throws -> any MacosBridgeRequestLease { throw error }
    func beginClosing() {}
    func markDisconnected() {}
}
