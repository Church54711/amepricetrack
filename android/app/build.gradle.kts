plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// The app and the server share one version number, kept in the server's Cargo.toml.
val appVersion: String = Regex("""(?m)^version = "(\d+)\.(\d+)\.(\d+)"""")
    .find(rootProject.file("../crates/server/Cargo.toml").readText())
    ?.groupValues?.drop(1)?.joinToString(".")
    ?: error("No version found in crates/server/Cargo.toml")

// Where releases are published; the app looks here for a newer version of itself.
val releasesRepo: String = (findProperty("apt.releases.repo") as String?) ?: "Church54711/amepricetrack"

// The release key stays outside the repository. It is named by environment
// variables (on the build server) or in ~/.gradle/gradle.properties (at home).
fun secret(env: String, property: String): String? = System.getenv(env)?.takeIf { it.isNotBlank() } ?: findProperty(property) as String?
val keystorePath = secret("APT_KEYSTORE_FILE", "apt.keystore.file")

android {
    namespace = "app.amepricetrack"
    compileSdk = 36

    defaultConfig {
        applicationId = "app.amepricetrack"
        minSdk = 26
        targetSdk = 36
        // 1.2.3 becomes 10203, so a newer version always has a higher code.
        versionCode = appVersion.split(".").map(String::toInt).let { (major, minor, patch) -> major * 10000 + minor * 100 + patch }
        versionName = appVersion
        buildConfigField("String", "RELEASES_REPO", "\"$releasesRepo\"")
    }
    signingConfigs {
        if (keystorePath != null) {
            create("release") {
                storeFile = file(keystorePath)
                storePassword = secret("APT_KEYSTORE_PASSWORD", "apt.keystore.password")
                keyAlias = secret("APT_KEY_ALIAS", "apt.key.alias") ?: "amepricetrack"
                keyPassword = secret("APT_KEY_PASSWORD", "apt.key.password") ?: secret("APT_KEYSTORE_PASSWORD", "apt.keystore.password")
            }
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            // Without the key the release build is left unsigned and cannot be installed.
            signingConfig = signingConfigs.findByName("release")
        }
    }
    buildFeatures {
        compose = true
        buildConfig = true
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    testOptions { unitTests.isReturnDefaultValues = true }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2025.12.01"))
    implementation("androidx.activity:activity-compose:1.12.2")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material:material-icons-core")
    implementation("androidx.compose.ui:ui-tooling-preview")
    debugImplementation("androidx.compose.ui:ui-tooling")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.9.4")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.4")
    implementation("androidx.core:core-ktx:1.17.0")
    implementation("androidx.work:work-runtime-ktx:2.11.2")
    // The home-screen widget.
    implementation("androidx.glance:glance-appwidget:1.1.1")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    implementation("io.coil-kt:coil-compose:2.7.0")
    // Google's barcode scanner screen: no camera permission needed in this app.
    implementation("com.google.android.gms:play-services-code-scanner:16.1.0")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20251224")
    testImplementation("com.squareup.okhttp3:mockwebserver:4.12.0")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
}
