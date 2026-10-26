plugins {
    alias(libs.plugins.android.test)
    alias(libs.plugins.androidx.baseline.profile)
}

android {
    namespace = "io.github.brainage04.tunemesh.baselineprofile"
    compileSdk = 36

    defaultConfig {
        minSdk = 30
        targetSdk = 36
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        // The emulator is the only test device (see AGENTS.md): its numbers compare builds, not phones.
        testInstrumentationRunnerArguments["androidx.benchmark.suppressErrors"] = "EMULATOR"
    }

    targetProjectPath = ":app"
    experimentalProperties["android.experimental.self-instrumenting"] = true

    // The hostile-library generator, shared with the app's instrumented tests.
    sourceSets.getByName("main").kotlin.srcDir("../hostile-library/java")
    sourceSets.getByName("main").kotlin.srcDir("../hostile-library/common")

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    jvmToolchain(17)
}

baselineProfile {
    useConnectedDevices = true
}

dependencies {
    implementation(libs.androidx.benchmark.macro.junit4)
    implementation(libs.androidx.test.ext.junit)
    implementation(libs.androidx.test.runner)
    implementation(libs.androidx.test.uiautomator)
}
