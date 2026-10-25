plugins {
    alias(libs.plugins.aboutlibraries) apply false
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.android.test) apply false
    alias(libs.plugins.androidx.baseline.profile) apply false
    alias(libs.plugins.compose.compiler) apply false
    alias(libs.plugins.kotlin.serialization) apply false
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
