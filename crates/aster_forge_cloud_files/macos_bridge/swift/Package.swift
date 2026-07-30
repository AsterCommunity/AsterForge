// swift-tools-version: 6.2

import PackageDescription

let package = Package(
    name: "AsterForgeCloudFilesMacosShell",
    platforms: [.macOS(.v13)],
    products: [
        .library(
            name: "AsterForgeCloudFilesMacosShell",
            targets: ["AsterForgeCloudFilesMacosShell"]
        ),
    ],
    targets: [
        .target(
            name: "AsterForgeCloudFilesMacosShell",
            path: "Sources/AsterForgeCloudFilesMacosShell"
        ),
        .target(
            name: "AsterForgeCloudFilesMacosExampleSupport",
            path: "Sources/AsterForgeCloudFilesMacosExampleSupport"
        ),
        .testTarget(
            name: "AsterForgeCloudFilesMacosShellTests",
            dependencies: ["AsterForgeCloudFilesMacosShell"],
            path: "Tests/AsterForgeCloudFilesMacosShellTests"
        ),
        .testTarget(
            name: "AsterForgeCloudFilesMacosExampleSupportTests",
            dependencies: ["AsterForgeCloudFilesMacosExampleSupport"],
            path: "Tests/AsterForgeCloudFilesMacosExampleSupportTests"
        ),
    ],
    swiftLanguageModes: [.v5]
)
