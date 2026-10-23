plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
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
}

dependencies {
    // The app's own Compose version wins at resolution; this only sets a floor.
    implementation("androidx.compose.ui:ui:1.7.6")
    // Only for AgentDdpInterceptor; the app brings its own OkHttp, an app without it never loads that class.
    compileOnly("com.squareup.okhttp3:okhttp:4.12.0")
}
