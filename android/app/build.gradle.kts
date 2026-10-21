import com.android.apksig.ApkSigner
import com.android.apksig.SigningCertificateLineage
import com.android.build.api.artifact.ArtifactTransformationRequest
import com.android.build.api.artifact.SingleArtifact
import java.security.KeyStore
import java.security.PrivateKey
import java.security.cert.X509Certificate
import java.util.Properties
import groovy.json.JsonSlurper

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.androidx.baseline.profile)
    alias(libs.plugins.compose.compiler)
    alias(libs.plugins.kotlin.serialization)
    alias(libs.plugins.aboutlibraries)
}

// When android/.signing/signing.properties exists (git-ignored), debug and release builds are signed
// with the key it describes, so every machine that has it produces the same signature and installs
// update in place. Release CI writes the directory from repository secrets; keep your own copy of
// the key somewhere safe, since Android only accepts updates signed with it.
// Keys (file names are relative to .signing/):
//   storeFile, storePassword, keyAlias, keyPassword   the app's key
//   lineageFile                                      optional APK Signature Scheme v3 rotation lineage
//                                                    from an earlier key (see RotateApkSignature)
//   previousStoreFile, previousStorePassword,        optional earlier key, needed for the v2 signature
//   previousKeyAlias, previousKeyPassword            when minSdk is below 28
// Without the file, builds fall back to the machine's own debug key; Android rejects such an APK as
// an update to an install signed with another key.
// The app has the same version as the Linux workspace (`version` in `[workspace.package]` of linux/Cargo.toml),
// which the release tag must equal. versionCode = major * 10000 + minor * 100 + patch, so it grows with every
// release; minor and patch stay below 100.
val appVersion: String = run {
    val cargo = rootProject.file("../linux/Cargo.toml")
    var section = ""
    // Through providers, so the configuration cache notices a version change.
    val toml = providers.fileContents(rootProject.layout.projectDirectory.file("../linux/Cargo.toml")).asText.get()
    val version = toml.lines().firstNotNullOfOrNull { line ->
        val text = line.substringBefore('#').trim()
        if (text.startsWith("[")) section = text
        Regex("""^version\s*=\s*"([^"]*)"$""").find(text)?.groupValues?.get(1)?.takeIf { section == "[workspace.package]" }
    } ?: throw GradleException("No version in [workspace.package] of $cargo")
    check(Regex("""(0|[1-9]\d*)\.(0|[1-9]\d?)\.(0|[1-9]\d?)""").matches(version)) {
        "$cargo: version \"$version\" must be MAJOR.MINOR.PATCH with MINOR and PATCH below 100"
    }
    version
}
val appVersionCode: Int = appVersion.split('.').map(String::toInt).let { (major, minor, patch) -> major * 10_000 + minor * 100 + patch }

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
        versionCode = appVersionCode
        versionName = appVersion

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        // The hostile-library stress test (@Stress) runs only with -Pstress, then alone; see AGENTS.md.
        val stress = "io.github.brainage04.tunemesh.Stress"
        testInstrumentationRunnerArguments[if (providers.gradleProperty("stress").isPresent) "annotation" else "notAnnotation"] = stress
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

// Licence notices of every library in the release APK: AboutLibraries collects them (licence
// texts included) and writeThirdPartyNotice turns that into assets/NOTICE.txt in every APK, also
// published next to the release APK.
aboutLibraries {
    collect {
        fetchRemoteLicense = false
        filterVariants.add("release")
    }
    export {
        variant = "release"
        outputFile = layout.buildDirectory.file("generated/aboutlibraries/aboutlibraries.json")
    }
    library {
        requireLicense = true
    }
}

val thirdPartyNotice = tasks.register<WriteThirdPartyNotice>("writeThirdPartyNotice") {
    dependsOn("exportLibraryDefinitions")
    definitions.set(aboutLibraries.export.outputFile)
    outputDirectory.set(layout.buildDirectory.dir("generated/notice"))
}

androidComponents {
    onVariants { variant ->
        variant.sources.assets?.addGeneratedSourceDirectory(
            thirdPartyNotice,
            WriteThirdPartyNotice::outputDirectory,
        )
    }
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
    androidTestImplementation(libs.androidx.test.uiautomator)

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

tasks.register("printVersion") {
    description = "Prints versionName and versionCode (from linux/Cargo.toml) without building."
    val name = appVersion
    val code = appVersionCode
    doLast {
        println("versionName=$name")
        println("versionCode=$code")
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

abstract class WriteThirdPartyNotice : DefaultTask() {
    @get:InputFile abstract val definitions: RegularFileProperty
    @get:OutputDirectory abstract val outputDirectory: DirectoryProperty

    @TaskAction
    @Suppress("UNCHECKED_CAST")
    fun write() {
        val json = JsonSlurper().parse(definitions.get().asFile) as Map<String, Any?>
        val libraries = (json["libraries"] as List<Map<String, Any?>>)
            .sortedBy { it["uniqueId"] as String }
        val licenses = json["licenses"] as Map<String, Map<String, Any?>>
        val text = buildString {
            appendLine("Third-party software in TuneMesh for Android")
            appendLine("============================================")
            appendLine()
            appendLine("TuneMesh is MIT licensed. The app includes the libraries below, each under")
            appendLine("the licence named; the full licence texts follow the list.")
            appendLine()
            for (library in libraries) {
                val ids = library["licenses"] as List<String>
                check(ids.isNotEmpty()) { "no licence found for ${library["uniqueId"]}" }
                val names = ids.joinToString(", ") { id -> licenses[id]?.get("name") as? String ?: id }
                appendLine("- ${library["name"]} (${library["uniqueId"]} ${library["artifactVersion"]}): $names")
                (library["website"] as? String)?.let { appendLine("  $it") }
            }
            for ((id, license) in licenses.toSortedMap()) {
                appendLine()
                appendLine("-".repeat(80))
                appendLine("${license["name"] ?: id}")
                (license["url"] as? String)?.let { appendLine(it) }
                appendLine()
                val content = license["content"] as? String
                checkNotNull(content) { "no licence text for $id" }
                appendLine(content.trim())
            }
        }
        val dir = outputDirectory.get().asFile
        dir.deleteRecursively()
        dir.mkdirs()
        dir.resolve("NOTICE.txt").writeText(text)
    }
}
