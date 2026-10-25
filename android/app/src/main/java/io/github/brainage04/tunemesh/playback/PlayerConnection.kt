package io.github.brainage04.tunemesh.playback

import android.content.ComponentName
import android.content.Context
import android.os.Looper
import androidx.core.content.ContextCompat
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import io.github.brainage04.tunemesh.library.Track
import java.io.File
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

data class PlayerUiState(
    val connected: Boolean = false,
    val queue: List<MediaItem> = emptyList(),
    val currentIndex: Int = -1,
    val isPlaying: Boolean = false,
    val playWhenReady: Boolean = false,
    val playbackState: Int = Player.STATE_IDLE,
    val durationMs: Long = 0,
    val repeatMode: Int = Player.REPEAT_MODE_OFF,
    val shuffle: Boolean = false,
) {
    val current: MediaItem?
        get() = queue.getOrNull(currentIndex)

    val currentPath: String?
        get() = current?.let(QueueItems::path)

    /** Pause is offered while playback is requested and can continue (not after the queue ended or failed). */
    val showPause: Boolean
        get() = playWhenReady && (playbackState == Player.STATE_READY || playbackState == Player.STATE_BUFFERING)
}

/**
 * The app's [MediaController] to [PlaybackService]. Shuffle is implemented here by reordering the queue (the context order is remembered to
 * undo it), so the queue screen always shows the real play order and "Play next" really plays next.
 */
class PlayerConnection(private val context: Context, private val state: PlaybackState) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var controller: MediaController? = null
    private var connecting = false
    private var ticker: Job? = null
    private val pending = mutableListOf<(MediaController) -> Unit>()

    private val _ui = MutableStateFlow(PlayerUiState(shuffle = state.shuffle))
    val ui: StateFlow<PlayerUiState> = _ui
    private val _position = MutableStateFlow(0L)
    val position: StateFlow<Long> = _position

    private val listener =
        object : Player.Listener {
            override fun onEvents(player: Player, events: Player.Events) = publish()
        }

    fun connect() {
        if (controller != null || connecting) return
        connecting = true
        val token = SessionToken(context, ComponentName(context, PlaybackService::class.java))
        val future = MediaController.Builder(context, token).buildAsync()
        future.addListener(
            {
                connecting = false
                val built = runCatching { future.get() }.getOrNull() ?: return@addListener
                controller = built
                built.addListener(listener)
                publish()
                pending.toList().forEach { it(built) }
                pending.clear()
            },
            ContextCompat.getMainExecutor(context),
        )
    }

    fun disconnect() {
        ticker?.cancel()
        controller?.run {
            removeListener(listener)
            release()
        }
        controller = null
        _ui.update { it.copy(connected = false) }
    }

    /** Runs [action] on the main thread (MediaController's application thread) once connected. */
    private fun withController(action: (MediaController) -> Unit) {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            scope.launch { withController(action) }
            return
        }
        val current = controller
        if (current != null) action(current)
        else {
            pending += action
            connect()
        }
    }

    private fun publish() {
        val player = controller ?: return
        val queue = (0 until player.mediaItemCount).map { player.getMediaItemAt(it) }
        _ui.value =
            PlayerUiState(
                connected = true,
                queue = queue,
                currentIndex = if (queue.isEmpty()) -1 else player.currentMediaItemIndex,
                isPlaying = player.isPlaying,
                playWhenReady = player.playWhenReady,
                playbackState = player.playbackState,
                durationMs = player.duration.takeIf { it > 0 } ?: (player.currentMediaItem?.mediaMetadata?.durationMs ?: 0),
                repeatMode = player.repeatMode,
                shuffle = state.shuffle,
            )
        _position.value = player.currentPosition
        ticker?.cancel()
        if (player.isPlaying) {
            ticker = scope.launch {
                while (isActive) {
                    _position.value = player.currentPosition
                    delay(500)
                }
            }
        }
    }

    /** Plays [tracks] as a context from [startIndex]; a negative index with shuffle starts at a random track. */
    fun play(root: File, tracks: List<Track>, startIndex: Int, shuffle: Boolean? = null) {
        if (tracks.isEmpty()) return
        val shuffled = shuffle ?: state.shuffle
        val items = tracks.map { QueueItems.build(context, root, it) }
        state.originalOrder = items.map(QueueItems::id)
        state.shuffle = shuffled
        val ordered: List<MediaItem>
        val start: Int
        if (shuffled) {
            val first = if (startIndex in items.indices) startIndex else items.indices.random()
            ordered = listOf(items[first]) + (items.take(first) + items.drop(first + 1)).shuffled()
            start = 0
        } else {
            ordered = items
            start = startIndex.coerceIn(0, items.lastIndex)
        }
        withController { player ->
            player.setMediaItems(ordered, start, 0)
            player.prepare()
            player.play()
            publish()
        }
    }

    fun playNext(root: File, tracks: List<Track>) = insert(root, tracks, next = true)

    fun addToQueue(root: File, tracks: List<Track>) = insert(root, tracks, next = false)

    private fun insert(root: File, tracks: List<Track>, next: Boolean) {
        if (tracks.isEmpty()) return
        val items = tracks.map { QueueItems.build(context, root, it) }
        withController { player ->
            if (player.mediaItemCount == 0) {
                state.originalOrder = items.map(QueueItems::id)
                player.setMediaItems(items)
                player.prepare()
                return@withController
            }
            val ids = items.map(QueueItems::id)
            val original = state.originalOrder.toMutableList()
            if (next) {
                player.addMediaItems(player.currentMediaItemIndex + 1, items)
                val currentId = player.currentMediaItem?.let(QueueItems::id)
                val at = original.indexOf(currentId).let { if (it < 0) original.size else it + 1 }
                original.addAll(at, ids)
            } else {
                player.addMediaItems(items)
                original.addAll(ids)
            }
            state.originalOrder = original
        }
    }

    fun removeAt(index: Int) = withController { player ->
        if (index !in 0 until player.mediaItemCount) return@withController
        val id = QueueItems.id(player.getMediaItemAt(index))
        player.removeMediaItem(index)
        state.originalOrder = state.originalOrder - id
    }

    /** Removes every queue entry for a deleted file. */
    fun removePath(path: String) = withController { player ->
        for (index in player.mediaItemCount - 1 downTo 0) {
            if (QueueItems.path(player.getMediaItemAt(index)) == path) removeAt(index)
        }
    }

    fun move(from: Int, to: Int) = withController { player ->
        if (from !in 0 until player.mediaItemCount || to !in 0 until player.mediaItemCount) return@withController
        player.moveMediaItem(from, to)
        if (!state.shuffle) state.originalOrder = (0 until player.mediaItemCount).map { QueueItems.id(player.getMediaItemAt(it)) }
    }

    fun skipTo(index: Int) = withController { player ->
        if (index !in 0 until player.mediaItemCount) return@withController
        player.seekToDefaultPosition(index)
        player.prepare()
        player.play()
    }

    fun togglePlay() = withController { player ->
        if (player.playWhenReady && (player.playbackState == Player.STATE_READY || player.playbackState == Player.STATE_BUFFERING)) {
            player.pause()
        } else {
            if (player.playbackState == Player.STATE_IDLE) player.prepare()
            if (player.playbackState == Player.STATE_ENDED) player.seekToDefaultPosition(0)
            player.play()
        }
    }

    fun next() = withController { it.seekToNext() }

    fun previous() = withController { it.seekToPrevious() }

    fun seekTo(positionMs: Long) = withController { player ->
        player.seekTo(positionMs)
        _position.value = positionMs
    }

    fun cycleRepeat() = withController { player ->
        player.repeatMode =
            when (player.repeatMode) {
                Player.REPEAT_MODE_OFF -> Player.REPEAT_MODE_ALL
                Player.REPEAT_MODE_ALL -> Player.REPEAT_MODE_ONE
                else -> Player.REPEAT_MODE_OFF
            }
    }

    fun toggleShuffle() = withController { player ->
        val enable = !state.shuffle
        val count = player.mediaItemCount
        val current = player.currentMediaItemIndex
        state.shuffle = enable
        if (count > 1 && current in 0 until count) {
            val items = (0 until count).map { player.getMediaItemAt(it) }
            val currentItem = items[current]
            val (before, after) =
                if (enable) {
                    state.originalOrder = items.map(QueueItems::id)
                    emptyList<MediaItem>() to (items.take(current) + items.drop(current + 1)).shuffled()
                } else {
                    val byId = items.associateBy(QueueItems::id)
                    val known = state.originalOrder.filter { it in byId }
                    val extra = items.map(QueueItems::id).filter { it !in known.toSet() && it != QueueItems.id(currentItem) }
                    val order = (known + extra).mapNotNull(byId::get)
                    val at = order.indexOfFirst { QueueItems.id(it) == QueueItems.id(currentItem) }
                    if (at < 0) emptyList<MediaItem>() to order else order.take(at) to order.drop(at + 1)
                }
            // Keep the playing item in place so playback is not interrupted.
            if (current + 1 < count) player.removeMediaItems(current + 1, count)
            if (current > 0) player.removeMediaItems(0, current)
            if (after.isNotEmpty()) player.addMediaItems(1, after)
            if (before.isNotEmpty()) player.addMediaItems(0, before)
        }
        publish()
    }
}
