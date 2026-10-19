// The Android app's pure-Kotlin library package (docs/library-format.md rules, LibraryIndex, M3u8,
// PlaylistStore, Downloads), compiled unchanged for the JVM together with its unit tests. A real migration
// would move these files into a Kotlin Multiplatform module that both apps depend on; pointing at the
// Android sources keeps this prototype from touching the Android app.
plugins {
    kotlin("jvm")
    kotlin("plugin.serialization")
}

val androidSources = rootDir.resolve("../android/app/src")
val libraryPackage = "java/io/github/brainage04/tunemesh/library"

sourceSets {
    main { kotlin.srcDir(androidSources.resolve("main/$libraryPackage")) }
    test { kotlin.srcDir(androidSources.resolve("test/$libraryPackage")) }
}

dependencies {
    api("org.jetbrains.kotlinx:kotlinx-serialization-json:1.11.0")
    testImplementation("junit:junit:4.13.2")
}
