plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
    `maven-publish`
}

android {
    namespace = "dev.agentddp"
    compileSdk = 35
    defaultConfig { minSdk = 26 }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    publishing { singleVariant("release") { withSourcesJar() } }
}

dependencies {
    // The app's own Compose version wins at resolution; this only sets a floor.
    implementation("androidx.compose.ui:ui:1.7.6")
    // Only for AgentDdpInterceptor; the app brings its own OkHttp, an app without it never loads that class.
    compileOnly("com.squareup.okhttp3:okhttp:4.12.0")
}

// What JitPack builds from a tag (jitpack.yml): com.github.bandada-labs:agent-ddp:<tag>.
afterEvaluate {
    publishing {
        publications {
            create<MavenPublication>("release") {
                from(components["release"])
                groupId = "com.github.bandada-labs"
                artifactId = "agent-ddp"
                version = System.getenv("VERSION") ?: "dev"
            }
        }
    }
}
