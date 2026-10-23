// swift-tools-version:5.9
// For adding the library by URL. ios/AgentDDP has the same package for a local path.
import PackageDescription

let package = Package(
    name: "AgentDDP",
    platforms: [.iOS(.v15)],
    products: [.library(name: "AgentDDP", targets: ["AgentDDP"])],
    targets: [
        .target(name: "AgentDDP", path: "ios/AgentDDP/Sources/AgentDDP"),
        .testTarget(name: "AgentDDPTests", dependencies: ["AgentDDP"], path: "ios/AgentDDP/Tests/AgentDDPTests"),
    ]
)
