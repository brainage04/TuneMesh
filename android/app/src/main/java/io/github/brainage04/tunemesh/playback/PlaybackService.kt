package io.github.brainage04.tunemesh.playback

import android.app.PendingIntent
import android.content.Intent
import android.os.Handler
import android.os.Looper
import androidx.annotation.OptIn
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import io.github.brainage04.tunemesh.MainActivity
import io.github.brainage04.tunemesh.TuneMeshApp

/**
 * Owns the ExoPlayer and its media session: background playback, the media notification, lock screen and
 * headset/Bluetooth buttons. Audio focus and becoming-noisy are handled by ExoPlayer. The queue is saved to
 * app storage and restored (paused) when the service starts again.
 */
class PlaybackService : MediaSessionService() {
    private var session: MediaSession? = null
    private val handler = Handler(Looper.getMainLooper())
    private val state by lazy { (application as TuneMeshApp).playbackState }

    private val savePosition = object : Runnable {
        override fun run() {
            save()
            handler.postDelayed(this, SAVE_INTERVAL_MS)
        }
    }

    override fun onCreate() {
        super.onCreate()
        val player = ExoPlayer.Builder(this)
            .setAudioAttributes(
                AudioAttributes.Builder().setUsage(C.USAGE_MEDIA).setContentType(C.AUDIO_CONTENT_TYPE_MUSIC).build(),
                true,
            )
            .setHandleAudioBecomingNoisy(true)
            .setWakeMode(C.WAKE_MODE_LOCAL)
            .build()
        player.addListener(object : Player.Listener {
            override fun onEvents(player: Player, events: Player.Events) {
                if (events.containsAny(
                        Player.EVENT_TIMELINE_CHANGED,
                        Player.EVENT_MEDIA_ITEM_TRANSITION,
                        Player.EVENT_IS_PLAYING_CHANGED,
                        Player.EVENT_REPEAT_MODE_CHANGED,
                        Player.EVENT_POSITION_DISCONTINUITY,
                    )
                ) {
                    save()
                }
                if (events.contains(Player.EVENT_IS_PLAYING_CHANGED)) {
                    handler.removeCallbacks(savePosition)
                    if (player.isPlaying) handler.postDelayed(savePosition, SAVE_INTERVAL_MS)
                }
            }

            override fun onPlayerError(error: PlaybackException) {
                // While playing, a file that vanished (deleted on another device) should not stop the whole
                // queue. A paused player (for example a restored queue without storage access) stays put.
                if (player.playWhenReady && player.hasNextMediaItem()) {
                    player.seekToNextMediaItem()
                    player.prepare()
                }
            }
        })
        val openApp = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        session = MediaSession.Builder(this, player)
            .setSessionActivity(openApp)
            .setCallback(Callback())
            .build()
        restore(player)
    }

    private fun restore(player: ExoPlayer) {
        val queue = state.loadQueue() ?: return
        if (queue.items.isEmpty()) return
        player.repeatMode = queue.repeatMode
        player.setMediaItems(
            QueueItems.withUniqueIds(queue.items).map(QueueItems::build),
            queue.index.coerceIn(0, queue.items.lastIndex),
            queue.positionMs,
        )
        player.playWhenReady = false
        player.prepare()
    }

    private fun save() {
        val player = session?.player ?: return
        val items = (0 until player.mediaItemCount).map { QueueItems.store(player.getMediaItemAt(it)) }
        state.saveQueue(
            StoredQueue(
                items = items,
                index = player.currentMediaItemIndex.coerceAtLeast(0),
                positionMs = player.currentPosition.coerceAtLeast(0),
                repeatMode = player.repeatMode,
            ),
        )
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? = session

    override fun onTaskRemoved(rootIntent: Intent?) {
        val player = session?.player
        if (player == null || !player.playWhenReady || player.mediaItemCount == 0) {
            save()
            stopSelf()
        }
    }

    override fun onDestroy() {
        handler.removeCallbacks(savePosition)
        save()
        session?.run {
            player.release()
            release()
        }
        session = null
        super.onDestroy()
    }

    private inner class Callback : MediaSession.Callback {
        override fun onAddMediaItems(
            mediaSession: MediaSession,
            controller: MediaSession.ControllerInfo,
            mediaItems: List<MediaItem>,
        ): ListenableFuture<List<MediaItem>> {
            val repository = (application as TuneMeshApp).repository
            val root = repository.root
            val player = mediaSession.player
            val used = (0 until player.mediaItemCount).mapTo(HashSet()) { QueueItems.id(player.getMediaItemAt(it)) }
            return Futures.immediateFuture(
                mediaItems.mapNotNull { QueueItems.resolve(this@PlaybackService, it, root, repository::track, used) },
            )
        }

        @OptIn(UnstableApi::class)
        override fun onPlaybackResumption(
            mediaSession: MediaSession,
            controller: MediaSession.ControllerInfo,
            isForPlayback: Boolean,
        ): ListenableFuture<MediaSession.MediaItemsWithStartPosition> {
            val queue = state.loadQueue()
            if (queue == null || queue.items.isEmpty()) {
                return Futures.immediateFailedFuture(UnsupportedOperationException("Nothing to resume"))
            }
            return Futures.immediateFuture(
                MediaSession.MediaItemsWithStartPosition(
                    QueueItems.withUniqueIds(queue.items).map(QueueItems::build),
                    queue.index.coerceIn(0, queue.items.lastIndex),
                    queue.positionMs,
                ),
            )
        }
    }

    companion object {
        private const val SAVE_INTERVAL_MS = 5_000L
    }
}
