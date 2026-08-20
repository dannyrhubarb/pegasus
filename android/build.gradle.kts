plugins {
    // 8.9.1 is the first AGP with compileSdk 36 support; CI's Gradle 8.14.3
    // covers this version's Gradle floor (8.13).
    id("com.android.application") version "8.13.2" apply false
    id("org.jetbrains.kotlin.android") version "2.0.21" apply false
}
