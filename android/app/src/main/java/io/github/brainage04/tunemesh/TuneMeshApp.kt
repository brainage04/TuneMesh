package io.github.brainage04.tunemesh

import android.app.Application
import io.github.brainage04.tunemesh.data.AppSettings
import io.github.brainage04.tunemesh.data.CoverLoader
import io.github.brainage04.tunemesh.data.LibraryRepository
import io.github.brainage04.tunemesh.data.LibraryScanner
import io.github.brainage04.tunemesh.playback.PlaybackState
import java.io.File

class TuneMeshApp : Application() {
    val settings by lazy { AppSettings(this) }
    val repository by lazy { LibraryRepository(settings, LibraryScanner(File(filesDir, "tag-cache.json"))) }
    val covers by lazy { CoverLoader(File(cacheDir, "covers")) }
    val playbackState by lazy { PlaybackState(this) }

    override fun onCreate() {
        super.onCreate()
        repository.start()
    }
}
