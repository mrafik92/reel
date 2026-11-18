import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
}

// Optional release signing, same sources as the Compose app (clients/android):
// clients/android-web/keystore.properties (git-ignored) or the REEL_KEYSTORE_* / REEL_KEY_* env vars.
// Without either, release builds are unsigned.
val keystoreProps = Properties().apply {
    val f = rootProject.file("keystore.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}
fun signingValue(prop: String, env: String): String? =
    System.getenv(env)?.takeIf { it.isNotEmpty() } ?: keystoreProps.getProperty(prop)
val releaseStoreFile = signingValue("storeFile", "REEL_KEYSTORE_FILE")

android {
    // A different package from the Compose app (app.reel.android), so both install side by side (#225).
    namespace = "app.reel.android.web"
    compileSdk = 35

    defaultConfig {
        applicationId = "app.reel.android.web"
        minSdk = 24
        targetSdk = 35
        versionCode = (System.getenv("REEL_VERSION_CODE") ?: "1").toInt()
        versionName = System.getenv("REEL_VERSION_NAME") ?: "0.1.0"
        // Optional default server baked in at build time (like VITE_REEL_URL for the TV apps).
        buildConfigField("String", "DEFAULT_SERVER", "\"${System.getenv("REEL_URL") ?: ""}\"")
    }

    signingConfigs {
        if (releaseStoreFile != null) {
            create("release") {
                storeFile = rootProject.file(releaseStoreFile)
                storePassword = signingValue("storePassword", "REEL_KEYSTORE_PASSWORD")
                keyAlias = signingValue("keyAlias", "REEL_KEY_ALIAS")
                keyPassword = signingValue("keyPassword", "REEL_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfigs.findByName("release")?.let { signingConfig = it }
        }
        debug {
            applicationIdSuffix = ".debug"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildFeatures {
        buildConfig = true
    }
    lint {
        warningsAsErrors = false
        abortOnError = true
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity)
    // Only for HttpUrl parsing and the server check on the setup screen; the WebView has its own stack.
    implementation(libs.okhttp)

    testImplementation(libs.junit)
    testImplementation(libs.okhttp.mockwebserver)
}
