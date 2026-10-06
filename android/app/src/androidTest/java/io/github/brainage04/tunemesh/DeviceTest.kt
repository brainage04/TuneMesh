package io.github.brainage04.tunemesh

import android.content.ComponentName
import android.content.Context
import android.os.Handler
import android.os.Looper
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.brainage04.tunemesh.data.LibraryScanner
import io.github.brainage04.tunemesh.library.Track
import io.github.brainage04.tunemesh.playback.PlaybackService
import io.github.brainage04.tunemesh.playback.QueueItems
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.math.sin

/** Exercises the real MediaMetadataRetriever scan and the real ExoPlayer media session on a device. */
@RunWith(AndroidJUnit4::class)
class DeviceTest {
    private val context: Context = InstrumentationRegistry.getInstrumentation().targetContext
    private lateinit var root: File

    @Before
    fun setUp() {
        root = File(context.cacheDir, "test-library").apply { deleteRecursively(); mkdirs() }
    }

    @After
    fun tearDown() {
        root.deleteRecursively()
    }

    @Test
    fun scannerReadsDurationAppliesFallbacksAndCachesTags() {
        writeWav(File(root, "Test Artist/Test Album/Tone [abc123].wav"), seconds = 3)
        writeWav(File(root, ".tunemesh/requests/ignored.wav"), seconds = 1)
        writeWav(File(root, "Playlists/ignored.wav"), seconds = 1)
        val cache = File(context.cacheDir, "test-tag-cache.json").apply { delete() }

        val first = LibraryScanner(cache).scan(root).tracks
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

    @Test
    fun playbackServicePlaysAQueuedFile() {
        val path = "Test Artist/Test Album/Long Tone [def456].wav"
        val file = File(root, path)
        writeWav(file, seconds = 8)
        val track = Track(path, "Long Tone", "Test Artist", "Test Album", "Test Artist", 1, null, 8_000, file.length(), file.lastModified())
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
                controller.setMediaItem(QueueItems.build(context, root, track))
                controller.prepare()
                controller.play()
            }
            assertTrue("player never started", playing.await(10, TimeUnit.SECONDS))
            Thread.sleep(1_500)
            val position = CountDownLatch(1)
            var positionMs = 0L
            var mediaId = ""
            main.post {
                positionMs = controller.currentPosition
                mediaId = controller.currentMediaItem?.mediaId.orEmpty()
                position.countDown()
            }
            position.await(5, TimeUnit.SECONDS)
            assertTrue("position $positionMs", positionMs >= 800)
            assertTrue(mediaId.endsWith("|$path"))
        } finally {
            main.post {
                controller.stop()
                controller.clearMediaItems()
                controller.release()
            }
        }
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
