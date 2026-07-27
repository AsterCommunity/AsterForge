import AppKit
import Darwin
import FileProvider

@main
enum FixtureHostMain {
    static func main() {
        let application = NSApplication.shared
        let delegate = AppDelegate()
        application.delegate = delegate
        withExtendedLifetime(delegate) {
            application.run()
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let statusLabel = NSTextField(labelWithString: "")
    private var window: NSWindow?

    func applicationDidFinishLaunching(_: Notification) {
        if let command = MacosSystemTestCommand.requested(arguments: CommandLine.arguments),
           let phase = FixtureSystemTestPhase(rawValue: command.phase)
        {
            FixtureSystemTestRunner(phase: phase).run { report in
                do {
                    print(try report.encodedLine())
                } catch {
                    fputs("system test report encoding failed: \(error)\n", stderr)
                    fflush(stderr)
                    exit(EXIT_FAILURE)
                }
                fflush(stdout)
                exit(report.passed ? EXIT_SUCCESS : EXIT_FAILURE)
            }
            return
        }

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 640, height: 190),
            styleMask: [.titled, .closable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = FixtureDomainConfiguration.displayName
        window.center()
        window.contentView = makeContentView()
        window.makeKeyAndOrderFront(nil)
        self.window = window
        refreshStatus()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_: NSApplication) -> Bool {
        true
    }

    @objc private func addDomain() {
        NSFileProviderManager.add(FixtureDomainConfiguration.domain) { [weak self] error in
            DispatchQueue.main.async {
                self?.statusLabel.stringValue = error.map(Self.describe) ?? "Domain added"
            }
        }
    }

    @objc private func removeDomain() {
        NSFileProviderManager.remove(FixtureDomainConfiguration.domain) { [weak self] error in
            DispatchQueue.main.async {
                self?.statusLabel.stringValue = error.map(Self.describe) ?? "Domain removed"
            }
        }
    }

    @objc private func openDomain() {
        guard let manager = NSFileProviderManager(for: FixtureDomainConfiguration.domain) else {
            statusLabel.stringValue = "Domain is not registered"
            return
        }
        manager.getUserVisibleURL(for: .rootContainer) { [weak self] url, error in
            DispatchQueue.main.async {
                if let error {
                    self?.statusLabel.stringValue = Self.describe(error)
                } else if let url {
                    NSWorkspace.shared.open(url)
                }
            }
        }
    }

    @objc private func signalWorkingSet() {
        guard let manager = NSFileProviderManager(for: FixtureDomainConfiguration.domain) else {
            statusLabel.stringValue = "Domain is not registered"
            return
        }
        MacosWorkingSetSignaler(manager: manager).signal { [weak self] error in
            DispatchQueue.main.async {
                self?.statusLabel.stringValue = error.map(Self.describe)
                    ?? "Working set signaled"
            }
        }
    }

    private func refreshStatus() {
        NSFileProviderManager.getDomainsWithCompletionHandler { [weak self] domains, error in
            DispatchQueue.main.async {
                if let error {
                    self?.statusLabel.stringValue = Self.describe(error)
                    return
                }
                let isRegistered = domains.contains {
                    $0.identifier == FixtureDomainConfiguration.identifier
                }
                self?.statusLabel.stringValue = isRegistered ? "Domain registered" : "Domain absent"
            }
        }
    }

    private func makeContentView() -> NSView {
        let addButton = NSButton(title: "Add Domain", target: self, action: #selector(addDomain))
        let removeButton = NSButton(
            title: "Remove Domain",
            target: self,
            action: #selector(removeDomain)
        )
        let openButton = NSButton(title: "Open in Finder", target: self, action: #selector(openDomain))
        let signalButton = NSButton(
            title: "Signal Working Set",
            target: self,
            action: #selector(signalWorkingSet)
        )
        let buttons = NSStackView(views: [addButton, removeButton, openButton, signalButton])
        buttons.orientation = .horizontal
        buttons.spacing = 10

        statusLabel.font = .systemFont(ofSize: 13)
        statusLabel.textColor = .secondaryLabelColor

        let stack = NSStackView(views: [buttons, statusLabel])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 18
        stack.translatesAutoresizingMaskIntoConstraints = false

        let content = NSView()
        content.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 28),
            stack.trailingAnchor.constraint(lessThanOrEqualTo: content.trailingAnchor, constant: -28),
            stack.centerYAnchor.constraint(equalTo: content.centerYAnchor),
        ])
        return content
    }

    private static func describe(_ error: Error) -> String {
        (error as NSError).localizedDescription
    }
}
