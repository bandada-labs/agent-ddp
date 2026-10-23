pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }
dependencyResolutionManagement { repositories { google(); mavenCentral() } }
rootProject.name = "DDPSample"
include(":app")
include(":agent-ddp")
project(":agent-ddp").projectDir = file("../../android/agent-ddp")
