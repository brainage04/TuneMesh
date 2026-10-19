package io.github.brainage04.tunemesh.desktop.playback

import com.sun.jna.Library
import com.sun.jna.Native
import com.sun.jna.Pointer
import com.sun.jna.ptr.DoubleByReference

/** The part of libmpv's client API (`mpv/client.h`) the player uses. */
@Suppress("FunctionName")
interface LibMpv : Library {
    fun mpv_create(): Pointer?
    fun mpv_initialize(ctx: Pointer): Int
    fun mpv_terminate_destroy(ctx: Pointer)
    fun mpv_set_option_string(ctx: Pointer, name: String, data: String): Int
    fun mpv_set_property_string(ctx: Pointer, name: String, data: String): Int
    fun mpv_get_property(ctx: Pointer, name: String, format: Int, data: Pointer): Int
    fun mpv_command(ctx: Pointer, args: Array<String?>): Int
    fun mpv_observe_property(ctx: Pointer, replyUserdata: Long, name: String, format: Int): Int
    fun mpv_wait_event(ctx: Pointer, timeout: Double): Pointer
    fun mpv_wakeup(ctx: Pointer)
    fun mpv_error_string(error: Int): String

    /** `setlocale` from libc: libmpv refuses to start unless LC_NUMERIC is "C", and the JVM sets the user's locale. */
    interface LibC : Library {
        fun setlocale(category: Int, locale: String): Pointer?
    }

    companion object {
        const val LC_NUMERIC = 1
        const val FORMAT_FLAG = 3
        const val FORMAT_DOUBLE = 5

        const val EVENT_NONE = 0
        const val EVENT_SHUTDOWN = 1
        const val EVENT_END_FILE = 7
        const val EVENT_FILE_LOADED = 8
        const val EVENT_PROPERTY_CHANGE = 22

        const val END_FILE_EOF = 0
        const val END_FILE_ERROR = 4

        /** libmpv from `jna.library.path` or the linker path; distributions ship only the versioned soname. */
        fun load(): LibMpv = runCatching { Native.load("mpv", LibMpv::class.java) }
            .recoverCatching { Native.load("libmpv.so.2", LibMpv::class.java) }
            .getOrThrow()
    }
}

/** One libmpv instance configured for audio-only playback, with its event loop on a daemon thread. */
class Mpv(private val onEvent: (MpvEvent) -> Unit) : AutoCloseable {
    private val lib = LibMpv.load().also { Native.load("c", LibMpv.LibC::class.java).setlocale(LibMpv.LC_NUMERIC, "C") }
    private val handle: Pointer = lib.mpv_create() ?: error("mpv_create failed")
    private val thread: Thread
    @Volatile private var closed = false

    init {
        // The same options as the GTK app (linux/crates/tunemesh/src/mpv.rs).
        for ((name, value) in listOf(
            "config" to "no",
            "terminal" to "no",
            "input-default-bindings" to "no",
            "input-vo-keyboard" to "no",
            "osc" to "no",
            "ytdl" to "no",
            "load-scripts" to "no",
            "load-stats-overlay" to "no",
            "load-console" to "no",
            "load-select" to "no",
            "load-positioning" to "no",
            "load-context-menu" to "no",
            "load-commands" to "no",
            "load-auto-profiles" to "no",
            "input-terminal" to "no",
            "video" to "no",
            "audio-display" to "no",
            "idle" to "yes",
            "gapless-audio" to "weak",
            "prefetch-playlist" to "yes",
            "volume-max" to "100",
            "audio-client-name" to "TuneMesh",
            "title" to "TuneMesh",
            "demuxer-max-bytes" to "8MiB",
            "demuxer-max-back-bytes" to "1MiB",
        )) {
            lib.mpv_set_option_string(handle, name, value)
        }
        check(lib.mpv_initialize(handle) >= 0) { "mpv_initialize failed" }
        lib.mpv_observe_property(handle, PROP_PAUSE, "pause", LibMpv.FORMAT_FLAG)
        lib.mpv_observe_property(handle, PROP_DURATION, "duration", LibMpv.FORMAT_DOUBLE)
        thread = Thread(::eventLoop, "mpv-events").apply { isDaemon = true; start() }
    }

    fun command(vararg args: String): Int = lib.mpv_command(handle, arrayOf(*args, null))

    fun setProperty(name: String, value: String): Int = lib.mpv_set_property_string(handle, name, value)

    fun getDouble(name: String): Double? {
        val ref = DoubleByReference()
        return if (lib.mpv_get_property(handle, name, LibMpv.FORMAT_DOUBLE, ref.pointer) >= 0) ref.value else null
    }

    private fun eventLoop() {
        while (true) {
            val event = lib.mpv_wait_event(handle, -1.0)
            if (closed) return
            val id = event.getInt(0)
            when (id) {
                LibMpv.EVENT_NONE -> continue
                LibMpv.EVENT_SHUTDOWN -> return
                LibMpv.EVENT_FILE_LOADED -> onEvent(MpvEvent.FileLoaded)
                LibMpv.EVENT_END_FILE -> {
                    val data = event.getPointer(16)
                    onEvent(MpvEvent.EndFile(reason = data.getInt(0)))
                }
                LibMpv.EVENT_PROPERTY_CHANGE -> {
                    val property = event.getPointer(16)
                    val format = property.getInt(8)
                    val value = property.getPointer(16)
                    when (event.getLong(8)) {
                        PROP_PAUSE -> if (format == LibMpv.FORMAT_FLAG && value != null) onEvent(MpvEvent.Pause(value.getInt(0) != 0))
                        PROP_DURATION -> onEvent(MpvEvent.Duration(if (format == LibMpv.FORMAT_DOUBLE && value != null) value.getDouble(0) else 0.0))
                    }
                }
            }
        }
    }

    /** Stops the event thread first: libmpv must not be destroyed while another thread waits for its events. */
    @Synchronized
    override fun close() {
        if (closed) return
        closed = true
        lib.mpv_wakeup(handle)
        thread.join(2000)
        lib.mpv_terminate_destroy(handle)
    }

    private companion object {
        const val PROP_PAUSE = 1L
        const val PROP_DURATION = 2L
    }
}

sealed interface MpvEvent {
    data object FileLoaded : MpvEvent
    data class EndFile(val reason: Int) : MpvEvent
    data class Pause(val paused: Boolean) : MpvEvent
    data class Duration(val seconds: Double) : MpvEvent
}
