package io.github.brainage04.tunemesh.playback

import android.content.Context
import android.net.Uri
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import io.github.brainage04.tunemesh.data.CoverProvider
import io.github.brainage04.tunemesh.library.LibraryUris
import io.github.brainage04.tunemesh.library.Track
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.io.File
import java.util.concurrent.atomic.AtomicLong

/** Queue entries carry `"<unique id>|<library path>"` as media ID so duplicates stay distinguishable. */
object QueueItems {
    private val counter = AtomicLong(System.currentTimeMillis())

    fun newId(): String = java.lang.Long.toString(counter.incrementAndGet(), 36)

    fun path(item: MediaItem): String = item.mediaId.substringAfter('|')

    fun id(item: MediaItem): String = item.mediaId.substringBefore('|')

    fun build(context: Context, root: File, track: Track, id: String = newId()): MediaItem =
        build(
            StoredItem(
                id = id,
                path = track.path,
                uri = Uri.fromFile(File(root, track.path)).toString(),
                title = track.title,
                artist = track.artist,
                album = track.album,
                albumArtist = track.albumArtist,
                durationMs = track.durationMs,
                artwork = CoverProvider.uri(context, track.path).toString(),
            ),
        )

    fun build(stored: StoredItem): MediaItem {
        val uri = Uri.parse(stored.uri)
        return MediaItem.Builder()
            .setMediaId("${stored.id}|${stored.path}")
            .setUri(uri)
            .setRequestMetadata(MediaItem.RequestMetadata.Builder().setMediaUri(uri).build())
            .setMediaMetadata(
                MediaMetadata.Builder()
                    .setTitle(stored.title)
                    .setArtist(stored.artist)
                    .setAlbumTitle(stored.album)
                    .setAlbumArtist(stored.albumArtist)
                    .setDurationMs(stored.durationMs.takeIf { it > 0 })
                    .setArtworkUri(stored.artwork?.let(Uri::parse))
                    .setIsPlayable(true)
                    .setIsBrowsable(false)
                    .setMediaType(MediaMetadata.MEDIA_TYPE_MUSIC)
                    .build(),
            )
            .build()
    }

    fun store(item: MediaItem): StoredItem {
        val metadata = item.mediaMetadata
        val uri = item.localConfiguration?.uri ?: item.requestMetadata.mediaUri
        return StoredItem(
            id = id(item),
            path = path(item),
            uri = uri?.toString().orEmpty(),
            title = metadata.title?.toString().orEmpty(),
            artist = metadata.artist?.toString().orEmpty(),
            album = metadata.albumTitle?.toString().orEmpty(),
            albumArtist = metadata.albumArtist?.toString().orEmpty(),
            durationMs = metadata.durationMs ?: 0,
            artwork = metadata.artworkUri?.toString(),
        )
    }

    /**
     * Fills in the playable URI for items that crossed the session boundary without one, or returns null when the
     * item does not name an audio file inside [root]: the session is exported, so other apps' controllers must not
     * make the player open arbitrary files or URLs.
     */
    fun resolve(item: MediaItem, root: File): MediaItem? {
        val uri = item.localConfiguration?.uri ?: item.requestMetadata.mediaUri ?: return null
        if (!LibraryUris.isLibraryFile(uri.toString(), root)) return null
        return if (item.localConfiguration != null) item else item.buildUpon().setUri(uri).build()
    }
}

@Serializable
data class StoredItem(
    val id: String,
    val path: String,
    val uri: String,
    val title: String,
    val artist: String,
    val album: String,
    val albumArtist: String = "",
    val durationMs: Long = 0,
    val artwork: String? = null,
)

@Serializable
data class StoredQueue(
    val items: List<StoredItem> = emptyList(),
    val index: Int = 0,
    val positionMs: Long = 0,
    val repeatMode: Int = 0,
)

/** Per-device playback state in app storage: queue, position, repeat (service) and shuffle (controller). */
class PlaybackState(context: Context) {
    private val prefs = context.getSharedPreferences("playback", Context.MODE_PRIVATE)
    private val json = Json { ignoreUnknownKeys = true }

    fun loadQueue(): StoredQueue? = prefs.getString(KEY_QUEUE, null)?.let {
        runCatching { json.decodeFromString<StoredQueue>(it) }.getOrNull()
    }

    fun saveQueue(queue: StoredQueue) {
        prefs.edit().putString(KEY_QUEUE, json.encodeToString(StoredQueue.serializer(), queue)).apply()
    }

    var shuffle: Boolean
        get() = prefs.getBoolean(KEY_SHUFFLE, false)
        set(value) = prefs.edit().putBoolean(KEY_SHUFFLE, value).apply()

    /** Queue entry IDs in context order, used to undo shuffling. */
    var originalOrder: List<String>
        get() = prefs.getString(KEY_ORIGINAL, null)?.let { runCatching { json.decodeFromString<List<String>>(it) }.getOrNull() }
            ?: emptyList()
        set(value) = prefs.edit().putString(KEY_ORIGINAL, json.encodeToString(value)).apply()

    companion object {
        private const val KEY_QUEUE = "queue"
        private const val KEY_SHUFFLE = "shuffle"
        private const val KEY_ORIGINAL = "originalOrder"
    }
}
