package io.github.brainage04.tunemesh

import android.content.Context
import android.content.Intent
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import java.io.File
import leakcanary.DetectLeaksAfterTestSuccess
import org.junit.After
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Opens the app's screens and the share sheet, recreates them as a rotation does and closes them; LeakCanary
 * ([DetectLeaksAfterTestSuccess]) fails the test when a destroyed activity, view model or view stays reachable.
 */
@RunWith(AndroidJUnit4::class)
class ActivityLeakTest {
    @get:Rule val leaks = DetectLeaksAfterTestSuccess()

    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context: Context = instrumentation.targetContext
    private val app
        get() = context.applicationContext as TuneMeshApp

    private val device = UiDevice.getInstance(instrumentation)
    private lateinit var root: File
    private lateinit var previousRoot: File

    @Before
    fun setUp() {
        instrumentation.uiAutomation.executeShellCommand("appops set --uid ${context.packageName} MANAGE_EXTERNAL_STORAGE allow").close()
        app.settings.syncthingSetupSeen = true
        root = File(context.cacheDir, "leak-library").apply { deleteRecursively() }
        for (track in 1..3) {
            File(root, "Leak Artist/Leak Album/$track.mp3")
                .apply { parentFile!!.mkdirs() }
                .writeBytes(HostileLibrary.mp3("Song $track", "Leak Artist", "Leak Album", track))
        }
        previousRoot = app.settings.libraryRoot
        app.repository.setLibraryRoot(root.absolutePath)
    }

    @After
    fun tearDown() {
        app.repository.setLibraryRoot(previousRoot.absolutePath)
        root.deleteRecursively()
    }

    @Test
    fun screensAreReleased() {
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            for (tab in listOf("Your Library", "Downloads", "Search", "Home")) {
                device.wait(Until.findObject(By.text(tab)), 10_000)?.click()
                device.waitForIdle(2_000)
            }
            device.wait(Until.findObject(By.text("Leak Album")), 10_000)?.click()
            device.waitForIdle(2_000)
            scenario.recreate()
            scenario.recreate()
        }
        val share =
            Intent(context, ShareActivity::class.java)
                .setAction(Intent.ACTION_SEND)
                .setType("text/plain")
                .putExtra(Intent.EXTRA_TEXT, "https://youtu.be/jNQXAC9IVRw")
        ActivityScenario.launch<ShareActivity>(share).use { scenario -> scenario.recreate() }
    }
}
