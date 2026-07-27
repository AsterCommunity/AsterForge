import FileProvider
import Foundation

public final class MacosWorkingSetSignaler {
    private let signalAction: (@escaping (Error?) -> Void) -> Void

    public convenience init(manager: NSFileProviderManager) {
        self.init { completion in
            manager.signalEnumerator(for: .workingSet, completionHandler: completion)
        }
    }

    public init(signalAction: @escaping (@escaping (Error?) -> Void) -> Void) {
        self.signalAction = signalAction
    }

    public func signal(completion: @escaping (Error?) -> Void) {
        signalAction(completion)
    }
}
