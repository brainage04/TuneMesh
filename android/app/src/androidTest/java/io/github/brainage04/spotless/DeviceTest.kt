package io.github.brainage04.spotless

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
import io.github.brainage04.spotless.data.CoverProvider
import io.github.brainage04.spotless.data.LibraryScanner
import io.github.brainage04.spotless.data.LibraryState
import io.github.brainage04.spotless.library.LibraryIndex
import io.github.brainage04.spotless.library.Track
import io.github.brainage04.spotless.playback.PlaybackService
import io.github.brainage04.spotless.playback.QueueItems
import java.io.File
import java.io.FileNotFoundException
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.file.Files
import java.nio.file.attribute.BasicFileAttributes
import java.util.Locale
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.math.sin
import leakcanary.DetectLeaksAfterTestSuccess
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Exercises the real MediaMetadataRetriever scan, the real ExoPlayer media session and the cover provider on a device. The app's library
 * root points at a scratch directory in the app cache for the duration of each test.
 */
@RunWith(AndroidJUnit4::class)
class DeviceTest {
    @get:Rule val leaks = DetectLeaksAfterTestSuccess()

    private val context: Context = InstrumentationRegistry.getInstrumentation().targetContext
    private val app
        get() = context.applicationContext as SpotlessApp

    private lateinit var root: File
    private lateinit var outside: File
    private lateinit var previousRoot: File

    @Before
    fun setUp() {
        root =
            File(context.cacheDir, "test-library").apply {
                deleteRecursively()
                mkdirs()
            }
        outside =
            File(context.cacheDir, "test-outside").apply {
                deleteRecursively()
                mkdirs()
            }
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
        writeWav(File(root, ".spotless/requests/ignored.wav"), seconds = 1)
        writeWav(File(root, "Playlists/ignored.wav"), seconds = 1)
        // Names with line breaks are not part of the library (an m3u8 line could not name them).
        writeWav(File(root, "Test Artist/Test Album/Two\nLines.wav"), seconds = 1)
        writeWav(File(root, "Test Artist/Test Album/Carriage\rReturn.wav"), seconds = 1)
        writeWav(File(root, "Broken\nArtist/Album/Inside.wav"), seconds = 1)
        val cacheDirectory = File(context.cacheDir, "test-tag-cache").apply { deleteRecursively() }
        val cache = File(cacheDirectory, LibraryScanner.CACHE_FILE)

        val result = LibraryScanner(cacheDirectory).scan(root, LibraryIndex.EMPTY)
        assertTrue(result.directories.files.none { '\n' in it.path })
        val first = result.index.tracks
        assertEquals(1, first.size)
        val track = first.single()
        assertEquals("Test Artist/Test Album/Tone [abc123].wav", track.path)
        assertEquals("Tone", track.title)
        assertEquals("Test Album", track.album)
        assertEquals("Test Artist", track.artist)
        assertTrue("duration ${track.durationMs}", track.durationMs in 2_900..3_100)
        assertTrue(cache.isFile)

        // A fresh scanner answers from the cache file without a scan, and a scan of the unchanged library keeps that index.
        val cached = LibraryScanner(cacheDirectory).cached(root)!!
        assertEquals(first, cached.tracks)
        assertSame(cached, LibraryScanner(cacheDirectory).scan(root, cached).index)
        // A scan that starts before the saved index is shown (a refresh racing the start) keeps the saved one too: no tags are read again,
        // so the cache file is not rewritten (a rewrite replaces it with a new file).
        val saved = Files.readAttributes(cache.toPath(), BasicFileAttributes::class.java).fileKey()
        assertEquals(first, LibraryScanner(cacheDirectory).scan(root, LibraryIndex.EMPTY).index.tracks)
        assertEquals(saved, Files.readAttributes(cache.toPath(), BasicFileAttributes::class.java).fileKey())
        cacheDirectory.deleteRecursively()
    }

    /** "Limits": folders deeper than 64 levels are not scanned; tag fields are cut and artists limited. */
    @Test
    fun scannerFollowsTheFolderAndTagLimits() {
        val level64 = (1..64).joinToString("/") { "d%02d".format(Locale.ROOT, it) }
        File(root, "$level64/ok.mp3").apply { parentFile!!.mkdirs() }.writeBytes(HostileLibrary.mp3("Deep", "A", "B", 1))
        File(root, "$level64/d65/too deep.mp3").apply { parentFile!!.mkdirs() }.writeBytes(HostileLibrary.mp3("Deeper", "A", "B", 1))
        // Android's ID3 reader returns only the first of NUL-separated values, so the limit is checked with `;`.
        val artists = (1..40).joinToString("; ") { "Artist $it" }
        File(root, "Huge/Tags/long tag.mp3")
            .apply { parentFile!!.mkdirs() }
            .writeBytes(HostileLibrary.mp3("t".repeat(5000), artists, "a".repeat(5000), 1))
        // MediaMetadataRetriever itself drops a 1 MiB frame; either way nothing longer than the limit is kept.
        File(root, "Huge/Tags/big tag.mp3").writeBytes(HostileLibrary.mp3("t".repeat(1024 * 1024), "A", "a".repeat(1024 * 1024), 1))
        val cacheDirectory = File(context.cacheDir, "test-tag-cache").apply { deleteRecursively() }
        val tracks = LibraryScanner(cacheDirectory).scan(root, LibraryIndex.EMPTY).index.tracks.associateBy { it.path }
        cacheDirectory.deleteRecursively()
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
        val loader = io.github.brainage04.spotless.data.CoverLoader(File(context.cacheDir, "test-covers").apply { deleteRecursively() })
        fun track(path: String, bytes: ByteArray): Track {
            val file =
                File(root, path).apply {
                    parentFile!!.mkdirs()
                    writeBytes(bytes)
                }
            return Track.fromTags(path, io.github.brainage04.spotless.library.RawTags(), file.length(), file.lastModified())
        }
        val small = track("S/A/small.mp3", HostileLibrary.mp3("s", "S", "A", 1, picture = HostileLibrary.smallJpeg()))
        assertNotNull(loader.diskFile(root, small))
        // A loaded cover is in memory for its track and size; a changed file (another modification time) or another size is not.
        val bitmap = loader.load(root, small, 160)
        assertNotNull(bitmap)
        assertSame(bitmap, loader.cached(small, 160))
        assertNull(loader.cached(small, 400))
        assertNull(loader.cached(small.copy(modifiedMs = small.modifiedMs + 1), 160))
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

    /**
     * A scan of the previous library folder that ends after the folder changed is not shown under the new one, and the new folder's scan
     * does not start from the old folder's tracks.
     */
    @Test
    fun aScanOfThePreviousFolderIsNotShownUnderTheNewOne() {
        InstrumentationRegistry.getInstrumentation()
            .uiAutomation
            .executeShellCommand("appops set --uid ${context.packageName} MANAGE_EXTERNAL_STORAGE allow")
            .close()
        repeat(OLD_TRACKS) { writeWav(File(root, "Old/Album/$it.wav"), seconds = 1) }
        val next =
            File(context.cacheDir, "test-library-next").apply {
                deleteRecursively()
                mkdirs()
            }
        try {
            writeWav(File(next, "New/Album/new.wav"), seconds = 1)
            app.repository.setLibraryRoot(root.absolutePath)
            val scanning = System.currentTimeMillis() + 10_000
            while (System.currentTimeMillis() < scanning && !app.repository.state.value.scanning) Thread.sleep(1)
            app.repository.setLibraryRoot(next.absolutePath)
            var stale: String? = null
            var settled = false
            val end = System.currentTimeMillis() + 60_000
            while (!settled && System.currentTimeMillis() < end) {
                val state = app.repository.state.value
                val shown = state.root == next
                if (shown) stale = stale ?: state.index.tracks.firstOrNull { it.path.startsWith("Old/") }?.path
                settled = shown && state.scanned()
                Thread.sleep(2)
            }
            assertNull("shown under the new folder: ${stale.orEmpty()}", stale)
            assertEquals(listOf("New/Album/new.wav"), app.repository.state.value.index.tracks.map { it.path })
        } finally {
            next.deleteRecursively()
        }
    }

    /** R2-opus-5: quick playlist changes run one after another, so none is lost. */
    @Test
    fun quickPlaylistChangesAreAllKept() {
        val tracks = (1..12).map { Track.fromTags("Quick/Album/$it.mp3", io.github.brainage04.spotless.library.RawTags(), 1, 1) }
        tracks.forEach { app.repository.setLiked(it, true) }
        val store = io.github.brainage04.spotless.library.PlaylistStore(root)
        val end = System.currentTimeMillis() + 20_000
        while (System.currentTimeMillis() < end && store.likedPaths().size < tracks.size) Thread.sleep(100)
        Thread.sleep(500)
        assertEquals(tracks.map { it.path }.toSet(), store.likedPaths())
        // Liking inserts at the top: the order of the taps is kept.
        assertEquals(
            tracks.reversed().map { it.path },
            store.read(io.github.brainage04.spotless.library.LibraryFormat.LIKED_SONGS)!!.entries.map { it.path },
        )
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
        for (path in
            listOf(
                "../test-outside/Private/Secret.wav",
                "Artist/../../test-outside/Private/Secret.wav",
                File(outside, "Private/Secret.wav").absolutePath,
                "Artist/Album/cover.jpg",
                "Artist/Album/Missing.wav",
            )) {
            try {
                context.contentResolver.openFileDescriptor(CoverProvider.uri(context, path), "r")?.close()
                fail("served a cover for $path")
            } catch (_: FileNotFoundException) {}
        }
    }

    /** Audit finding 8: controller-supplied metadata, artwork URIs in particular, is replaced by the library's. */
    @Test
    fun resolveRebuildsControllerItemsFromTheLibrary() {
        val path = "Artist/Album/Song [abc].wav"
        val file = File(root, path)
        writeWav(file, seconds = 1)
        val spoofed =
            MediaItem.Builder()
                .setMediaId("q1|Somewhere/Else.mp3")
                .setUri(Uri.fromFile(file))
                .setMediaMetadata(
                    MediaMetadata.Builder()
                        .setTitle("Spoofed")
                        .setArtworkUri(Uri.parse("file:///sdcard/DCIM/Camera/private.jpg"))
                        .setArtworkData(byteArrayOf(1, 2, 3), MediaMetadata.PICTURE_TYPE_FRONT_COVER)
                        .build()
                )
                .build()
        val resolved = QueueItems.resolve(context, spoofed, root, app.repository::track)!!
        assertEquals("q1|$path", resolved.mediaId)
        assertEquals(Uri.fromFile(File(root, path)), resolved.localConfiguration!!.uri)
        assertEquals("Song", resolved.mediaMetadata.title?.toString())
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
        val ids =
            listOf(dup, dup, dup, dup.buildUpon().setMediaId("taken|x").build(), dup.buildUpon().setMediaId("fresh|x").build()).map {
                QueueItems.id(QueueItems.resolve(context, it, root, app.repository::track, used)!!)
            }
        assertEquals("dup", ids[0])
        assertEquals("fresh", ids[4])
        assertEquals(ids.size, ids.toSet().size)
        assertTrue("taken" !in ids)
        assertEquals(used, ids.toSet() + "taken")

        // Through the real session, as another app's controller would send them.
        val main = Handler(Looper.getMainLooper())
        val token = SessionToken(context, ComponentName(context, PlaybackService::class.java))
        val controller =
            MediaController.Builder(context, token).setApplicationLooper(Looper.getMainLooper()).buildAsync().get(10, TimeUnit.SECONDS)
        try {
            val done = CountDownLatch(1)
            var queued = emptyList<String>()
            main.post {
                controller.setMediaItems(listOf(dup, dup, dup))
                controller.addMediaItems(listOf(dup))
                main.postDelayed(
                    {
                        queued = (0 until controller.mediaItemCount).map { controller.getMediaItemAt(it).mediaId }
                        done.countDown()
                    },
                    1_000,
                )
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
        val item = io.github.brainage04.spotless.playback.StoredItem("same", "a.wav", "file:///a.wav", "A", "B", "C")
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
        val sent =
            QueueItems.build(context, root, track).let { item ->
                item
                    .buildUpon()
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
                controller.addListener(
                    object : Player.Listener {
                        override fun onIsPlayingChanged(isPlaying: Boolean) {
                            if (isPlaying) playing.countDown()
                        }
                    }
                )
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

    private fun LibraryState.scanned() = loaded && !scanning && index.tracks.isNotEmpty()

    private companion object {
        /** Enough files that their scan is still running when the folder changes. */
        const val OLD_TRACKS = 200
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
