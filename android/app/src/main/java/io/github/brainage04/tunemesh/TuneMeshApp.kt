package io.github.brainage04.tunemesh

import android.app.Application
import android.graphics.Paint
import io.github.brainage04.tunemesh.data.AppSettings
import io.github.brainage04.tunemesh.data.CoverLoader
import io.github.brainage04.tunemesh.data.LibraryRepository
import io.github.brainage04.tunemesh.data.LibraryScanner
import io.github.brainage04.tunemesh.playback.PlaybackState
import java.io.File

class TuneMeshApp : Application() {
    val settings by lazy { AppSettings(this) }
    val repository by lazy {
        LibraryRepository(settings, LibraryScanner(File(filesDir, LibraryScanner.CACHE_FILE)), File(filesDir, "playlist-base"))
    }
    val covers by lazy { CoverLoader(File(cacheDir, "covers")) }
    val playbackState by lazy { PlaybackState(this) }

    override fun onCreate() {
        super.onCreate()
        // The first text layout loads the system font from storage, which took up to 160 ms of the first screen's frame on the
        // emulator; this loads it while the activity starts. The arrow the Downloads cards show ("→ playlist") is not in the system font:
        // its fallback font (NotoSansSymbols) took 3-4 ms of the first Downloads frame, so it is loaded here too.
        Thread({ Paint().measureText("TuneMesh \u2192") }, "font-warmup").start()
        repository.start()
    }
}
