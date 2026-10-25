package io.github.brainage04.tunemesh.baselineprofile

import androidx.benchmark.macro.BaselineProfileMode
import androidx.benchmark.macro.CompilationMode
import androidx.benchmark.macro.FrameTimingMetric
import androidx.benchmark.macro.MacrobenchmarkScope
import androidx.benchmark.macro.Metric
import androidx.benchmark.macro.StartupMode
import androidx.benchmark.macro.StartupTimingMetric
import androidx.benchmark.macro.junit4.MacrobenchmarkRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.filters.LargeTest
import androidx.test.uiautomator.By
import org.junit.BeforeClass
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The hostile-library flows on the minified benchmark build (`benchmarkRelease`): start, search, the 40,000-track album, a 250,000-entry
 * playlist, the Downloads tab and scrolling Your Library. Each reports [MainThreadStallMetric] (budget: no main-thread stall or frame over
 * 100 ms) and the frame timings; every iteration leaves a Perfetto trace in the test output. Run on the emulator only:
 * ```sh
 * ANDROID_SERIAL=emulator-5554 ./gradlew --no-daemon :baselineprofile:connectedBenchmarkReleaseAndroidTest
 * ```
 */
@LargeTest
@RunWith(AndroidJUnit4::class)
class HostileLibraryBenchmark {
    @get:Rule val rule = MacrobenchmarkRule()

    @Test
    fun startup() =
        measure(
            listOf(StartupTimingMetric(), FrameTimingMetric(), MainThreadStallMetric(afterFirstFrame = true)),
            StartupMode.COLD,
            // Scans the library on the first run; the process is killed after setup, so each start reads the tag cache.
            setup = {
                openApp()
                pressHome()
            },
        ) {
            startActivityAndWait()
            device.waitFor(By.text("Recently added"))
            device.waitFor(By.text("Your playlists"))
        }

    @Test
    fun search() =
        measure(
            setup = {
                openApp()
                tab("Search")
            }
        ) {
            searchFor("One Album")
        }

    @Test
    fun openHugeAlbum() =
        measure(
            setup = {
                openApp()
                searchFor("One Album")
                waitUntilIdle()
            }
        ) {
            openSearchResult("One Album")
            device.waitFor(By.textStartsWith("Album • "))
            fling(3)
        }

    @Test
    fun openHugePlaylist() =
        measure(
            setup = {
                openApp()
                searchFor("Big 00")
                waitUntilIdle()
            }
        ) {
            openSearchResult("Big 00")
            device.waitFor(By.text("Edit order"))
            fling(3)
        }

    @Test
    fun downloadsTab() =
        measure(
            setup = {
                openApp()
                tab("Home")
            }
        ) {
            click(By.text("Downloads"))
            device.waitFor(By.text("Requests"))
            fling(3)
        }

    @Test
    fun scrollLibrary() =
        measure(
            setup = {
                openApp()
                tab("Your Library")
                waitUntilIdle()
            }
        ) {
            fling(5)
        }

    private fun measure(
        metrics: List<Metric> = listOf(FrameTimingMetric(), MainThreadStallMetric()),
        startupMode: StartupMode? = null,
        setup: MacrobenchmarkScope.() -> Unit,
        block: MacrobenchmarkScope.() -> Unit,
    ) =
        rule.measureRepeated(
            packageName = PACKAGE,
            metrics = metrics,
            compilationMode = CompilationMode.Partial(BaselineProfileMode.UseIfAvailable),
            startupMode = startupMode,
            iterations = ITERATIONS,
            setupBlock = setup,
            measureBlock = block,
        )

    companion object {
        private const val ITERATIONS = 3

        @JvmStatic @BeforeClass fun library() = HostileSetup.ensureLibrary()
    }
}
