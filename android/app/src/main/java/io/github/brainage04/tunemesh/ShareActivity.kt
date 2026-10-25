package io.github.brainage04.tunemesh

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.brainage04.tunemesh.library.DownloadInputs
import io.github.brainage04.tunemesh.library.Downloader
import io.github.brainage04.tunemesh.ui.AppIcons
import io.github.brainage04.tunemesh.ui.screens.DownloaderBanner
import io.github.brainage04.tunemesh.ui.screens.RequestForm
import io.github.brainage04.tunemesh.ui.screens.effectiveSaveAsPlaylists
import io.github.brainage04.tunemesh.ui.theme.Accent
import io.github.brainage04.tunemesh.ui.theme.AppTheme
import io.github.brainage04.tunemesh.ui.theme.Surface
import io.github.brainage04.tunemesh.ui.theme.TextSecondary
import kotlinx.coroutines.delay

/** Share target for links from YouTube, YouTube Music, Spotify and browsers: queues a download request. */
class ShareActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val shared = sharedInput(intent)
        setContent {
            AppTheme {
                ShareSheet(shared, onClose = ::finish)
            }
        }
    }

    override fun onStart() {
        super.onStart()
        (application as TuneMeshApp).repository.refreshAsync()
    }

    companion object {
        /**
         * The first URL in the shared text, or the whole text as a search ([DownloadInputs.sharedInput]). Read as a CharSequence: apps may
         * share styled text (a Spanned), for which getStringExtra returns null.
         */
        fun sharedInput(intent: Intent?): String =
            DownloadInputs.sharedInput(intent?.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString().orEmpty())
    }
}

@Composable
private fun ShareSheet(input: String, onClose: () -> Unit) {
    val app = LocalContext.current.applicationContext as TuneMeshApp
    val library by app.repository.state.collectAsStateWithLifecycle()
    var playlist by remember { mutableStateOf("") }
    var saveAsPlaylists by remember { mutableStateOf<Boolean?>(null) }
    var submitting by remember { mutableStateOf(false) }
    var queued by remember { mutableStateOf(false) }
    val context = LocalContext.current
    LaunchedEffect(queued) {
        if (queued) {
            delay(if (library.downloader is Downloader.Active) 1500 else 3000)
            onClose()
        }
    }
    Box(
        Modifier.fillMaxSize().clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = onClose),
        contentAlignment = Alignment.BottomCenter,
    ) {
        Column(
            Modifier.fillMaxWidth()
                .clip(RoundedCornerShape(topStart = 16.dp, topEnd = 16.dp))
                .background(Surface)
                .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null) {}
                .navigationBarsPadding()
                .imePadding()
                .padding(vertical = 20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(AppIcons.Download, null, tint = Accent)
                Spacer(Modifier.width(10.dp))
                Text("Download to TuneMesh", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
            }
            Text(
                input.ifEmpty { "Nothing was shared" },
                color = TextSecondary,
                maxLines = 3,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(horizontal = 16.dp),
            )
            when {
                queued ->
                    Row(Modifier.padding(horizontal = 16.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Icon(AppIcons.Check, null, tint = Accent, modifier = Modifier.size(28.dp))
                        Spacer(Modifier.width(10.dp))
                        Text(
                            when (val downloader = library.downloader) {
                                is Downloader.Active -> "Queued. It appears in your library once ${downloader.device} has downloaded it."
                                Downloader.None -> "Queued. No device is downloading right now; it is downloaded once one picks it up."
                            }
                        )
                    }
                !library.hasAccess ->
                    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("TuneMesh needs All files access to write the request into your library folder.")
                        Button(
                            onClick = {
                                context.startActivity(
                                    Intent(
                                        Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                                        Uri.parse("package:${context.packageName}"),
                                    )
                                )
                            },
                            colors = ButtonDefaults.buttonColors(containerColor = Accent, contentColor = Color.Black),
                            modifier = Modifier.fillMaxWidth().height(48.dp),
                        ) {
                            Text("Open settings")
                        }
                    }
                else ->
                    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        DownloaderBanner(library.downloader, Modifier.padding(horizontal = 16.dp))
                        RequestForm(
                            hasAccess = library.hasAccess,
                            playlists = library.playlists,
                            input = input,
                            onInput = {},
                            playlist = playlist,
                            onPlaylist = { playlist = it },
                            saveAsPlaylists = saveAsPlaylists,
                            onSaveAsPlaylists = { saveAsPlaylists = it },
                            submitting = submitting,
                            onSubmit = {
                                submitting = true
                                app.repository.submitDownloads(
                                    input,
                                    playlist.ifBlank { null },
                                    effectiveSaveAsPlaylists(input, saveAsPlaylists),
                                ) { result ->
                                    submitting = false
                                    queued = result.isSuccess
                                }
                            },
                            showInput = false,
                        )
                    }
            }
        }
    }
}
