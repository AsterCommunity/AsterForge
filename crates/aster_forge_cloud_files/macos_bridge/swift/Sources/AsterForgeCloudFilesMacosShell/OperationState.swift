import Foundation

final class MacosTerminalGate {
    private let lock = NSLock()
    private var finished = false

    @discardableResult
    func finish(_ action: () -> Void) -> Bool {
        lock.lock()
        guard !finished else {
            lock.unlock()
            return false
        }
        finished = true
        lock.unlock()
        action()
        return true
    }
}

final class MacosCancellationSlot {
    private let lock = NSLock()
    private var operation: (any MacosCancellable)?
    private var cancellationAction: (() -> Void)?
    private var cancelled = false
    private var completed = false

    init(onCancel cancellationAction: (() -> Void)? = nil) {
        self.cancellationAction = cancellationAction
    }

    func install(_ operation: any MacosCancellable) {
        lock.lock()
        if cancelled {
            lock.unlock()
            operation.cancel()
            return
        }
        guard !completed else {
            lock.unlock()
            return
        }
        self.operation = operation
        lock.unlock()
    }

    func complete() {
        lock.lock()
        guard !cancelled, !completed else {
            lock.unlock()
            return
        }
        completed = true
        operation = nil
        cancellationAction = nil
        lock.unlock()
    }

    func cancel() {
        lock.lock()
        guard !cancelled, !completed else {
            lock.unlock()
            return
        }
        cancelled = true
        let operation = operation
        let cancellationAction = cancellationAction
        self.operation = nil
        self.cancellationAction = nil
        lock.unlock()
        operation?.cancel()
        cancellationAction?()
    }
}

final class MacosCancellationRegistry {
    private let lock = NSLock()
    private var slots: [UUID: MacosCancellationSlot] = [:]
    private var invalidated = false

    func insert(_ slot: MacosCancellationSlot) -> UUID? {
        lock.lock()
        defer { lock.unlock() }
        guard !invalidated else { return nil }
        let identifier = UUID()
        slots[identifier] = slot
        return identifier
    }

    func remove(_ identifier: UUID) {
        lock.lock()
        slots.removeValue(forKey: identifier)
        lock.unlock()
    }

    func invalidate() {
        lock.lock()
        invalidated = true
        let current = Array(slots.values)
        slots.removeAll()
        lock.unlock()
        current.forEach { $0.cancel() }
    }
}
