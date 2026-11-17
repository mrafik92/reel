import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

// Optional release signing, from either
//  - clients/android/keystore.properties (git-ignored; local builds) with storeFile, storePassword,
//    keyAlias, keyPassword, or
//  - env REEL_KEYSTORE_FILE, REEL_KEYSTORE_PASSWORD, REEL_KEY_ALIAS, REEL_KEY_PASSWORD (the
//    android-release workflow, #195; keeps the passwords off disk).
// Without either, release builds are unsigned.
val keystoreProps = Properties().apply {
    val f = rootProject.file("keystore.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}
fun signingValue(prop: String, env: String): String? =
    System.getenv(env)?.takeIf { it.isNotEmpty() } ?: keystoreProps.getProperty(prop)
val releaseStoreFile = signingValue("storeFile", "REEL_KEYSTORE_FILE")

android {
    namespace = "app.reel.android"
    compileSdk = 35

    defaultConfig {
        applicationId = "app.reel.android"
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
        compose = true
        buildConfig = true
    }
    lint {
        warningsAsErrors = false
        abortOnError = true
    }
    testOptions {
        unitTests.all {
            // The model drift test reads the server's OpenAPI document (#186).
            it.systemProperty("reel.openapi", rootProject.file("../../server/openapi.json").absolutePath)
        }
    }
}

dependencies {
    // The catalog also pins navigation + Coil (#187) and Media3 (#190); they're added with those tickets.
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.datastore.preferences)

    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    debugImplementation(libs.compose.ui.tooling)

    implementation(libs.okhttp)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.kotlinx.coroutines.android)

    testImplementation(libs.junit)
    testImplementation(libs.okhttp.mockwebserver)
    testImplementation(libs.kotlinx.coroutines.test)
}
