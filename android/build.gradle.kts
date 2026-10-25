plugins {
    alias(libs.plugins.aboutlibraries) apply false
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.android.test) apply false
    alias(libs.plugins.androidx.baseline.profile) apply false
    alias(libs.plugins.compose.compiler) apply false
    alias(libs.plugins.kotlin.serialization) apply false
    alias(libs.plugins.detekt) apply false
    alias(libs.plugins.spotless)
}

// ktfmt in its kotlinlang style (the Kotlin coding conventions) for every Kotlin source and build script, at the
// line width the code already used. ./gradlew spotlessCheck fails on a difference, ./gradlew spotlessApply fixes it.
spotless {
    val ktfmtVersion = libs.versions.ktfmt.get()
    kotlin {
        target("**/*.kt")
        targetExclude("**/build/**")
        ktfmt(ktfmtVersion).kotlinlangStyle().configure { it.setMaxWidth(140) }
    }
    kotlinGradle {
        target("**/*.gradle.kts")
        targetExclude("**/build/**")
        ktfmt(ktfmtVersion).kotlinlangStyle().configure { it.setMaxWidth(140) }
    }
}

// detekt with type resolution on every module: the default rules plus config/detekt.yml. detekt-baseline-<variant>.xml in a module lists
// the style findings in code older than detekt (./gradlew detektBaselineDebug etc. rewrite them); any other finding fails the build.
subprojects {
    apply(plugin = "dev.detekt")
    extensions.configure<dev.detekt.gradle.extensions.DetektExtension> {
        buildUponDefaultConfig.set(true)
        config.setFrom(rootProject.file("config/detekt.yml"))
        baseline.set(file("detekt-baseline.xml"))
    }
}

tasks.register("detektAll") {
    description = "Runs detekt with type resolution on the app (main, unit and instrumented tests) and the baseline profile module."
    group = "verification"
    dependsOn(":app:detektDebug", ":app:detektDebugUnitTest", ":app:detektDebugAndroidTest", ":baselineprofile:detektNonMinifiedRelease")
}
