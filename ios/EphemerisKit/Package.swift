// swift-tools-version: 6.0
// The engine for the iOS app (docs/phase3-design.md §2): the Rust core as an
// XCFramework, its generated Swift bindings, and the Swift around them. Run
// scripts/build-ios.sh first; it writes the XCFramework, the bindings and
// the golden test files, all gitignored.

import PackageDescription

let package = Package(
    name: "EphemerisKit",
    // macOS so `swift test` runs on the Mac with no simulator.
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "EphemerisKit", targets: ["EphemerisKit"]),
    ],
    targets: [
        .binaryTarget(name: "EphemerisFFI", path: "EphemerisFFI.xcframework"),
        .target(name: "EphemerisKit", dependencies: ["EphemerisFFI"]),
        .testTarget(
            name: "EphemerisKitTests",
            dependencies: ["EphemerisKit"],
            resources: [.copy("Golden")]
        ),
    ]
)
