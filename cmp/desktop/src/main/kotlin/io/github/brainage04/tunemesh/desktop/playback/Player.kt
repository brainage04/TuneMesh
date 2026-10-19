package io.github.brainage04.tunemesh.desktop.playback

import io.github.brainage04.tunemesh.library.Track
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.io.File

enum class RepeatMode { Off, All, One }

data class PlayerUi(
    val queue: List<Track> = emptyList(),
    val currentIndex: Int = -1,
    /** Playback is requested (the user pressed play) and the queue has not ended. */
    val playing: Boolean = false,
    val durationMs: Long = 0,
    val shuffle: Boolean = false,
    val repeat: RepeatMode = RepeatMode.Off,
    val volume: Int = 100,
    val context: String = "",
) {
    val current: Track? get() = queue.getOrNull(currentIndex)
    val currentPath: String? get() = current?.path
}

/**
 * Queue and transport on top of [Mpv]. The queue lives here (one file loaded at a time); shuffle reorders the
 * queue and remembers the context order to undo it, as on Android.
 */
class Player(private val root: () -> File) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val _ui = MutableStateFlow(PlayerUi())
    val ui: StateFlow<PlayerUi> = _ui
    private val _position = MutableStateFlow(0L)
    val position: StateFlow<Long> = _position
    private var contextOrder: List<Track> = emptyList()
    private var ticker: Job? = null

    private val mpv: Mpv? = runCatching { Mpv(::onEvent) }
        .onFailure { System.err.println("tunemesh: libmpv unavailable, playback disabled: $it") }
        .getOrNull()

    private fun onEvent(event: MpvEvent) {
        when (event) {
            is MpvEvent.EndFile -> if (event.reason == LibMpv.END_FILE_EOF || event.reason == LibMpv.END_FILE_ERROR) advance()
            is MpvEvent.Duration -> _ui.update { it.copy(durationMs = (event.seconds * 1000).toLong()) }
            is MpvEvent.Pause -> Unit
            MpvEvent.FileLoaded -> Unit
        }
    }

    private fun advance() {
        val state = _ui.value
        val next = when {
            state.repeat == RepeatMode.One -> state.currentIndex
            state.currentIndex + 1 < state.queue.size -> state.currentIndex + 1
            state.repeat == RepeatMode.All && state.queue.isNotEmpty() -> 0
            else -> -1
        }
        if (next < 0) {
            _ui.update { it.copy(playing = false) }
            stopTicker()
            _position.value = 0
        } else {
            load(next, play = true)
        }
    }

    private fun load(index: Int, play: Boolean) {
        val track = _ui.value.queue.getOrNull(index) ?: return
        _ui.update { it.copy(currentIndex = index, playing = play, durationMs = track.durationMs) }
        _position.value = 0
        mpv?.setProperty("pause", if (play) "no" else "yes")
        mpv?.command("loadfile", File(root(), track.path).path, "replace")
        if (play) startTicker() else stopTicker()
    }

    /** Polls the position twice a second while playing, like the GTK app. */
    private fun startTicker() {
        if (ticker?.isActive == true) return
        ticker = scope.launch {
            while (isActive) {
                mpv?.getDouble("time-pos")?.let { _position.value = (it * 1000).toLong() }
                delay(500)
            }
        }
    }

    private fun stopTicker() {
        ticker?.cancel()
        ticker = null
    }

    fun play(context: String, tracks: List<Track>, index: Int, shuffle: Boolean? = null) {
        if (tracks.isEmpty()) return
        val useShuffle = shuffle ?: _ui.value.shuffle
        contextOrder = tracks
        val start = if (index < 0) tracks.indices.random() else index
        val queue = if (useShuffle) listOf(tracks[start]) + (tracks.indices - start).shuffled().map { tracks[it] } else tracks
        _ui.update { it.copy(queue = queue, shuffle = useShuffle, context = context) }
        load(if (useShuffle) 0 else start, play = true)
    }

    fun togglePlay() {
        val state = _ui.value
        if (state.current == null) return
        if (!state.playing && _position.value == 0L && mpv?.getDouble("time-pos") == null) {
            load(state.currentIndex, play = true)
            return
        }
        val play = !state.playing
        mpv?.setProperty("pause", if (play) "no" else "yes")
        _ui.update { it.copy(playing = play) }
        if (play) startTicker() else stopTicker()
    }

    fun next() {
        val state = _ui.value
        when {
            state.currentIndex + 1 < state.queue.size -> load(state.currentIndex + 1, state.playing)
            state.repeat == RepeatMode.All && state.queue.isNotEmpty() -> load(0, state.playing)
        }
    }

    fun previous() {
        val state = _ui.value
        if (_position.value > 3000 || state.currentIndex <= 0) seekTo(0) else load(state.currentIndex - 1, state.playing)
    }

    fun skipTo(index: Int) = load(index, play = true)

    fun seekTo(ms: Long) {
        mpv?.command("seek", (ms / 1000.0).toString(), "absolute")
        _position.value = ms
    }

    fun setVolume(volume: Int) {
        mpv?.setProperty("volume", volume.toString())
        _ui.update { it.copy(volume = volume) }
    }

    fun toggleShuffle() {
        val state = _ui.value
        val current = state.current
        val upcoming = state.queue.drop(state.currentIndex + 1)
        val queue = when {
            current == null -> state.queue
            !state.shuffle -> state.queue.take(state.currentIndex + 1) + upcoming.shuffled()
            else -> {
                val position = contextOrder.indexOf(current)
                if (position >= 0) contextOrder else state.queue
            }
        }
        val index = if (current == null) -1 else if (!state.shuffle) state.currentIndex else queue.indexOf(current).coerceAtLeast(0)
        _ui.update { it.copy(queue = queue, currentIndex = index, shuffle = !state.shuffle) }
    }

    fun cycleRepeat() = _ui.update {
        it.copy(repeat = when (it.repeat) {
            RepeatMode.Off -> RepeatMode.All
            RepeatMode.All -> RepeatMode.One
            RepeatMode.One -> RepeatMode.Off
        })
    }

    fun playNext(tracks: List<Track>) = insert(tracks, atEnd = false)

    fun addToQueue(tracks: List<Track>) = insert(tracks, atEnd = true)

    private fun insert(tracks: List<Track>, atEnd: Boolean) {
        val state = _ui.value
        if (state.current == null) {
            play("Queue", tracks, 0, shuffle = false)
            return
        }
        val at = if (atEnd) state.queue.size else state.currentIndex + 1
        _ui.update { it.copy(queue = it.queue.toMutableList().apply { addAll(at, tracks) }) }
    }

    fun move(from: Int, to: Int) {
        val state = _ui.value
        if (from == state.currentIndex || to !in state.queue.indices || to == state.currentIndex) return
        val queue = state.queue.toMutableList().apply { add(to, removeAt(from)) }
        _ui.update { it.copy(queue = queue) }
    }

    fun removeAt(index: Int) {
        val state = _ui.value
        if (index == state.currentIndex || index !in state.queue.indices) return
        val queue = state.queue.toMutableList().apply { removeAt(index) }
        _ui.update { it.copy(queue = queue, currentIndex = if (index < state.currentIndex) state.currentIndex - 1 else state.currentIndex) }
    }

    fun shutdown() {
        stopTicker()
        mpv?.close()
    }
}
