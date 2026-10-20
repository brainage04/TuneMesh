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
     * Rebuilds an item that crossed the session boundary from the library [track] it names, or returns null when
     * it does not name an audio file inside [root]. The session is exported, so other apps' controllers must not
     * make the player open arbitrary files or URLs, nor make the app load their metadata (artwork URIs in
     * particular) with its All files access: only the media URI and the queue entry ID are taken from [item].
     *
     * Queue entry IDs must stay unique (the queue screen keys rows by them, undoing shuffle maps them): the
     * controller's ID is kept only when it is not in [used] (the IDs already queued and those given out earlier
     * in the same batch), else the entry gets a new one. The ID kept or given is added to [used].
     */
    fun resolve(
        context: Context,
        item: MediaItem,
        root: File,
        track: (String) -> Track?,
        used: MutableSet<String> = HashSet(),
    ): MediaItem? {
        val uri = item.localConfiguration?.uri ?: item.requestMetadata.mediaUri ?: return null
        val path = LibraryUris.libraryPath(uri.toString(), root) ?: return null
        val resolved = track(path) ?: return null
        val id = item.mediaId.takeIf { '|' in it }?.substringBefore('|')?.takeIf { it.isNotEmpty() && used.add(it) }
            ?: newId().also { used += it }
        return build(context, root, resolved, id)
    }

    /** [items] with a new ID for every entry whose ID an earlier entry already has (queues saved before IDs were checked). */
    fun withUniqueIds(items: List<StoredItem>): List<StoredItem> {
        val seen = HashSet<String>()
        return items.map { item -> if (seen.add(item.id)) item else item.copy(id = newId()).also { seen += it.id } }
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
