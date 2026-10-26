package io.github.brainage04.tunemesh

import android.content.Context
import android.os.SystemClock
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.brainage04.tunemesh.library.DownloadStore
import io.github.brainage04.tunemesh.library.Limits
import java.io.File
import kotlinx.coroutines.runBlocking
import leakcanary.DetectLeaksAfterTestSuccess
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * R3-Opus-1 on the device: 200 requests and 200 statuses with every field at its maximum (about 400 MB) are listed twice while the first
 * list is held, directly and through the app's own refresh, within the app's heap and without an error on the Downloads tab.
 */
@RunWith(AndroidJUnit4::class)
class DownloadsMemoryTest {
    @get:Rule val leaks = DetectLeaksAfterTestSuccess()

    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context: Context = instrumentation.targetContext
    private val app
        get() = context.applicationContext as TuneMeshApp

    private lateinit var root: File
    private lateinit var previousRoot: File

    @Before
    fun setUp() {
        instrumentation.uiAutomation.executeShellCommand("appops set --uid ${context.packageName} MANAGE_EXTERNAL_STORAGE allow").close()
        root = File(context.filesDir, "downloads-memory").apply { deleteRecursively() }
        previousRoot = app.settings.libraryRoot
    }

    @After
    fun tearDown() {
        app.repository.setLibraryRoot(previousRoot.absolutePath)
        root.deleteRecursively()
    }

    @Test
    fun maximalDownloadsFitTheAppHeap() {
        val bytes = HostileFiles.writeDownloads(root)
        report("wrote $bytes B of request and status files; ${heap()}")
        val store = DownloadStore(root)
        val first = store.list()
        val second = store.list()
        assertEquals(Limits.MAX_DOWNLOADS_READ, first.items.size)
        assertEquals(first, second)
        report("two lists held; ${heap()}")

        app.repository.setLibraryRoot(root.absolutePath)
        val end = SystemClock.uptimeMillis() + 120_000
        while (app.repository.state.value.downloads.items.size < Limits.MAX_DOWNLOADS_READ) {
            assertTrue("the app did not list the downloads", SystemClock.uptimeMillis() < end)
            Thread.sleep(200)
        }
        runBlocking {
            app.repository.refresh()
            app.repository.refresh()
        }
        val state = app.repository.state.value
        assertNull(state.downloadsError)
        assertEquals(Limits.MAX_DOWNLOADS_READ, state.downloads.items.size)
        report("after the app's refreshes, still holding both lists (${first.items.size + second.items.size} items); ${heap()}")
    }

    @Suppress("ExplicitGarbageCollectionCall") // measures what the lists keep, without garbage
    private fun heap(): String {
        System.gc()
        val runtime = Runtime.getRuntime()
        val used = (runtime.totalMemory() - runtime.freeMemory()) / (1024 * 1024)
        return "heap $used MiB used of ${runtime.maxMemory() / (1024 * 1024)} MiB"
    }

    private fun report(line: String) {
        Log.i(TAG, line)
        instrumentation.sendStatus(0, android.os.Bundle().apply { putString("stream", "$line\n") })
    }

    companion object {
        private const val TAG = "DownloadsMemory"
    }
}
