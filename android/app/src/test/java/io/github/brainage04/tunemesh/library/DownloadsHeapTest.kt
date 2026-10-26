package io.github.brainage04.tunemesh.library

import io.github.brainage04.tunemesh.HostileFiles
import java.io.File
import java.util.concurrent.TimeUnit
import kotlinx.serialization.KSerializer
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

/**
 * R3-Opus-1: the Downloads list of 200 requests and 200 statuses with every field at its maximum, built twice while the first is still held
 * (as the 3 s refresh does), fits in the 192 MB heap of the emulator image (`dalvik.vm.heapgrowthlimit`; the app has no largeHeap). Runs
 * [DownloadsHeapCheck] in a JVM of its own with that heap.
 */
class DownloadsHeapTest {
    @get:Rule val temp = TemporaryFolder()

    @Test
    fun twoListsOfMaximalFilesFitTheAppHeap() {
        val root = temp.newFolder("lib")
        val bytes = HostileFiles.writeDownloads(root)
        val classpath =
            listOf(
                    DownloadsHeapCheck::class.java,
                    HostileFiles::class.java,
                    DownloadStore::class.java,
                    Unit::class.java,
                    Json::class.java,
                    KSerializer::class.java,
                )
                .map { File(it.protectionDomain.codeSource.location.toURI()).path }
                .distinct()
                .joinToString(File.pathSeparator)
        val java = File(System.getProperty("java.home"), "bin/java").path
        val process =
            ProcessBuilder(java, "-Xmx${HEAP_MB}m", "-cp", classpath, DownloadsHeapCheck::class.java.name, root.path)
                .redirectErrorStream(true)
                .start()
        val output = process.inputStream.bufferedReader().readText()
        assertTrue("timed out: $output", process.waitFor(5, TimeUnit.MINUTES))
        println("DownloadsHeap: $bytes B of files; $output")
        assertEquals(output, 0, process.exitValue())
        assertTrue(output, output.contains("400 items"))
    }

    companion object {
        const val HEAP_MB = 192
    }
}

/** Lists the downloads of `args[0]` twice, keeping both; exits non-zero on an OutOfMemoryError. */
object DownloadsHeapCheck {
    @JvmStatic
    @Suppress("ExplicitGarbageCollectionCall") // measures what both lists keep, without garbage
    fun main(args: Array<String>) {
        val store = DownloadStore(File(args[0]))
        val first = store.list()
        val second = store.list()
        System.gc()
        val runtime = Runtime.getRuntime()
        val used = (runtime.totalMemory() - runtime.freeMemory()) / (1024 * 1024)
        println("${first.items.size + second.items.size} items, $used MiB used of ${runtime.maxMemory() / (1024 * 1024)} MiB")
    }
}
