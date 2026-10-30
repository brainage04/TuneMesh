package io.github.brainage04.tunemesh

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.os.Debug
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import androidx.test.uiautomator.By
import androidx.test.uiautomator.Direction
import androidx.test.uiautomator.StaleObjectException
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import io.github.brainage04.tunemesh.data.LibraryScanner
import io.github.brainage04.tunemesh.data.LibraryState
import io.github.brainage04.tunemesh.library.Limits
import io.github.brainage04.tunemesh.library.ShownDownload
import java.io.File
import java.util.concurrent.atomic.AtomicLong
import leakcanary.DetectLeaksAfterTestSuccess
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.FixMethodOrder
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.MethodSorters

/** Marks the stress tests: excluded from the normal instrumented run, run alone with `-Pstress`. */
@Retention(AnnotationRetention.RUNTIME) @Target(AnnotationTarget.CLASS, AnnotationTarget.FUNCTION) annotation class Stress

/**
 * The hostile library ([HostileLibrary], `docs/library-format.md` "Limits") against the real app. [generateLibrary] generates it in app
 * storage and waits until it is written to disk; [hostileLibraryStaysUsable] then lets the app scan it and opens Home, Your Library, the
 * 40,000-track album, a 250,000-entry playlist and Downloads through the UI. `-Pstress` runs each test in a process of its own (the test
 * orchestrator), so the second one measures the app, not the generator's garbage and writes. It fails on an OutOfMemoryError (the app and
 * this test share a process, so it ends the run), on a main-thread stall over [MAX_STALL_MS] (an ANR needs 5 s, so it is caught as a
 * stall), and when what the app shows does not follow the limits. It reports how many stalls it saw over the budget of one 60 Hz frame
 * ([BUDGET_MS]); the macrobenchmarks (`HostileLibraryBenchmark`) measure those on the minified build, this test runs a debug one too.
 *
 * Instrumentation argument `hostileScale`: `full` (default; the core generator's full profile) or `small`.
 */
@Stress
@RunWith(AndroidJUnit4::class)
@FixMethodOrder(MethodSorters.NAME_ASCENDING) // generateLibrary first
class HostileLibraryTest {
    @get:Rule val leaks = DetectLeaksAfterTestSuccess()

    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context: Context = instrumentation.targetContext
    private val app
        get() = context.applicationContext as TuneMeshApp

    private val device = UiDevice.getInstance(instrumentation)
    private val root = File(context.filesDir, "hostile-library")
    private val scale =
        when (InstrumentationRegistry.getArguments().getString("hostileScale")) {
            "small" -> HostileLibrary.Scale.SMALL
            else -> HostileLibrary.Scale.FULL
        }

    /** Written once the library of [scale] is generated and on disk. */
    private val marker = File(root, ".generated-${if (scale == HostileLibrary.Scale.SMALL) "small" else "full"}")
    private lateinit var previousRoot: File
    private val watchdog = MainThreadWatchdog()
    private var cleanUp = false

    @Before
    fun setUp() {
        shell("appops set --uid ${context.packageName} MANAGE_EXTERNAL_STORAGE allow")
        app.settings.syncthingSetupSeen = true
        previousRoot = app.settings.libraryRoot
    }

    @After
    fun tearDown() {
        watchdog.stop()
        finishActivities()
        if (!cleanUp) return
        app.repository.setLibraryRoot(previousRoot.absolutePath)
        root.deleteRecursively()
        File(context.filesDir, LibraryScanner.CACHE_FILE).delete()
        File(context.cacheDir, "covers").deleteRecursively()
    }

    /** Generates the library, then waits until the kernel has written it out, so its writes do not land in the next test's measurements. */
    @Test
    fun generateLibrary() {
        marker.delete()
        val generated = timed("generate") { HostileLibrary(root, scale).generate() }
        report("generated $scale in $generated ms")
        val flushed = timed("writeback") { waitForWriteback() }
        report("written to disk in $flushed ms")
        marker.writeText("$scale\n")
    }

    @Test
    fun hostileLibraryStaysUsable() {
        check(marker.isFile) { "no generated library: run the stress tests with -Pstress, which runs generateLibrary first" }
        cleanUp = true
        val library = HostileLibrary(root, scale)
        val stignoreBytes = File(root, ".stignore").length()

        // Cold start of a debug build is not what this test is about: the watchdog starts once the app is up.
        launch()
        device.waitForIdle(5_000)
        watchdog.start()
        val scanned =
            timed("scan") {
                app.repository.setLibraryRoot(root.absolutePath)
                waitFor(SCAN_TIMEOUT_MS, "the first full scan") { state ->
                    state.root == root && state.loaded && !state.scanning && state.index.tracks.isNotEmpty()
                }
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
            // The list is read once the Downloads screen is shown.
            val downloads = waitFor(60_000, "downloads read") { it.downloads?.items?.isNotEmpty() == true }.downloads!!
            assertTrue("${downloads.items.size} downloads listed", downloads.items.size in 197..200)
            assertTrue(downloads.older > 0)
            assertTrue(
                downloads.items.all {
                    (it.shown.message?.length ?: 0) <= ShownDownload.PREVIEW_CHARS &&
                        it.shown.lines.size <= Limits.SHOWN_LIST_ROWS &&
                        it.shown.lines.all { line -> line.length <= ShownDownload.LINE_PREVIEW_CHARS } &&
                        it.trackCount <= Limits.MAX_STATUS_TRACKS &&
                        (it.playlist?.length ?: 0) <= Limits.SHOWN_LINE_CHARS
                }
            )
            device.findObject(By.scrollable(true))?.fling(Direction.DOWN)
            device.findObject(By.scrollable(true))?.fling(Direction.DOWN)
        }
        step("rescan while shown") {
            val before = app.repository.state.value.index
            runBlockingRefresh()
            // Nothing changed, so the screens keep the index they show.
            assertTrue(app.repository.state.value.index === before)
        }
        report("worst main-thread stall ${watchdog.worst} ms, ${watchdog.overBudget} over $BUDGET_MS ms; ${heap()}")
        assertTrue("main thread stalled ${watchdog.worst} ms (${watchdog.worstAt})", watchdog.worst < MAX_STALL_MS)
    }

    /** Flushes the page cache's dirty pages (`sync`) and waits until the kernel reports almost none dirty or under writeback. */
    private fun waitForWriteback() {
        shellAndWait("sync")
        val end = SystemClock.uptimeMillis() + WRITEBACK_TIMEOUT_MS
        while (SystemClock.uptimeMillis() < end) {
            val pending =
                File("/proc/meminfo")
                    .readLines()
                    .filter { it.startsWith("Dirty:") || it.startsWith("Writeback:") }
                    .sumOf { line ->
                        line.split(Regex("\\s+"))[1].toLong()
                    }
            if (pending < WRITEBACK_DONE_KB) return
            Thread.sleep(500)
        }
        error("the generated library was not written out within $WRITEBACK_TIMEOUT_MS ms")
    }

    /** Finishes the app's activities and waits until they are destroyed, so LeakCanary's check after the test covers them. */
    private fun finishActivities() {
        val monitor = ActivityLifecycleMonitorRegistry.getInstance()
        fun alive(): List<Activity> = Stage.entries.filter { it != Stage.DESTROYED }.flatMap { monitor.getActivitiesInStage(it) }
        instrumentation.runOnMainSync { alive().forEach(Activity::finish) }
        val end = SystemClock.uptimeMillis() + 10_000
        var remaining = true
        while (remaining && SystemClock.uptimeMillis() < end) {
            instrumentation.runOnMainSync { remaining = alive().isNotEmpty() }
            if (remaining) SystemClock.sleep(100)
        }
    }

    private fun runBlockingRefresh() = kotlinx.coroutines.runBlocking { app.repository.refresh() }

    private fun launch() {
        val intent =
            context.packageManager
                .getLaunchIntentForPackage(context.packageName)!!
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
        context.startActivity(intent)
        device.wait(Until.hasObject(By.pkg(context.packageName).depth(0)), 10_000)
    }

    private fun openFromSearch(text: String) {
        tap("Search")
        val field = device.wait(Until.findObject(By.clazz("android.widget.EditText")), 5_000) ?: error("no search field")
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
                } catch (_: StaleObjectException) {}
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

    /** Runs [command] and waits until it ends (its output closes). */
    private fun shellAndWait(command: String) {
        android.os.ParcelFileDescriptor.AutoCloseInputStream(instrumentation.uiAutomation.executeShellCommand(command)).use {
            it.readBytes()
        }
    }

    /** Measures how late the main thread runs a message posted every [PERIOD_MS]: a stall shows as lateness. */
    private class MainThreadWatchdog {
        private val main = Handler(Looper.getMainLooper())
        @Volatile private var running = false
        private val worstMs = AtomicLong(0)
        @Volatile var label = "start"
        @Volatile var worstAt = ""
        /** How many posted messages ran later than [BUDGET_MS]. */
        @Volatile var overBudget = 0
        val worst: Long
            get() = worstMs.get()

        fun start() {
            running = true
            Thread(
                    {
                        while (running) {
                            val posted = SystemClock.uptimeMillis()
                            val ran = java.util.concurrent.CountDownLatch(1)
                            main.post { ran.countDown() }
                            ran.await()
                            val late = SystemClock.uptimeMillis() - posted
                            if (late > BUDGET_MS) overBudget++
                            if (late > worstMs.get()) {
                                worstMs.set(late)
                                worstAt = label
                            }
                            Thread.sleep(PERIOD_MS)
                        }
                    },
                    "main-thread-watchdog",
                )
                .start()
        }

        fun stop() {
            running = false
        }
    }

    companion object {
        private const val TAG = "HostileLibrary"
        private const val MAX_STALL_MS = 1_000L
        /** One frame at 60 Hz. */
        private const val BUDGET_MS = 16.6
        private const val PERIOD_MS = 50L
        private const val SCAN_TIMEOUT_MS = 60 * 60 * 1000L
        private const val WRITEBACK_TIMEOUT_MS = 10 * 60 * 1000L

        /** Dirty and under-writeback page cache, in KiB, below which the library counts as written out. */
        private const val WRITEBACK_DONE_KB = 4 * 1024L
    }
}
