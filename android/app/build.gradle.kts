plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "se.danielfalk.pegasus"
    compileSdk = 35

    defaultConfig {
        applicationId = "se.danielfalk.pegasus"
        minSdk = 26
        targetSdk = 35
        // CI passes the workflow run number so every Play upload is a new,
        // monotonically increasing versionCode; local builds default to 1.
        versionCode = (System.getenv("PEGASUS_VERSION_CODE") ?: "1").toInt()
        versionName = "1.0"
    }

    // Release signing comes entirely from the environment (CI decodes the
    // keystore secret to a temp file) — nothing signing-related in the repo.
    // Without the env vars, release builds are simply unsigned.
    signingConfigs {
        create("release") {
            val ks = System.getenv("PEGASUS_KEYSTORE_FILE")
            if (ks != null) {
                storeFile = file(ks)
                storePassword = System.getenv("PEGASUS_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("PEGASUS_KEY_ALIAS")
                keyPassword = System.getenv("PEGASUS_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            if (System.getenv("PEGASUS_KEYSTORE_FILE") != null) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
}

dependencies {
    implementation("androidx.webkit:webkit:1.12.1")
}
