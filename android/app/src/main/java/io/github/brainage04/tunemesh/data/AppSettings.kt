package io.github.brainage04.tunemesh.data

import android.content.Context
import android.os.Build
import android.os.Environment
import android.provider.Settings
import io.github.brainage04.tunemesh.library.DownloadStore
import java.io.File

/** Per-device settings in app storage (never in the library folder). */
class AppSettings(private val context: Context) {
    private val prefs = context.getSharedPreferences("settings", Context.MODE_PRIVATE)

    var libraryRoot: File
        get() = File(prefs.getString(KEY_ROOT, null) ?: defaultRoot().absolutePath)
        set(value) = prefs.edit().putString(KEY_ROOT, value.absolutePath).apply()

    /** `thomass-s21`-style name for `requestedBy`. */
    val deviceName: String
        get() {
            val name = Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
                ?.takeIf { it.isNotBlank() } ?: Build.MODEL
            return DownloadStore.deviceSlug(name)
        }

    companion object {
        private const val KEY_ROOT = "libraryRoot"

        @Suppress("DEPRECATION")
        fun defaultRoot(): File = File(Environment.getExternalStorageDirectory(), "Music/TuneMesh")
    }
}
