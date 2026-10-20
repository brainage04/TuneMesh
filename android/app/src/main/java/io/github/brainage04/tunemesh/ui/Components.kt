package io.github.brainage04.tunemesh.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.brainage04.tunemesh.data.LibraryState
import io.github.brainage04.tunemesh.library.EntryRef
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.Playlist
import io.github.brainage04.tunemesh.playback.PlayerUiState
import io.github.brainage04.tunemesh.library.Track
import io.github.brainage04.tunemesh.ui.theme.Accent
import io.github.brainage04.tunemesh.ui.theme.TextSecondary
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

val LocalAppViewModel = staticCompositionLocalOf<AppViewModel> { error("AppViewModel not provided") }
val LocalLibrary = compositionLocalOf<LibraryState> { error("LibraryState not provided") }
val LocalPlayerUi = compositionLocalOf { PlayerUiState() }

private val coverDispatcher = Dispatchers.IO.limitedParallelism(3)

/** Deterministic dark gradient used when a track, album or playlist has no art. */
fun placeholderBrush(seed: String): Brush {
    val hue = ((seed.hashCode() and 0x7fffffff) % 360).toFloat()
    return Brush.linearGradient(
        listOf(Color.hsv(hue, 0.55f, 0.55f), Color.hsv((hue + 40f) % 360f, 0.65f, 0.25f)),
    )
}

/** Album art with memory/disk caching and a gradient placeholder. */
@Composable
fun CoverArt(
    track: Track?,
    seed: String,
    modifier: Modifier = Modifier,
    shape: Shape = RoundedCornerShape(4.dp),
    sizeHint: Dp = 48.dp,
    placeholderIcon: ImageVector = AppIcons.MusicNote,
) {
    val viewModel = LocalAppViewModel.current
    val sizePx = when {
        sizeHint <= 64.dp -> 160
        sizeHint <= 200.dp -> 400
        else -> 600
    }
    val bitmap by produceState<ImageBitmap?>(
        initialValue = track?.let { viewModel.covers.cached(it, sizePx)?.asImageBitmap() },
        track?.path,
        track?.modifiedMs,
        sizePx,
    ) {
        // Keys changed (or first run): never keep showing the previous track's art.
        value = track?.let { viewModel.covers.cached(it, sizePx)?.asImageBitmap() }
        if (value == null && track != null) {
            value = withContext(coverDispatcher) {
                viewModel.covers.load(viewModel.repository.root, track, sizePx)?.asImageBitmap()
            }
        }
    }
    Box(modifier.clip(shape).background(placeholderBrush(seed)), contentAlignment = Alignment.Center) {
        val image = bitmap
        if (image != null) {
            Image(image, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        } else {
            BoxWithConstraints(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Icon(placeholderIcon, null, tint = Color.White.copy(alpha = 0.7f), modifier = Modifier.size(maxWidth * 0.42f))
            }
        }
    }
}

/** Liked Songs tile: purple-blue gradient with a heart, as on Spotify. */
@Composable
fun LikedSongsArt(modifier: Modifier = Modifier, shape: Shape = RoundedCornerShape(4.dp)) {
    BoxWithConstraints(
        modifier.clip(shape).background(Brush.linearGradient(listOf(Color(0xFF4A1FD6), Color(0xFF8BB4E8)))),
        contentAlignment = Alignment.Center,
    ) {
        Icon(AppIcons.Favorite, null, tint = Color.White, modifier = Modifier.size(maxWidth * 0.42f))
    }
}

@Composable
fun SectionTitle(text: String, modifier: Modifier = Modifier) {
    Text(
        text,
        style = MaterialTheme.typography.titleLarge,
        fontWeight = FontWeight.Bold,
        modifier = modifier.padding(start = 16.dp, end = 16.dp, top = 24.dp, bottom = 12.dp),
    )
}

@Composable
fun ScreenTitle(text: String, actions: @Composable () -> Unit = {}) {
    Row(
        Modifier.fillMaxWidth().statusBarsPadding().padding(start = 16.dp, end = 4.dp, top = 12.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(text, style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
        actions()
    }
}

@Composable
fun BackBar(title: String = "", onBack: () -> Unit, actions: @Composable () -> Unit = {}) {
    Row(
        Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 4.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClick = onBack) { Icon(AppIcons.Back, "Back") }
        Text(title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
        actions()
    }
}

@Composable
fun LikeButton(liked: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier, size: Dp = 22.dp) {
    IconButton(onClick = onClick, modifier = modifier) {
        Icon(
            if (liked) AppIcons.Favorite else AppIcons.FavoriteBorder,
            contentDescription = if (liked) "Remove from Liked Songs" else "Save to Liked Songs",
            tint = if (liked) Accent else TextSecondary,
            modifier = Modifier.size(size),
        )
    }
}

/** Where a track row is shown, so its menu can offer context-specific entries. */
data class RowContext(
    val playlistName: String? = null,
    val index: Int = -1,
    val showAlbum: Boolean = true,
    val showArtist: Boolean = true,
)

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun TrackRow(
    track: Track,
    onPlay: () -> Unit,
    context: RowContext = RowContext(),
    leading: (@Composable () -> Unit)? = null,
) {
    val viewModel = LocalAppViewModel.current
    val library = LocalLibrary.current
    val playing = LocalPlayerUi.current
    val isCurrent = playing.currentPath == track.path
    var menuOpen by remember { mutableStateOf(false) }
    Row(
        Modifier
            .fillMaxWidth()
            .combinedClickable(onClick = onPlay, onLongClick = { menuOpen = true })
            .padding(start = 16.dp, end = 0.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (leading != null) leading() else CoverArt(track, track.album, Modifier.size(48.dp))
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(
                track.title,
                color = if (isCurrent) Accent else Color.White,
                style = MaterialTheme.typography.bodyLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                if (context.showAlbum && context.showArtist) "${track.artist} • ${track.album}" else track.artist,
                color = TextSecondary,
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        LikeButton(track.path in library.liked, { viewModel.toggleLike(track) })
        Box {
            IconButton(onClick = { menuOpen = true }) { Icon(AppIcons.More, "More options", tint = TextSecondary) }
            TrackMenu(track, context, menuOpen) { menuOpen = false }
        }
    }
}

@Composable
fun TrackMenu(track: Track, context: RowContext, expanded: Boolean, onDismiss: () -> Unit) {
    val viewModel = LocalAppViewModel.current
    val library = LocalLibrary.current
    val liked = track.path in library.liked
    DropdownMenu(expanded = expanded, onDismissRequest = onDismiss) {
        MenuItem("Play next", AppIcons.Queue) { viewModel.playNext(listOf(track)); onDismiss() }
        MenuItem("Add to queue", AppIcons.Queue) { viewModel.addToQueue(listOf(track)); onDismiss() }
        MenuItem(if (liked) "Remove from Liked Songs" else "Save to Liked Songs", if (liked) AppIcons.Favorite else AppIcons.FavoriteBorder) {
            viewModel.toggleLike(track); onDismiss()
        }
        MenuItem("Add to playlist", AppIcons.PlaylistAdd) { viewModel.dialog = DialogRequest.AddToPlaylist(listOf(track)); onDismiss() }
        if (context.playlistName != null && context.playlistName != LibraryFormat.LIKED_SONGS) {
            MenuItem("Remove from this playlist", AppIcons.Close) {
                viewModel.repository.removeFromPlaylist(context.playlistName, EntryRef(context.index, track.path)); onDismiss()
            }
        }
        if (context.showAlbum) MenuItem("Go to album", AppIcons.Album) { viewModel.openAlbum(track); onDismiss() }
        if (context.showArtist) MenuItem("Go to artist", AppIcons.Person) { viewModel.openArtist(track.albumArtist); onDismiss() }
        MenuItem("Delete from library", AppIcons.Delete, color = MaterialTheme.colorScheme.error) {
            viewModel.dialog = DialogRequest.DeleteTrack(track); onDismiss()
        }
    }
}

@Composable
fun MenuItem(text: String, icon: ImageVector, color: Color = Color.White, onClick: () -> Unit) {
    DropdownMenuItem(
        text = { Text(text, color = color) },
        leadingIcon = { Icon(icon, null, tint = if (color == Color.White) TextSecondary else color) },
        onClick = onClick,
    )
}

/** Playlist entry whose file is not (yet) in the library. */
@Composable
fun UnavailableRow(raw: String, onRemove: (() -> Unit)?) {
    Row(
        Modifier.fillMaxWidth().padding(start = 16.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(48.dp).clip(RoundedCornerShape(4.dp)).background(Color(0xFF2A2A2A)), contentAlignment = Alignment.Center) {
            Icon(AppIcons.MusicNote, null, tint = Color(0xFF5A5A5A))
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(LibraryFormat.titleFromFileName(raw.substringAfterLast('/')), color = Color(0xFF6A6A6A), maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text("Unavailable on this device", color = Color(0xFF6A6A6A), style = MaterialTheme.typography.bodyMedium)
        }
        if (onRemove != null) IconButton(onClick = onRemove) { Icon(AppIcons.Close, "Remove", tint = TextSecondary) }
    }
}

/** Big square tile for albums and playlists in horizontal rows and grids. */
@Composable
fun MediaTile(
    title: String,
    subtitle: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    circle: Boolean = false,
    art: @Composable (Modifier) -> Unit,
) {
    Column(modifier.clip(RoundedCornerShape(6.dp)).clickable(onClick = onClick).padding(6.dp)) {
        art(Modifier.fillMaxWidth().aspectSquare())
        Spacer(Modifier.height(8.dp))
        Text(
            title,
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.SemiBold,
            maxLines = if (circle) 1 else 2,
            overflow = TextOverflow.Ellipsis,
            modifier = if (circle) Modifier.align(Alignment.CenterHorizontally) else Modifier,
        )
        Text(
            subtitle,
            style = MaterialTheme.typography.bodySmall,
            color = TextSecondary,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = if (circle) Modifier.align(Alignment.CenterHorizontally) else Modifier,
        )
    }
}

fun Modifier.aspectSquare(): Modifier = this.aspectRatio(1f)

/** Header of album, artist and playlist pages: art, title, stats, shuffle and play buttons. */
@Composable
fun DetailHeader(
    title: String,
    subtitle: String,
    stats: String,
    seed: String,
    onBack: () -> Unit,
    onPlay: () -> Unit,
    onShuffle: () -> Unit,
    playEnabled: Boolean,
    art: @Composable (Modifier) -> Unit,
    playingThis: Boolean = false,
    onSubtitleClick: (() -> Unit)? = null,
    actions: @Composable () -> Unit = {},
) {
    val hue = ((seed.hashCode() and 0x7fffffff) % 360).toFloat()
    Column(
        Modifier.fillMaxWidth().background(
            Brush.verticalGradient(listOf(Color.hsv(hue, 0.45f, 0.42f), MaterialTheme.colorScheme.background)),
        ),
    ) {
        BackBar(onBack = onBack, actions = actions)
        art(Modifier.align(Alignment.CenterHorizontally).padding(top = 4.dp).size(232.dp))
        Spacer(Modifier.height(20.dp))
        Text(
            title,
            style = MaterialTheme.typography.headlineMedium,
            fontWeight = FontWeight.Bold,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(horizontal = 16.dp),
        )
        if (subtitle.isNotEmpty()) {
            Text(
                subtitle,
                style = MaterialTheme.typography.bodyLarge,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier
                    .padding(horizontal = 16.dp, vertical = 6.dp)
                    .then(if (onSubtitleClick != null) Modifier.clickable(onClick = onSubtitleClick) else Modifier),
            )
        }
        Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(stats, color = TextSecondary, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
            IconButton(onClick = onShuffle, enabled = playEnabled) {
                Icon(AppIcons.Shuffle, "Shuffle play", tint = if (playEnabled) TextSecondary else Color.DarkGray, modifier = Modifier.size(28.dp))
            }
            Spacer(Modifier.width(8.dp))
            PlayCircle(playing = playingThis, onClick = onPlay, enabled = playEnabled)
        }
    }
}

@Composable
fun PlayCircle(playing: Boolean, onClick: () -> Unit, enabled: Boolean = true, size: Dp = 56.dp, color: Color = Accent) {
    FilledIconButton(
        onClick = onClick,
        enabled = enabled,
        modifier = Modifier.size(size),
        shape = CircleShape,
        colors = IconButtonDefaults.filledIconButtonColors(containerColor = color, contentColor = Color.Black),
    ) {
        Icon(if (playing) AppIcons.Pause else AppIcons.Play, if (playing) "Pause" else "Play", modifier = Modifier.size(size * 0.5f))
    }
}

fun formatDuration(ms: Long): String {
    val totalSeconds = (ms / 1000).coerceAtLeast(0)
    val hours = totalSeconds / 3600
    val minutes = (totalSeconds % 3600) / 60
    val seconds = totalSeconds % 60
    return if (hours > 0) "%d:%02d:%02d".format(hours, minutes, seconds) else "%d:%02d".format(minutes, seconds)
}

fun totalLength(tracks: List<Track>): String {
    val minutes = tracks.sumOf { it.durationMs } / 60_000
    return if (minutes >= 60) "${minutes / 60} h ${minutes % 60} min" else "$minutes min"
}

fun songCount(count: Int) = if (count == 1) "1 song" else "$count songs"

/** "Playlist • 12 songs", or why it cannot be opened. */
fun playlistSubtitle(playlist: Playlist) =
    if (playlist.problem != null) "Playlist • too large to open" else "Playlist • ${songCount(playlist.size)}"

/** Drops text-field focus when the keyboard is dismissed, so no cursor keeps blinking (and redrawing). */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ClearFocusWhenImeHidden() {
    val focusManager = LocalFocusManager.current
    val imeVisible = WindowInsets.isImeVisible
    LaunchedEffect(imeVisible) {
        if (!imeVisible) focusManager.clearFocus()
    }
}

@Composable
fun EmptyMessage(title: String, body: String, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxWidth().padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
        Text(title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
        Spacer(Modifier.height(8.dp))
        Text(body, color = TextSecondary, style = MaterialTheme.typography.bodyMedium, fontSize = 14.sp)
    }
}
