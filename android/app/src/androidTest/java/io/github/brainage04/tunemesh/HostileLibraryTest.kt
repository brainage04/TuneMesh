package io.github.brainage04.tunemesh

import android.content.Context
import android.content.Intent
import android.os.Debug
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.Direction
import androidx.test.uiautomator.StaleObjectException
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import io.github.brainage04.tunemesh.data.LibraryState
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.concurrent.atomic.AtomicLong

/** Marks the stress tests: excluded from the normal instrumented run, run alone with `-Pstress`. */
@Retention(AnnotationRetention.RUNTIME)
@Target(AnnotationTarget.CLASS, AnnotationTarget.FUNCTION)
annotation class Stress

/**
 * The hostile library ([HostileLibrary], `docs/library-format.md` "Limits") against the real app: generates it in
 * app storage, lets the app scan it, then opens Home, Your Library, the 40,000-track album, a 250,000-entry
 * playlist and Downloads through the UI. Fails on an OutOfMemoryError (the app and this test share a process, so
 * it ends the run), on a main-thread stall over [MAX_STALL_MS] (an ANR needs 5 s, so it is caught as a stall),
 * and when what the app shows does not follow the limits.
 *
 * Instrumentation argument `hostileScale`: `full` (default; the core generator's full profile) or `small`.
 */
@Stress
@RunWith(AndroidJUnit4::class)
class HostileLibraryTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context: Context = instrumentation.targetContext
    private val app get() = context.applicationContext as TuneMeshApp
    private val device = UiDevice.getInstance(instrumentation)
    private lateinit var root: File
    private lateinit var previousRoot: File
    private val watchdog = MainThreadWatchdog()

    @Before
    fun setUp() {
        shell("appops set --uid ${context.packageName} MANAGE_EXTERNAL_STORAGE allow")
        app.settings.syncthingSetupSeen = true
        root = File(context.filesDir, "hostile-library")
        previousRoot = app.settings.libraryRoot
    }

    @After
    fun tearDown() {
        watchdog.stop()
        app.repository.setLibraryRoot(previousRoot.absolutePath)
        root.deleteRecursively()
        File(context.filesDir, "tag-cache.json").delete()
        File(context.cacheDir, "covers").deleteRecursively()
    }

    @Test
    fun hostileLibraryStaysUsable() {
        val scale = when (InstrumentationRegistry.getArguments().getString("hostileScale")) {
            "small" -> HostileLibrary.Scale.SMALL
            else -> HostileLibrary.Scale.FULL
        }
        val library = HostileLibrary(root, scale)
        val generated = timed("generate") { library.generate() }
        report("generated $scale in $generated ms")
        val stignoreBytes = File(root, ".stignore").length()

        // Cold start of a debug build is not what this test is about: the watchdog starts once the app is up.
        launch()
        device.waitForIdle(5_000)
        watchdog.start()
        val scanned = timed("scan") {
            app.repository.setLibraryRoot(root.absolutePath)
            waitFor(SCAN_TIMEOUT_MS, "the first full scan") { state -> state.loaded && !state.scanning && state.index.tracks.isNotEmpty() }
        }
        val state = app.repository.state.value
        report("scanned ${state.index.tracks.size} tracks, ${state.index.albums.size} albums in $scanned ms; ${heap()}")

        // What the limits leave in the app state.
        val expected = library.expected
        assertEquals(expected.tracks, state.index.tracks.size)
        assertEquals(expected.albums, state.index.albums.size)
        assertNull(state.index.tracks.firstOrNull { it.path.endsWith("d200/deep.mp3") })
        assertNotNull(state.index.tracks.firstOrNull { it.path.endsWith("d010/deep.mp3") })
        val bigTag = state.index.tracks.single { it.path == "Huge/Tags/big tag.mp3" }
        assertTrue(bigTag.title.length <= 1024 && bigTag.album.length <= 1024)
        assertTrue(bigTag.artist.split('\u0000', ';').size <= 32)
        waitFor(60_000, "playlists listed") { it.playlists.size > expected.playlistsListed - 1 }
        val playlists = app.repository.state.value.playlists.filterNot { it.isLikedSongs }
        assertEquals(expected.playlistsListed, playlists.size)
        assertEquals(expected.tooLarge, playlists.count { it.problem != null })
        val downloads = waitFor(30_000, "downloads read") { it.downloads.items.isNotEmpty() }.downloads
        assertTrue("${downloads.items.size} downloads listed", downloads.items.size in 197..200)
        assertTrue(downloads.older > 0)
        assertTrue(downloads.items.all { (it.status?.message?.length ?: 0) <= 4 * 1024 && (it.status?.warnings?.size ?: 0) <= 50 })
        assertNull(app.repository.state.value.downloader.let { it as? io.github.brainage04.tunemesh.library.Downloader.Active })
        assertEquals(stignoreBytes, File(root, ".stignore").length())

        // Covers: the huge one is refused, the small one is shown.
        assertNull(app.covers.diskFile(root, state.index.tracks.single { it.path == "Huge/Cover/big cover.mp3" }))
        assertNotNull(app.covers.diskFile(root, state.index.tracks.single { it.path == "Huge/Small Cover/small cover.mp3" }))

        // The UI, screen by screen.
        step("Home") { tap("Home") }
        step("Your Library") { tap("Your Library") }
        step("Albums filter and scroll") {
            tapIfPresent("Albums")
            device.findObject(By.scrollable(true))?.fling(Direction.DOWN)
        }
        step("40,000-track album") {
            tap("Your Library")
            openFromSearch("One Album")
            device.findObject(By.scrollable(true))?.fling(Direction.DOWN)
        }
        step("250,000-entry playlist") {
            device.pressBack()
            openFromSearch("Big 00")
            waitFor(60_000, "the playlist's entries") { it.openPlaylist?.entries?.size == 250_000 }
            device.findObject(By.scrollable(true))?.fling(Direction.DOWN)
        }
        step("Downloads") {
            device.pressBack()
            tap("Downloads")
            device.findObject(By.scrollable(true))?.fling(Direction.DOWN)
            device.findObject(By.scrollable(true))?.fling(Direction.DOWN)
        }
        step("rescan while shown") {
            val before = app.repository.state.value.index
            runBlockingRefresh()
            assertTrue(app.repository.state.value.index !== before)
        }
        report("worst main-thread stall ${watchdog.worst} ms; ${heap()}")
        assertTrue("main thread stalled ${watchdog.worst} ms (${watchdog.worstAt})", watchdog.worst < MAX_STALL_MS)
    }

    private fun runBlockingRefresh() = kotlinx.coroutines.runBlocking { app.repository.refresh() }

    private fun launch() {
        val intent = context.packageManager.getLaunchIntentForPackage(context.packageName)!!
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
        context.startActivity(intent)
        device.wait(Until.hasObject(By.pkg(context.packageName).depth(0)), 10_000)
    }

    private fun openFromSearch(text: String) {
        tap("Search")
        val field = device.wait(Until.findObject(By.clazz("android.widget.EditText")), 5_000)
            ?: error("no search field")
        field.text = text // no keyboard opens, so nothing to close
        // The first match that is not the search field itself.
        val end = SystemClock.uptimeMillis() + 15_000
        while (SystemClock.uptimeMillis() < end) {
            val result = device.findObjects(By.textContains(text)).firstOrNull { it.className != "android.widget.EditText" }
            if (result != null) {
                try {
                    result.click()
                    device.waitForIdle(2_000)
                    return
                } catch (_: StaleObjectException) {
                }
            }
            Thread.sleep(200)
        }
        error("\"$text\" not found by search")
    }

    private fun tap(text: String) {
        // The screen may recompose between finding and clicking; find it again then.
        repeat(5) {
            val target = device.wait(Until.findObject(By.text(text)), 15_000) ?: error("no \"$text\" on screen")
            try {
                target.click()
                device.waitForIdle(2_000)
                return
            } catch (_: StaleObjectException) {
                Thread.sleep(200)
            }
        }
        error("\"$text\" kept changing")
    }

    private fun tapIfPresent(text: String) {
        device.wait(Until.findObject(By.text(text)), 3_000)?.click()
        device.waitForIdle(2_000)
    }

    private fun step(name: String, action: () -> Unit) {
        watchdog.label = name
        val ms = timed(name, action)
        report("$name: $ms ms, worst stall so far ${watchdog.worst} ms; ${heap()}")
    }

    private fun waitFor(timeoutMs: Long, what: String, done: (LibraryState) -> Boolean): LibraryState {
        val end = SystemClock.uptimeMillis() + timeoutMs
        while (SystemClock.uptimeMillis() < end) {
            val state = app.repository.state.value
            if (done(state)) return state
            Thread.sleep(200)
        }
        error("timed out waiting for $what")
    }

    private fun timed(name: String, action: () -> Unit): Long {
        val start = SystemClock.uptimeMillis()
        action()
        return (SystemClock.uptimeMillis() - start).also { Log.i(TAG, "$name took $it ms") }
    }

    private fun heap(): String {
        val runtime = Runtime.getRuntime()
        val used = (runtime.totalMemory() - runtime.freeMemory()) / (1024 * 1024)
        return "heap $used MiB of ${runtime.maxMemory() / (1024 * 1024)} MiB, native ${Debug.getNativeHeapAllocatedSize() / (1024 * 1024)} MiB"
    }

    private fun report(line: String) {
        Log.i(TAG, line)
        instrumentation.sendStatus(0, android.os.Bundle().apply { putString("stream", "$line\n") })
    }

    private fun shell(command: String) {
        instrumentation.uiAutomation.executeShellCommand(command).close()
    }

    /** Measures how late the main thread runs a message posted every [PERIOD_MS]: a stall shows as lateness. */
    private class MainThreadWatchdog {
        private val main = Handler(Looper.getMainLooper())
        @Volatile private var running = false
        private val worstMs = AtomicLong(0)
        @Volatile var label = "start"
        @Volatile var worstAt = ""
        val worst: Long get() = worstMs.get()

        fun start() {
            running = true
            Thread({
                while (running) {
                    val posted = SystemClock.uptimeMillis()
                    val ran = java.util.concurrent.CountDownLatch(1)
                    main.post { ran.countDown() }
                    ran.await()
                    val late = SystemClock.uptimeMillis() - posted
                    if (late > worstMs.get()) {
                        worstMs.set(late)
                        worstAt = label
                    }
                    Thread.sleep(PERIOD_MS)
                }
            }, "main-thread-watchdog").start()
        }

        fun stop() {
            running = false
        }
    }

    companion object {
        private const val TAG = "HostileLibrary"
        private const val MAX_STALL_MS = 1_000L
        private const val PERIOD_MS = 50L
        private const val SCAN_TIMEOUT_MS = 60 * 60 * 1000L
    }
}
