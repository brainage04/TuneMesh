package io.github.brainage04.tunemesh.ui.screens

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.compose.foundation.layout.Arrangement
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import io.github.brainage04.tunemesh.data.AppSettings
import io.github.brainage04.tunemesh.ui.AppIcons
import io.github.brainage04.tunemesh.ui.BackBar
import io.github.brainage04.tunemesh.ui.LocalAppViewModel
import io.github.brainage04.tunemesh.ui.LocalLibrary
import io.github.brainage04.tunemesh.ui.theme.Accent
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
