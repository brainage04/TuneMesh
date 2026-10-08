import com.android.apksig.ApkSigner
import com.android.apksig.SigningCertificateLineage
import com.android.build.api.artifact.ArtifactTransformationRequest
import com.android.build.api.artifact.SingleArtifact
import java.security.KeyStore
import java.security.PrivateKey
import java.security.cert.X509Certificate
import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.androidx.baseline.profile)
    alias(libs.plugins.compose.compiler)
    alias(libs.plugins.kotlin.serialization)
}

// Debug and release builds are signed with this app's own key, so every machine produces the same
// signature and installs update in place. The key lives in the git-ignored .signing/ directory,
// which the Borg backups include; Bitwarden keeps a copy in the item
// "Android signing key – TuneMesh".
// .signing/signing.properties describes it (file names are relative to .signing/):
//   storeFile, storePassword, keyAlias, keyPassword   this app's key
//   lineageFile                                      APK Signature Scheme v3 rotation lineage from
//                                                    the debug key that signed earlier installs
// Without the file, builds fall back to the machine's own debug key, which Android rejects as an
// update to the installed app.
val signing = rootProject.file(".signing/signing.properties")
    .takeIf { it.isFile }
    ?.let { file -> Properties().apply { file.inputStream().use(::load) } }

fun signingFile(key: String): File? =
    signing?.getProperty(key)?.let { rootProject.file(".signing/$it") }

android {
    namespace = "io.github.brainage04.tunemesh"
    compileSdk = 36

    defaultConfig {
        applicationId = "io.github.brainage04.tunemesh"
        minSdk = 30
        targetSdk = 36
        versionCode = 1
        versionName = "1.0.0"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    signingConfigs {
        if (signing != null) {
            create("project") {
                storeFile = signingFile("storeFile")
                storePassword = signing.getProperty("storePassword")
                keyAlias = signing.getProperty("keyAlias")
                keyPassword = signing.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        debug {
            if (signing != null) {
                signingConfig = signingConfigs.getByName("project")
            }
        }
        release {
            if (signing != null) {
                signingConfig = signingConfigs.getByName("project")
            }
            isMinifyEnabled = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures {
        compose = true
    }
    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }
}

kotlin {
    jvmToolchain(17)
}

baselineProfile {
    automaticGenerationDuringBuild = false
    dexLayoutOptimization = true
    mergeIntoMain = true
    saveInSrc = true
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.media3.exoplayer)
    implementation(libs.androidx.media3.session)
    implementation(libs.androidx.profileinstaller)

    implementation(libs.kotlinx.serialization.json)

    testImplementation(libs.junit)
    androidTestImplementation(libs.androidx.test.ext.junit)
    androidTestImplementation(libs.androidx.test.runner)

    debugImplementation(libs.androidx.compose.ui.tooling)
    baselineProfile(project(":baselineprofile"))
}

// With a lineage, every APK is re-signed after packaging: the v3 block carries this app's key plus
// the proof that the previous key handed over to it, so Android 9+ accepts the build as an update
// to an install signed with the previous key.
androidComponents {
    onVariants { variant ->
        val lineage = signingFile("lineageFile") ?: return@onVariants
        val properties = signing!!
        val rotate = tasks.register<RotateApkSignature>(
            "rotate${variant.name.replaceFirstChar(Char::uppercase)}ApkSignature",
        ) {
            lineageFile.set(lineage)
            storeFile.set(signingFile("storeFile"))
            storePassword.set(properties.getProperty("storePassword"))
            keyAlias.set(properties.getProperty("keyAlias"))
            keyPassword.set(properties.getProperty("keyPassword"))
            signingFile("previousStoreFile")?.let { previousStoreFile.set(it) }
            previousStorePassword.set(properties.getProperty("previousStorePassword"))
            previousKeyAlias.set(properties.getProperty("previousKeyAlias"))
            previousKeyPassword.set(properties.getProperty("previousKeyPassword"))
            minSdk.set(variant.minSdk.apiLevel)
        }
        val request = variant.artifacts.use(rotate)
            .wiredWithDirectories(RotateApkSignature::inputApks, RotateApkSignature::outputApks)
            .toTransformMany(SingleArtifact.APK)
        rotate.configure { transformationRequest.set(request) }
    }
}

abstract class RotateApkSignature : DefaultTask() {
    @get:InputFiles abstract val inputApks: DirectoryProperty
    @get:OutputDirectory abstract val outputApks: DirectoryProperty
    @get:InputFile abstract val lineageFile: RegularFileProperty
    @get:InputFile abstract val storeFile: RegularFileProperty
    @get:Input abstract val storePassword: Property<String>
    @get:Input abstract val keyAlias: Property<String>
    @get:Input abstract val keyPassword: Property<String>
    @get:InputFile @get:Optional abstract val previousStoreFile: RegularFileProperty
    @get:Input @get:Optional abstract val previousStorePassword: Property<String>
    @get:Input @get:Optional abstract val previousKeyAlias: Property<String>
    @get:Input @get:Optional abstract val previousKeyPassword: Property<String>
    @get:Input abstract val minSdk: Property<Int>
    @get:Internal
    abstract val transformationRequest: Property<ArtifactTransformationRequest<RotateApkSignature>>

    @TaskAction
    fun rotate() {
        // apksig signs v2 only with the lineage's oldest key; without that key the APK carries v3
        // alone, which Android 8.x and older cannot verify.
        val withPrevious = previousStoreFile.isPresent
        check(withPrevious || minSdk.get() >= 28) {
            "minSdk ${minSdk.get()} needs the previous key (previousStoreFile) for the v2 signature"
        }
        val signers = buildList {
            if (withPrevious) {
                add(
                    signer(
                        "previous",
                        previousStoreFile.get().asFile,
                        previousStorePassword.get(),
                        previousKeyAlias.get(),
                        previousKeyPassword.get(),
                    ),
                )
            }
            add(
                signer(
                    "current",
                    storeFile.get().asFile,
                    storePassword.get(),
                    keyAlias.get(),
                    keyPassword.get(),
                ),
            )
        }
        val lineage = SigningCertificateLineage.readFromFile(lineageFile.get().asFile)
        transformationRequest.get().submit(this) { artifact ->
            val input = File(artifact.outputFile)
            val output = outputApks.get().file(input.name).asFile
            ApkSigner.Builder(signers)
                .setSigningCertificateLineage(lineage)
                .setMinSdkVersion(minSdk.get())
                .setMinSdkVersionForRotation(28)
                .setV1SigningEnabled(false)
                .setV2SigningEnabled(withPrevious)
                .setV3SigningEnabled(true)
                .setInputApk(input)
                .setOutputApk(output)
                .build()
                .sign()
            output
        }
    }

    private fun signer(
        name: String,
        store: File,
        storePassword: String,
        alias: String,
        keyPassword: String,
    ): ApkSigner.SignerConfig {
        val keyStore = KeyStore.getInstance(store, storePassword.toCharArray())
        val key = keyStore.getKey(alias, keyPassword.toCharArray()) as PrivateKey
        val certificates = keyStore.getCertificateChain(alias).map { it as X509Certificate }
        return ApkSigner.SignerConfig.Builder(name, key, certificates).build()
    }
}
