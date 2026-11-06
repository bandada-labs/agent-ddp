plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// ddp's own APK for apps without AgentDDP: an instrumentation that serves the control protocol with UiAutomation.
android {
    namespace = "dev.agentddp.runner"
    compileSdk = 35
    defaultConfig {
        applicationId = "dev.agentddp.runner"
        minSdk = 26
        targetSdk = 35
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    sourceSets["main"].java.srcDir("../wire")
}
