plugins {
    // AGP 9 ships built-in Kotlin: the org.jetbrains.kotlin.android plugin
    // must NOT be applied any more (it conflicts) — the Kotlin compiler
    // version now rides AGP. Gradle floor is 9.5.0 (the CI workflows pin it).
    id("com.android.application") version "9.3.1" apply false
}
