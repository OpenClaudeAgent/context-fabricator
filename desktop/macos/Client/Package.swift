// swift-tools-version: 6.3

import PackageDescription

let package = Package(
  name: "LocalAPI",
  platforms: [.macOS(.v14)],
  products: [
    .library(name: "LocalAPI", targets: ["LocalAPI"]),
    .library(name: "DesktopFeature", targets: ["DesktopFeature"]),
  ],
  dependencies: [
    .package(url: "https://github.com/apple/swift-http-types", exact: "1.8.0"),
    .package(url: "https://github.com/apple/swift-openapi-generator", exact: "1.14.4"),
    .package(url: "https://github.com/apple/swift-openapi-runtime", exact: "1.13.4"),
    .package(url: "https://github.com/apple/swift-openapi-urlsession", exact: "1.4.0"),
    .package(url: "https://github.com/pointfreeco/swift-composable-architecture", exact: "1.25.5"),
  ],
  targets: [
    .target(
      name: "LocalAPI",
      dependencies: [
        .product(name: "HTTPTypes", package: "swift-http-types"),
        .product(name: "OpenAPIRuntime", package: "swift-openapi-runtime"),
        .product(name: "OpenAPIURLSession", package: "swift-openapi-urlsession"),
      ],
      plugins: [.plugin(name: "OpenAPIGenerator", package: "swift-openapi-generator")]
    ),
    .target(
      name: "DesktopFeature",
      dependencies: [
        "LocalAPI",
        .product(name: "ComposableArchitecture", package: "swift-composable-architecture"),
      ]
    ),
    .testTarget(
      name: "DesktopFeatureTests",
      dependencies: [
        "DesktopFeature", "LocalAPI",
        .product(name: "ComposableArchitecture", package: "swift-composable-architecture"),
      ],
      swiftSettings: [.unsafeFlags(["-warnings-as-errors"])]
    ),
  ],
  swiftLanguageModes: [.v6]
)
