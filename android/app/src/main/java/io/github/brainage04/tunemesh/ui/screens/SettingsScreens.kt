package io.github.brainage04.tunemesh.ui.screens

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import io.github.brainage04.tunemesh.data.AppSettings
import io.github.brainage04.tunemesh.ui.AppIcons
import io.github.brainage04.tunemesh.ui.BackBar
import io.github.brainage04.tunemesh.ui.LocalAppViewModel
import io.github.brainage04.tunemesh.ui.LocalLibrary
import io.github.brainage04.tunemesh.ui.theme.Accent
import io.github.brainage04.tunemesh.ui.theme.SurfaceHigh
import io.github.brainage04.tunemesh.ui.theme.TextSecondary

@Composable
fun SettingsScreen(contentPadding: PaddingValues) {
    val viewModel = LocalAppViewModel.current
    val library = LocalLibrary.current
    var path by rememberSaveable(library.root.absolutePath) { mutableStateOf(library.root.absolutePath) }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = contentPadding.calculateBottomPadding())) {
        BackBar("Settings", onBack = { viewModel.back() })
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Library folder", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            Text(
                "The Syncthing folder TuneMesh reads songs and playlists from. Created if it does not exist.",
                color = TextSecondary,
                style = MaterialTheme.typography.bodyMedium,
            )
            OutlinedTextField(value = path, onValueChange = { path = it }, singleLine = true, modifier = Modifier.fillMaxWidth())
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(
                    onClick = { viewModel.repository.setLibraryRoot(path) },
                    enabled = path.isNotBlank() && path.trim() != library.root.absolutePath,
                    colors = ButtonDefaults.buttonColors(containerColor = Accent, contentColor = Color.Black),
                ) { Text("Save") }
                OutlinedButton(onClick = { path = AppSettings.defaultRoot().absolutePath }) { Text("Default") }
                OutlinedButton(onClick = { viewModel.repository.refreshAsync() }) { Text(if (library.scanning) "Scanning…" else "Rescan") }
            }
            library.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            Spacer(Modifier.height(12.dp))
            Text("Library", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            Text(
                "${library.index.tracks.size} songs • ${library.index.albums.size} albums • ${library.index.artists.size} artists • " +
                    "${library.playlists.size} playlists",
                color = TextSecondary,
            )
            Text("All files access: ${if (library.hasAccess) "granted" else "not granted"}", color = TextSecondary)
            Text("Requests are sent as \"${viewModel.app.settings.deviceName}\"", color = TextSecondary)
            Text(library.downloader.description, color = TextSecondary)
            Spacer(Modifier.height(12.dp))
            Text("Syncthing", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            Text(
                "How to get the library from your computer onto this phone with Syncthing-Fork.",
                color = TextSecondary,
                style = MaterialTheme.typography.bodyMedium,
            )
            OutlinedButton(onClick = { viewModel.openSyncthingSetup() }) { Text("Syncthing setup") }
        }
    }
}

/** Shown until the user grants All files access, which TuneMesh needs to read and write the Syncthing folder. */
@Composable
fun OnboardingScreen() {
    val context = LocalContext.current
    Column(
        Modifier.fillMaxSize().systemBarsPadding().padding(32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Text("Step 1 of 2", color = TextSecondary, style = MaterialTheme.typography.labelLarge)
        Spacer(Modifier.height(12.dp))
        Icon(AppIcons.Library, null, tint = Accent, modifier = Modifier.size(72.dp))
        Spacer(Modifier.height(24.dp))
        Text("Allow access to your music", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold, textAlign = TextAlign.Center)
        Spacer(Modifier.height(12.dp))
        Text(
            "TuneMesh plays the songs Syncthing keeps in ${AppSettings.defaultRoot().absolutePath} and writes playlists, " +
                "likes and download requests there. Android only allows that with \"All files access\".\n\n" +
                "On the next screen, switch on \"Allow access to manage all files\" for TuneMesh, then come back.",
            color = TextSecondary,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(32.dp))
        Button(
            onClick = {
                val specific = Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:${context.packageName}"))
                try {
                    context.startActivity(specific)
                } catch (_: ActivityNotFoundException) {
                    context.startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
                }
            },
            colors = ButtonDefaults.buttonColors(containerColor = Accent, contentColor = Color.Black),
            modifier = Modifier.fillMaxWidth().height(52.dp),
        ) { Text("Open settings", fontWeight = FontWeight.Bold) }
    }
}

/** Syncthing-Fork's package on F-Droid and GitHub, then the older one still used on Google Play. */
private val SYNCTHING_PACKAGES = listOf("com.github.catfriend1.syncthingfork", "com.github.catfriend1.syncthingandroid")
private const val SYNCTHING_FDROID = "https://f-droid.org/packages/com.github.catfriend1.syncthingfork/"
private const val SYNCTHING_GITHUB = "https://github.com/researchxxl/syncthing-android/releases/latest"

private fun syncthingLaunchIntent(context: Context): Intent? =
    SYNCTHING_PACKAGES.firstNotNullOfOrNull { context.packageManager.getLaunchIntentForPackage(it) }

private fun openUrl(context: Context, url: String) {
    try {
        context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
    } catch (_: ActivityNotFoundException) {
    }
}

/**
 * Second onboarding step, after All files access: how to sync the library folder with Syncthing-Fork. Skippable,
 * shown once, and reachable again from Settings. TuneMesh itself never talks to Syncthing.
 */
@Composable
fun SyncthingSetupScreen(onDone: () -> Unit) {
    val context = LocalContext.current
    val viewModel = LocalAppViewModel.current
    val firstRun = remember { !viewModel.app.settings.syncthingSetupSeen }
    var launch by remember { mutableStateOf(syncthingLaunchIntent(context)) }
    LifecycleResumeEffect(Unit) {
        launch = syncthingLaunchIntent(context)
        onPauseOrDispose {}
    }
    val root = AppSettings.defaultRoot().absolutePath
    Column(Modifier.fillMaxSize().systemBarsPadding().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp, vertical = 32.dp)) {
        if (firstRun) {
            Text("Step 2 of 2", color = TextSecondary, style = MaterialTheme.typography.labelLarge)
            Spacer(Modifier.height(12.dp))
        }
        Icon(AppIcons.Sync, null, tint = Accent, modifier = Modifier.size(56.dp))
        Spacer(Modifier.height(16.dp))
        Text("Sync your library", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
        Spacer(Modifier.height(8.dp))
        Text(
            "TuneMesh plays what Syncthing copies to this phone. With Syncthing on your computer and Syncthing-Fork " +
                "here, both keep the same library folder.",
            color = TextSecondary,
        )
        Spacer(Modifier.height(20.dp))
        SetupStep(1, "Install Syncthing-Fork", "Get it from F-Droid or from its GitHub releases.") {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = { openUrl(context, SYNCTHING_FDROID) }) { Text("F-Droid") }
                OutlinedButton(onClick = { openUrl(context, SYNCTHING_GITHUB) }) { Text("GitHub") }
            }
        }
        SetupStep(
            2,
            "Connect to your computer",
            "On the computer, open Syncthing (http://127.0.0.1:8384) and choose Actions → Show ID. In Syncthing-Fork, " +
                "open Devices, tap + and scan that QR code. Accept the phone on the computer when Syncthing asks.",
        )
        SetupStep(
            3,
            "Accept the tunemesh folder",
            "On the computer, share the folder with ID \"tunemesh\" with the phone. When Syncthing-Fork offers it, " +
                "accept it and set its directory to $root.",
        )
        SetupStep(
            4,
            "Turn on \"Ignore permissions\"",
            "Android reports the same permissions for every file; without this, Syncthing keeps syncing them back " +
                "as changes. Syncthing-Fork turns it on for folders it adds; in its Web GUI it is under Edit → Advanced.",
        )
        Spacer(Modifier.height(12.dp))
        launch?.let { intent ->
            OutlinedButton(
                onClick = {
                    try {
                        context.startActivity(intent)
                    } catch (_: ActivityNotFoundException) {
                        launch = null
                    }
                },
                modifier = Modifier.fillMaxWidth().height(48.dp),
            ) { Text("Open Syncthing-Fork") }
            Spacer(Modifier.height(8.dp))
        }
        Button(
            onClick = onDone,
            colors = ButtonDefaults.buttonColors(containerColor = Accent, contentColor = Color.Black),
            modifier = Modifier.fillMaxWidth().height(52.dp),
        ) { Text(if (firstRun) "Done" else "Close", fontWeight = FontWeight.Bold) }
        if (firstRun) {
            TextButton(onClick = onDone, modifier = Modifier.fillMaxWidth()) { Text("Skip for now") }
        }
    }
}

@Composable
private fun SetupStep(number: Int, title: String, body: String, actions: @Composable () -> Unit = {}) {
    Row(Modifier.fillMaxWidth().padding(vertical = 10.dp)) {
        Box(Modifier.size(28.dp).clip(CircleShape).background(SurfaceHigh), contentAlignment = Alignment.Center) {
            Text("$number", fontWeight = FontWeight.Bold, style = MaterialTheme.typography.labelLarge)
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(title, fontWeight = FontWeight.SemiBold)
            Text(body, color = TextSecondary, style = MaterialTheme.typography.bodyMedium)
            actions()
        }
    }
}
