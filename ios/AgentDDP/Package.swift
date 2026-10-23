// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "AgentDDP",
    platforms: [.iOS(.v15)],
    products: [.library(name: "AgentDDP", targets: ["AgentDDP"])],
    targets: [.target(name: "AgentDDP")]
)
