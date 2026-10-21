package io.github.brainage04.tunemesh

import android.content.ComponentName
import android.content.Context
import android.graphics.Bitmap
import android.net.Uri
import android.os.Handler
import android.os.Looper
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.brainage04.tunemesh.data.CoverProvider
import io.github.brainage04.tunemesh.data.LibraryScanner
import io.github.brainage04.tunemesh.library.Track
import io.github.brainage04.tunemesh.playback.PlaybackService
import io.github.brainage04.tunemesh.playback.QueueItems
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.io.FileNotFoundException
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.math.sin

/**
 * Exercises the real MediaMetadataRetriever scan, the real ExoPlayer media session and the cover provider on a
 * device. The app's library root points at a scratch directory in the app cache for the duration of each test.
 */
@RunWith(AndroidJUnit4::class)
class DeviceTest {
    private val context: Context = InstrumentationRegistry.getInstrumentation().targetContext
    private val app get() = context.applicationContext as TuneMeshApp
    private lateinit var root: File
    private lateinit var outside: File
    private lateinit var previousRoot: File

    @Before
    fun setUp() {
        root = File(context.cacheDir, "test-library").apply { deleteRecursively(); mkdirs() }
        outside = File(context.cacheDir, "test-outside").apply { deleteRecursively(); mkdirs() }
        previousRoot = app.settings.libraryRoot
        app.repository.setLibraryRoot(root.absolutePath)
    }

    @After
    fun tearDown() {
        app.repository.setLibraryRoot(previousRoot.absolutePath)
        root.deleteRecursively()
        outside.deleteRecursively()
    }

    @Test
    fun scannerReadsDurationAppliesFallbacksAndCachesTags() {
        writeWav(File(root, "Test Artist/Test Album/Tone [abc123].wav"), seconds = 3)
        writeWav(File(root, ".tunemesh/requests/ignored.wav"), seconds = 1)
        writeWav(File(root, "Playlists/ignored.wav"), seconds = 1)
        // Names with line breaks are not part of the library (an m3u8 line could not name them).
        writeWav(File(root, "Test Artist/Test Album/Two\nLines.wav"), seconds = 1)
        writeWav(File(root, "Test Artist/Test Album/Carriage\rReturn.wav"), seconds = 1)
        writeWav(File(root, "Broken\nArtist/Album/Inside.wav"), seconds = 1)
        val cache = File(context.cacheDir, "test-tag-cache.json").apply { delete() }

        val result = LibraryScanner(cache).scan(root)
        assertTrue(result.directories.none { '\n' in it.path })
        val first = result.tracks
        assertEquals(1, first.size)
        val track = first.single()
        assertEquals("Test Artist/Test Album/Tone [abc123].wav", track.path)
        assertEquals("Tone", track.title)
        assertEquals("Test Album", track.album)
        assertEquals("Test Artist", track.artist)
        assertTrue("duration ${track.durationMs}", track.durationMs in 2_900..3_100)
        assertTrue(cache.isFile)

        // A fresh scanner answers from the cache file without a scan.
        assertEquals(first, LibraryScanner(cache).cachedTracks(root))
        cache.delete()
    }

    /** "Limits": folders deeper than 64 levels are not scanned; tag fields are cut and artists limited. */
    @Test
    fun scannerFollowsTheFolderAndTagLimits() {
        val level64 = (1..64).joinToString("/") { "d%02d".format(it) }
        File(root, "$level64/ok.mp3").apply { parentFile!!.mkdirs() }.writeBytes(HostileLibrary.mp3("Deep", "A", "B", 1))
        File(root, "$level64/d65/too deep.mp3").apply { parentFile!!.mkdirs() }.writeBytes(HostileLibrary.mp3("Deeper", "A", "B", 1))
        // Android's ID3 reader returns only the first of NUL-separated values, so the limit is checked with `;`.
        val artists = (1..40).joinToString("; ") { "Artist $it" }
        File(root, "Huge/Tags/long tag.mp3").apply { parentFile!!.mkdirs() }
            .writeBytes(HostileLibrary.mp3("t".repeat(5000), artists, "a".repeat(5000), 1))
        // MediaMetadataRetriever itself drops a 1 MiB frame; either way nothing longer than the limit is kept.
        File(root, "Huge/Tags/big tag.mp3").writeBytes(HostileLibrary.mp3("t".repeat(1024 * 1024), "A", "a".repeat(1024 * 1024), 1))
        val cache = File(context.cacheDir, "test-tag-cache.json").apply { delete() }
        val tracks = LibraryScanner(cache).scan(root).tracks.associateBy { it.path }
        cache.delete()
        assertEquals(setOf("$level64/ok.mp3", "Huge/Tags/long tag.mp3", "Huge/Tags/big tag.mp3"), tracks.keys)
        val long = tracks.getValue("Huge/Tags/long tag.mp3")
        assertEquals(1024, long.title.length)
        assertEquals(1024, long.album.length)
        assertEquals(32, long.artist.split('\u0000', ';').size)
        val big = tracks.getValue("Huge/Tags/big tag.mp3")
        assertTrue(big.title.length <= 1024 && big.album.length <= 1024)
    }

    /** "Limits": a cover over 16 MiB or 5,000 pixels a side is not shown; a normal one is. */
    @Test
    fun coversFollowTheLimits() {
        val loader = io.github.brainage04.tunemesh.data.CoverLoader(File(context.cacheDir, "test-covers").apply { deleteRecursively() })
        fun track(path: String, bytes: ByteArray): Track {
            val file = File(root, path).apply { parentFile!!.mkdirs(); writeBytes(bytes) }
            return Track.fromTags(path, io.github.brainage04.tunemesh.library.RawTags(), file.length(), file.lastModified())
        }
        val small = track("S/A/small.mp3", HostileLibrary.mp3("s", "S", "A", 1, picture = HostileLibrary.smallJpeg()))
        assertNotNull(loader.diskFile(root, small))
        // Within 16 MiB but stated as 20,000 px a side.
        val wide = track("W/A/wide.mp3", HostileLibrary.mp3("w", "W", "A", 1, picture = HostileLibrary.hugeJpeg(64 * 1024)))
        assertNull(loader.diskFile(root, wide))
        // Over 16 MiB: the embedded picture and the folder cover.
        val heavy = track("H/A/heavy.mp3", HostileLibrary.mp3("h", "H", "A", 1, picture = HostileLibrary.hugeJpeg(20_000_000)))
        assertNull(loader.diskFile(root, heavy))
        File(root, "F/A/cover.jpg").apply { parentFile!!.mkdirs() }.writeBytes(HostileLibrary.hugeJpeg(17 * 1024 * 1024))
        assertNull(loader.diskFile(root, track("F/A/plain.mp3", HostileLibrary.mp3("p", "F", "A", 1))))
        File(root, "G/A/cover.jpg").apply { parentFile!!.mkdirs() }.writeBytes(HostileLibrary.smallJpeg())
        assertNotNull(loader.diskFile(root, track("G/A/plain.mp3", HostileLibrary.mp3("p", "G", "A", 1))))
        File(context.cacheDir, "test-covers").deleteRecursively()
    }

    /** R2-opus-5: quick playlist changes run one after another, so none is lost. */
    @Test
    fun quickPlaylistChangesAreAllKept() {
        val tracks = (1..12).map { Track.fromTags("Quick/Album/$it.mp3", io.github.brainage04.tunemesh.library.RawTags(), 1, 1) }
        tracks.forEach { app.repository.setLiked(it, true) }
        val store = io.github.brainage04.tunemesh.library.PlaylistStore(root) { null }
        val end = System.currentTimeMillis() + 20_000
        while (System.currentTimeMillis() < end && store.likedPaths().size < tracks.size) Thread.sleep(100)
        Thread.sleep(500)
        assertEquals(tracks.map { it.path }.toSet(), store.likedPaths())
        // Liking inserts at the top: the order of the taps is kept.
        assertEquals(tracks.reversed().map { it.path }, store.read(io.github.brainage04.tunemesh.library.LibraryFormat.LIKED_SONGS)!!.entries.map { it.path })
    }

    /** Audit finding 7: `path` must not reach audio files outside the library. */
    @Test
    fun coverProviderServesLibraryTracksOnly() {
        writeWav(File(root, "Artist/Album/Song.wav"), seconds = 1)
        writeCover(File(root, "Artist/Album/cover.jpg"))
        writeWav(File(outside, "Private/Secret.wav"), seconds = 1)
        writeCover(File(outside, "Private/cover.jpg"))

        context.contentResolver.openFileDescriptor(CoverProvider.uri(context, "Artist/Album/Song.wav"), "r").use {
            assertNotNull(it)
            assertTrue(it!!.statSize > 0)
        }
        for (path in listOf(
            "../test-outside/Private/Secret.wav",
            "Artist/../../test-outside/Private/Secret.wav",
            File(outside, "Private/Secret.wav").absolutePath,
            "Artist/Album/cover.jpg",
            "Artist/Album/Missing.wav",
        )) {
            try {
                context.contentResolver.openFileDescriptor(CoverProvider.uri(context, path), "r")?.close()
                fail("served a cover for $path")
            } catch (_: FileNotFoundException) {
            }
        }
    }

    /** Audit finding 8: controller-supplied metadata, artwork URIs in particular, is replaced by the library's. */
    @Test
    fun resolveRebuildsControllerItemsFromTheLibrary() {
        val path = "Artist/Album/Song [abc].wav"
        val file = File(root, path)
        writeWav(file, seconds = 1)
        val spoofed = MediaItem.Builder()
            .setMediaId("q1|Somewhere/Else.mp3")
            .setUri(Uri.fromFile(file))
            .setMediaMetadata(
                MediaMetadata.Builder()
                    .setTitle("Spoofed")
                    .setArtworkUri(Uri.parse("file:///sdcard/DCIM/Camera/private.jpg"))
                    .setArtworkData(byteArrayOf(1, 2, 3), MediaMetadata.PICTURE_TYPE_FRONT_COVER)
                    .build(),
            )
            .build()
        val resolved = QueueItems.resolve(context, spoofed, root, app.repository::track)!!
        assertEquals("q1|$path", resolved.mediaId)
        assertEquals(Uri.fromFile(File(root, path)), resolved.localConfiguration!!.uri)
        assertEquals("Song", resolved.mediaMetadata.title.toString())
        assertEquals(CoverProvider.uri(context, path), resolved.mediaMetadata.artworkUri)
        assertNull(resolved.mediaMetadata.artworkData)

        val outsideFile = File(outside, "Other.wav").also { writeWav(it, seconds = 1) }
        assertNull(QueueItems.resolve(context, MediaItem.fromUri(Uri.fromFile(outsideFile)), root, app.repository::track))
        assertNull(QueueItems.resolve(context, MediaItem.fromUri("https://tracker.example/a.mp3"), root, app.repository::track))
    }

    /** R1-opus-b-3: a controller cannot queue repeated entry IDs (they crashed the queue screen). */
    @Test
    fun controllerItemsGetUniqueQueueIds() {
        val path = "Artist/Album/Song [abc].wav"
        writeWav(File(root, path), seconds = 1)
        val dup = MediaItem.Builder().setMediaId("dup|x").setUri(Uri.fromFile(File(root, path))).build()
        val used = hashSetOf("taken")
        val ids = listOf(dup, dup, dup, dup.buildUpon().setMediaId("taken|x").build(), dup.buildUpon().setMediaId("fresh|x").build())
            .map { QueueItems.id(QueueItems.resolve(context, it, root, app.repository::track, used)!!) }
        assertEquals("dup", ids[0])
        assertEquals("fresh", ids[4])
        assertEquals(ids.size, ids.toSet().size)
        assertTrue("taken" !in ids)
        assertEquals(used, ids.toSet() + "taken")

        // Through the real session, as another app's controller would send them.
        val main = Handler(Looper.getMainLooper())
        val token = SessionToken(context, ComponentName(context, PlaybackService::class.java))
        val controller = MediaController.Builder(context, token).setApplicationLooper(Looper.getMainLooper()).buildAsync().get(10, TimeUnit.SECONDS)
        try {
            val done = CountDownLatch(1)
            var queued = emptyList<String>()
            main.post {
                controller.setMediaItems(listOf(dup, dup, dup))
                controller.addMediaItems(listOf(dup))
                main.postDelayed({
                    queued = (0 until controller.mediaItemCount).map { controller.getMediaItemAt(it).mediaId }
                    done.countDown()
                }, 1_000)
            }
            assertTrue(done.await(10, TimeUnit.SECONDS))
            assertEquals(4, queued.size)
            assertEquals(queued.toString(), 4, queued.toSet().size)
            assertTrue(queued.all { it.endsWith("|$path") })
        } finally {
            main.post {
                controller.stop()
                controller.clearMediaItems()
                controller.release()
            }
        }
    }

    @Test
    fun savedQueuesWithRepeatedIdsAreMadeUnique() {
        val item = io.github.brainage04.tunemesh.playback.StoredItem("same", "a.wav", "file:///a.wav", "A", "B", "C")
        val ids = QueueItems.withUniqueIds(listOf(item, item, item.copy(id = "other"), item)).map { it.id }
        assertEquals("same", ids[0])
        assertEquals("other", ids[2])
        assertEquals(4, ids.toSet().size)
    }

    @Test
    fun playbackServicePlaysAQueuedFileWithLibraryMetadata() {
        val path = "Test Artist/Test Album/Long Tone [def456].wav"
        val file = File(root, path)
        writeWav(file, seconds = 8)
        val track = Track(path, "Long Tone", "Test Artist", "Test Album", "Test Artist", 1, null, 8_000, file.length(), file.lastModified())
        val sent = QueueItems.build(context, root, track).let { item ->
            item.buildUpon()
                .setMediaMetadata(item.mediaMetadata.buildUpon().setArtworkUri(Uri.parse("https://tracker.example/pixel.png")).build())
                .build()
        }
        val main = Handler(Looper.getMainLooper())
        val token = SessionToken(context, ComponentName(context, PlaybackService::class.java))
        val future = MediaController.Builder(context, token).setApplicationLooper(Looper.getMainLooper()).buildAsync()
        val controller = future.get(10, TimeUnit.SECONDS)
        try {
            val playing = CountDownLatch(1)
            main.post {
                controller.addListener(object : Player.Listener {
                    override fun onIsPlayingChanged(isPlaying: Boolean) {
                        if (isPlaying) playing.countDown()
                    }
                })
                controller.setMediaItem(sent)
                controller.prepare()
                controller.play()
            }
            assertTrue("player never started", playing.await(10, TimeUnit.SECONDS))
            Thread.sleep(1_500)
            val position = CountDownLatch(1)
            var positionMs = 0L
            var mediaId = ""
            var artwork: Uri? = null
            main.post {
                positionMs = controller.currentPosition
                mediaId = controller.currentMediaItem?.mediaId.orEmpty()
                artwork = controller.currentMediaItem?.mediaMetadata?.artworkUri
                position.countDown()
            }
            position.await(5, TimeUnit.SECONDS)
            assertTrue("position $positionMs", positionMs >= 800)
            assertTrue(mediaId.endsWith("|$path"))
            assertEquals(CoverProvider.uri(context, path), artwork)
        } finally {
            main.post {
                controller.stop()
                controller.clearMediaItems()
                controller.release()
            }
        }
    }

    private fun writeCover(file: File) {
        file.parentFile!!.mkdirs()
        val bitmap = Bitmap.createBitmap(64, 64, Bitmap.Config.ARGB_8888).apply { eraseColor(0xFF1ED760.toInt()) }
        file.outputStream().use { bitmap.compress(Bitmap.CompressFormat.JPEG, 90, it) }
    }

    private fun writeWav(file: File, seconds: Int) {
        file.parentFile!!.mkdirs()
        val rate = 44_100
        val samples = rate * seconds
        val buffer = ByteBuffer.allocate(44 + samples * 2).order(ByteOrder.LITTLE_ENDIAN)
        buffer.put("RIFF".toByteArray()).putInt(36 + samples * 2).put("WAVE".toByteArray())
        buffer.put("fmt ".toByteArray()).putInt(16).putShort(1).putShort(1).putInt(rate).putInt(rate * 2).putShort(2).putShort(16)
        buffer.put("data".toByteArray()).putInt(samples * 2)
        for (i in 0 until samples) buffer.putShort((sin(2 * Math.PI * 440 * i / rate) * 8000).toInt().toShort())
        file.writeBytes(buffer.array())
    }
}
