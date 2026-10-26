package io.github.brainage04.tunemesh.library

import com.code_intelligence.jazzer.api.FuzzedDataProvider
import com.code_intelligence.jazzer.junit.FuzzTest
import java.time.Instant
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue

/**
 * Jazzer targets for the synced request, status and `worker.json` files; see `fuzz` in app/build.gradle.kts. Any text parses to a value
 * within [Limits] or to null, and what the Downloads tab makes of it stays within the display limits.
 */
class DownloadJsonFuzzTest {
    @FuzzTest(maxDuration = "10s")
    fun request(data: FuzzedDataProvider) {
        val request = DownloadStore.parseRequest(data.consumeRemainingAsString()) ?: return
        assertTrue(Limits.utf8Length(request.input) <= Limits.MAX_REQUEST_INPUT_BYTES)
        assertTrue(Limits.utf8Length(request.playlist.orEmpty()) <= Limits.MAX_REQUEST_PLAYLIST_BYTES)
        assertTrue(Limits.utf8Length(request.id) <= Limits.MAX_DOWNLOAD_ID_BYTES)
        assertTrue(Limits.utf8Length(request.requestedBy) <= Limits.MAX_DEVICE_NAME_BYTES)
        assertTrue(Limits.utf8Length(request.requestedAt) <= Limits.MAX_TIMESTAMP_BYTES)
        checkShown(ShownDownload.of(request.input, null))
        DownloadStore.sortKey(request.id)
    }

    @FuzzTest(maxDuration = "10s")
    fun status(data: FuzzedDataProvider) {
        val status = DownloadStore.parseStatus(data.consumeRemainingAsString()) ?: return
        assertTrue(Limits.utf8Length(status.input) <= Limits.MAX_REQUEST_INPUT_BYTES)
        assertTrue(status.state in DownloadStatus.STATES)
        assertTrue(Limits.utf8Length(status.id) <= Limits.MAX_DOWNLOAD_ID_BYTES)
        assertTrue(Limits.utf8Length(status.title.orEmpty()) <= Limits.MAX_STATUS_TEXT_BYTES)
        assertTrue(Limits.utf8Length(status.message.orEmpty()) <= Limits.MAX_STATUS_MESSAGE_BYTES)
        assertTrue(status.trackCount in 0..Limits.MAX_STATUS_TRACKS)
        assertTrue(status.failures.size <= Limits.MAX_STATUS_FAILURES)
        assertTrue(status.failures.all { Limits.utf8Length(it.item) <= Limits.MAX_STATUS_TEXT_BYTES })
        assertTrue(status.failures.all { Limits.utf8Length(it.error) <= Limits.MAX_STATUS_TEXT_BYTES })
        assertTrue(status.warnings.size <= Limits.MAX_STATUS_WARNINGS)
        assertTrue(status.warnings.all { Limits.utf8Length(it) <= Limits.MAX_STATUS_TEXT_BYTES })
        checkShown(DownloadItem.of("1-a", null, status).shown)
    }

    @FuzzTest(maxDuration = "10s")
    fun worker(data: FuzzedDataProvider) {
        val now = Instant.ofEpochSecond(data.consumeLong(-31_557_014_167_219_200L, 31_556_889_864_403_199L))
        val info = DownloadStore.parseWorker(data.consumeRemainingAsString())
        if (info != null) {
            assertTrue(Limits.utf8Length(info.device) <= Limits.MAX_DEVICE_NAME_BYTES)
            assertTrue(Limits.utf8Length(info.heartbeatAt) <= Limits.MAX_TIMESTAMP_BYTES)
        }
        val downloader = Downloader.of(info, now)
        if (downloader is Downloader.Active) assertTrue(downloader.device.isNotBlank())
        assertFalse(downloader.description.contains('\n'))
    }

    private fun checkShown(shown: ShownDownload) {
        for (line in listOfNotNull(shown.input, shown.title)) {
            assertTrue(line.length <= Limits.SHOWN_LINE_CHARS)
            assertFalse(line.contains('\n') || line.contains('\r'))
        }
        assertTrue(shown.lines.all { it.length <= ShownDownload.LINE_PREVIEW_CHARS })
        for (message in listOfNotNull(shown.message) + shown.lines) {
            assertTrue(message.length <= Limits.SHOWN_MESSAGE_CHARS)
            assertTrue(message.count { it == '\n' } < Limits.SHOWN_MESSAGE_LINES)
        }
        assertTrue(shown.lines.size <= Limits.SHOWN_LIST_ROWS)
    }
}
