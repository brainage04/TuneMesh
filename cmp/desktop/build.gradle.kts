import org.jetbrains.compose.desktop.application.dsl.TargetFormat

plugins {
    kotlin("jvm")
    kotlin("plugin.serialization")
    kotlin("plugin.compose")
    id("org.jetbrains.compose")
}

// The Android app's theme and icon set are plain Compose code with no Android imports: they are compiled
// unchanged from the Android source tree (copied into a generated source directory so the rest of the Android
// `ui` package, which uses Media3 and lifecycle APIs, stays out).
val androidUi = rootDir.resolve("../android/app/src/main/java/io/github/brainage04/tunemesh/ui")
val sharedAndroidUi = tasks.register<Sync>("sharedAndroidUi") {
    from(androidUi) {
        include("Icons.kt", "theme/AppTheme.kt")
    }
    into(layout.buildDirectory.dir("generated/androidUi/io/github/brainage04/tunemesh/ui"))
}
sourceSets.main {
    kotlin.srcDir(sharedAndroidUi.map { layout.buildDirectory.dir("generated/androidUi").get() })
}

dependencies {
    implementation(project(":shared"))
    implementation(compose.desktop.currentOs)
    implementation("org.jetbrains.compose.material3:material3:1.9.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-swing:1.11.0")
    implementation("net.java.dev.jna:jna:5.19.1")
    implementation("net.jthink:jaudiotagger:3.0.1")
}

// Directory holding libmpv.so.2 when it is not in the dynamic linker's default path (NixOS); baked into the
// launcher's JVM options. On other distributions libmpv from the package manager is found without it.
val libmpvDir = providers.gradleProperty("libmpvDir").orNull

compose.desktop {
    application {
        mainClass = "io.github.brainage04.tunemesh.desktop.MainKt"
        jvmArgs += listOf("-Dfile.encoding=UTF-8", "-Dawt.useSystemAAFontSettings=on", "--enable-native-access=ALL-UNNAMED")
        providers.gradleProperty("appJvmArgs").orNull?.split(' ')?.filter { it.isNotBlank() }?.let { jvmArgs += it }
        if (libmpvDir != null) jvmArgs += "-Djna.library.path=$libmpvDir"

        nativeDistributions {
            targetFormats(TargetFormat.AppImage)
            packageName = "tunemesh-cmp"
            packageVersion = "0.1.0"
            description = "TuneMesh Compose Multiplatform desktop prototype"
            vendor = "brainage04"
            modules("java.logging", "java.naming", "jdk.unsupported", "java.management")
        }

        buildTypes.release.proguard {
            isEnabled = true
            obfuscate = false
            optimize = true
            configurationFiles.from(project.file("proguard-rules.pro"))
        }
    }
}
