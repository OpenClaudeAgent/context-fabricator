// swift-tools-version: 6.3

import PackageDescription

let package = Package(
  name: "M01QualityFixture",
  platforms: [.macOS(.v14)],
  dependencies: [
    .package(
      url: "https://github.com/pointfreeco/swift-composable-architecture",
      exact: "1.25.5"
    )
  ],
  targets: [
    .target(
      name: "QualityFixture",
      dependencies: [
        .product(name: "ComposableArchitecture", package: "swift-composable-architecture")
      ],
      swiftSettings: [.unsafeFlags(["-warnings-as-errors"])]
    ),
    .testTarget(
      name: "QualityFixtureTests",
      dependencies: [
        "QualityFixture",
        .product(name: "ComposableArchitecture", package: "swift-composable-architecture"),
      ],
      swiftSettings: [.unsafeFlags(["-warnings-as-errors"])]
    ),
  ],
  swiftLanguageModes: [.v6]
)
