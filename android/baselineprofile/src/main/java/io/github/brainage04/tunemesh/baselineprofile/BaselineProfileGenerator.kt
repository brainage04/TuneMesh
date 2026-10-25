package io.github.brainage04.tunemesh.baselineprofile

import androidx.benchmark.macro.junit4.BaselineProfileRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.filters.LargeTest
import androidx.test.uiautomator.By
import org.junit.BeforeClass
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Generates `app/src/main/generated/baselineProfiles` with `./gradlew --no-daemon :app:generateBaselineProfile` (emulator only): start,
 * every tab, search, an album, a playlist and scrolling, on the hostile library ([HostileSetup]).
 */
@RunWith(AndroidJUnit4::class)
@LargeTest
class BaselineProfileGenerator {
    @get:Rule val baselineProfileRule = BaselineProfileRule()

    @Test
    fun generate() =
        // Each iteration waits for the app to load the hostile library and settle, so fewer than the default 15.
        baselineProfileRule.collect(packageName = PACKAGE, maxIterations = 5, stableIterations = 2) {
            pressHome()
            startActivityAndWait()
            openApp()
            tab("Your Library")
            fling(2)
            searchFor("One Album")
            openSearchResult("One Album")
            device.waitFor(By.textStartsWith("Album • "))
            fling(2)
            searchFor("Big 00")
            openSearchResult("Big 00")
            device.waitFor(By.text("Edit order"))
            fling(2)
            tab("Downloads")
            fling(2)
            tab("Home")
        }

    companion object {
        @JvmStatic @BeforeClass fun library() = HostileSetup.ensureLibrary()
    }
}
