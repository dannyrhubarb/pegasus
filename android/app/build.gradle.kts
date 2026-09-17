plugins {
    // Kotlin is built into AGP 9 — no org.jetbrains.kotlin.android here.
    id("com.android.application")
}

android {
    namespace = "se.danielfalk.pegasus"
    compileSdk = 36

    defaultConfig {
        applicationId = "se.danielfalk.pegasus"
        minSdk = 26
        // Google Play requires targeting within 1 year of the latest Android
        // release (Android 16 / API 36 from 2026-08-31) or updates are blocked.
        targetSdk = 36
        // CI passes the workflow run number so every Play upload is a new,
        // monotonically increasing versionCode; local builds default to 1.
        versionCode = (System.getenv("PEGASUS_VERSION_CODE") ?: "1").toInt()
        // Marketing version = the release tag (tools/version.sh --marketing,
        // exported by CI as PEGASUS_VERSION_NAME); local builds read 0.0.0.
        versionName = System.getenv("PEGASUS_VERSION_NAME") ?: "0.0.0"
        // Launcher label, overridden by the preview build type so an
        // on-demand PR build is tellable from the real app in the launcher.
        manifestPlaceholders["appLabel"] = "Pegasus"
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
        val releaseType = getByName("release").apply {
            // R8 on (2026-09): shrinks the shell (Kotlin stdlib + androidx.webkit
            // are the bulk of the dex) and, load-bearing for Play, makes AGP
            // embed the deobfuscation map in the AAB
            // (BUNDLE-METADATA/com.android.tools.build.obfuscation/proguard.map)
            // — Play Console warned "no deobfuscation file associated with this
            // App Bundle" on every release while this was off. The map also
            // lands at build/outputs/mapping/release/mapping.txt, which the
            // release workflow attaches next to the AAB. proguard-rules.pro
            // keeps the JS bridge's method names (see it).
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
            if (System.getenv("PEGASUS_KEYSTORE_FILE") != null) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
        // On-demand PR test builds (android-test-apk.yml): the release build
        // under a DIFFERENT applicationId, so a tester can install it next to
        // the real app instead of replacing it — no uninstall, and the real
        // app keeps its localStorage (settings, pilot name, board cache).
        // initWith copies the release signing config, so successive PR builds
        // upgrade in place rather than tripping a signature mismatch.
        create("preview") {
            initWith(releaseType)
            applicationIdSuffix = ".preview"
            // CI passes "-pr<n>" so the About screen's App build row names the
            // PR the tester is actually running.
            versionNameSuffix = System.getenv("PEGASUS_VERSION_SUFFIX") ?: "-preview"
            manifestPlaceholders["appLabel"] = "Pegasus PR"
            matchingFallbacks += "release"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    // No kotlinOptions block: under built-in Kotlin, jvmTarget defaults to
    // compileOptions.targetCompatibility (17 above).
}

dependencies {
    implementation("androidx.webkit:webkit:1.17.0")
}
