package io.github.brainage04.tunemesh.library

import java.io.File
import java.io.IOException
import java.time.Duration
import java.time.Instant
import java.time.OffsetDateTime
import java.time.format.DateTimeParseException
import java.time.temporal.ChronoUnit
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

@Serializable
data class DownloadRequest(
    val version: Int = 1,
    val id: String,
    val input: String,
    // Optional when read, as in tunemesh-core (`#[serde(default)]`); always written.
    val playlist: String? = null,
    val playlistFromSource: Boolean = false,
    val requestedBy: String = "",
    val requestedAt: String = "",
)

@Serializable
data class DownloadStatus(
    val version: Int = 1,
    val id: String,
    val input: String = "",
    val state: String,
    val title: String? = null,
    val progress: Double? = null,
    val message: String? = null,
    val tracks: List<String> = emptyList(),
    val updatedAt: String? = null,
    /** Items finished so far (downloaded, already present or failed), once [itemsTotal] is known. */
    val itemsDone: Int? = null,
    /** Items the request covers once the source is listed: 1 for a track, the entry count for a playlist or album. */
    val itemsTotal: Int? = null,
    /** Items that could not be downloaded; the request went on without them. */
    val failures: List<DownloadFailure> = emptyList(),
    /** Things to know although the request went on, for example that a source was cut to the item limit. */
    val warnings: List<String> = emptyList(),
) {
    val isFinal: Boolean
        get() = state in FINAL_STATES

    companion object {
        val FINAL_STATES = setOf("done", "failed", "cancelled")
    }
}

@Serializable
data class DownloadFailure(
    val item: String = "",
    val error: String = "",
)

/** A request as shown in the Downloads list: the request file, the status file, or both. */
data class DownloadItem(
    val id: String,
    val request: DownloadRequest?,
    val status: DownloadStatus?,
) {
    val input: String
        get() = request?.input ?: status?.input.orEmpty()

    val isFinal: Boolean
        get() = status?.isFinal == true

    val canCancel: Boolean
        get() = request != null && !isFinal

    /** Numeric `id` prefix, used for ordering. */
    val sortKey: Long
        get() = DownloadStore.sortKey(id)

    /** What the Downloads tab shows, worked out once here (off the main thread), bounded whatever the files hold. */
    val shown: ShownDownload = ShownDownload.of(input, status)
}

/**
 * The texts of one request as the Downloads tab shows them ([Limits] "Display"): single-line texts and each message, failure and warning
 * cut, at most [Limits.SHOWN_LIST_ROWS] rows; the counts cover all that was read.
 */
data class ShownDownload(
    val input: String,
    val title: String?,
    /**
     * The message as the card shows it, in at most 8 lines: [Limits.shownMessage], then cut to [PREVIEW_CHARS], since laying out all 2,000
     * characters of an unbroken message to show 8 lines takes tens of milliseconds per card on the main thread.
     */
    val message: String?,
    val failedCount: Int,
    val warningCount: Int,
    /** `item: error` per failure, then the warnings the message does not already say. */
    val lines: List<String>,
) {
    companion object {
        const val PREVIEW_CHARS = 8 * 80

        fun of(input: String, status: DownloadStatus?): ShownDownload {
            val message = status?.message?.takeIf { it.isNotBlank() }
            val failures = status?.failures.orEmpty()
            // The final message repeats the warnings; list only what it does not already say. Both are bounded
            // when read (Limits.MAX_STATUS_*), so this costs at most 50 searches of 4 KiB.
            val warnings = status?.warnings.orEmpty().filterNot { message != null && it.isNotEmpty() && message.contains(it) }
            val lines = ArrayList<String>(minOf(failures.size + warnings.size, Limits.SHOWN_LIST_ROWS))
            for (failure in failures) {
                if (lines.size == Limits.SHOWN_LIST_ROWS) break
                lines += Limits.shownMessage(listOf(failure.item, failure.error).filter { it.isNotBlank() }.joinToString(": "))
            }
            for (warning in warnings) {
                if (lines.size == Limits.SHOWN_LIST_ROWS) break
                lines += Limits.shownMessage(warning)
            }
            return ShownDownload(
                input = Limits.shownLine(input),
                title = status?.title?.let(Limits::shownLine),
                message = message?.let { Limits.cutChars(Limits.shownMessage(it), PREVIEW_CHARS) },
                failedCount = failures.size,
                warningCount = warnings.size,
                lines = lines,
            )
        }
    }
}

/** The requests read ([Limits.MAX_DOWNLOADS_READ] newest), newest first, and how many older ones were not read. */
data class DownloadList(val items: List<DownloadItem>, val older: Int) {
    companion object {
        val EMPTY = DownloadList(emptyList(), 0)
    }
}

/**
 * `.tunemesh/worker.json`: the device whose worker currently takes requests for this library. The worker also writes `instance`, `pid` and
 * `workerVersion`, which clients do not need.
 */
@Serializable
data class WorkerInfo(
    val version: Int = 1,
    val device: String,
    val heartbeatAt: String,
)

/** Who downloads requests, as clients show it. */
sealed interface Downloader {
    data class Active(val device: String) : Downloader

    data object None : Downloader

    val description: String
        get() =
            when (this) {
                is Active -> "Downloads are handled by ${Limits.shownLine(device)}"
                None -> "No device is downloading right now"
            }

    companion object {
        /** A worker counts as active while its heartbeat is less than this far from now, either way. */
        val FRESH: Duration = Duration.ofMinutes(3)

        fun of(info: WorkerInfo?, now: Instant = Instant.now()): Downloader {
            val heartbeat =
                info?.heartbeatAt?.let {
                    try {
                        OffsetDateTime.parse(it).toInstant()
                    } catch (_: DateTimeParseException) {
                        null
                    }
                } ?: return None
            val device = info.device.trim()
            if (device.isEmpty()) return None
            // Clocks of different devices drift a little, so a heartbeat slightly ahead of ours still counts.
            val age = Duration.between(heartbeat, now).abs()
            return if (age < FRESH) Active(device) else None
        }
    }
}

class DownloadStore(private val root: File) {
    private val requests
        get() = File(root, "${LibraryFormat.STATE_DIR}/requests")

    private val statuses
        get() = File(root, "${LibraryFormat.STATE_DIR}/status")

    private val workerFile
        get() = File(root, "${LibraryFormat.STATE_DIR}/worker.json")

    fun submit(
        input: String,
        playlist: String?,
        playlistFromSource: Boolean,
        requestedBy: String,
        now: Instant = Instant.now(),
    ): DownloadRequest {
        val trimmed = input.trim()
        require(trimmed.isNotEmpty()) { "Enter a link or search text" }
        val request = newRequest(trimmed, playlist?.trim()?.takeIf { it.isNotEmpty() }, playlistFromSource, requestedBy, now)
        AtomicFiles.write(File(requests, "${request.id}.json"), encode(request))
        return request
    }

    /**
     * Submits one request per line of [text] (see [DownloadInputs]). With [saveAsPlaylists], playlist and album lines get
     * `playlistFromSource`; a [playlist] target applies to every line instead. Request IDs keep paste order, so the worker downloads the
     * lines top to bottom.
     */
    fun submitAll(
        text: String,
        playlist: String?,
        saveAsPlaylists: Boolean,
        requestedBy: String,
        now: Instant = Instant.now(),
    ): List<DownloadRequest> {
        val lines = DownloadInputs.lines(text)
        require(lines.isNotEmpty()) { "Enter a link or search text" }
        return lines.mapIndexed { index, line ->
            submit(
                line,
                playlist,
                DownloadInputs.playlistFromSource(line, playlist, saveAsPlaylists),
                requestedBy,
                now.plusMillis(index.toLong()),
            )
        }
    }

    /**
     * The newest [Limits.MAX_DOWNLOADS_READ] requests by id (a request and its status file count once), each file read only when it is at
     * most [Limits.MAX_JSON_BYTES] and its texts cut to the status limits. Older ones are only counted.
     */
    fun list(): DownloadList {
        val requestIds = jsonIds(requests)
        val statusIds = jsonIds(statuses)
        val all = (requestIds + statusIds).sortedWith(compareByDescending<String> { sortKey(it) }.thenByDescending { it })
        val newest = all.take(Limits.MAX_DOWNLOADS_READ)
        val items = newest.mapNotNull { id ->
            val request = if (id in requestIds) read(File(requests, "$id.json"), ::parseRequest) else null
            val status = if (id in statusIds) read(File(statuses, "$id.json"), ::parseStatus) else null
            if (request == null && status == null) null else DownloadItem(id, request, status)
        }
        return DownloadList(items, all.size - newest.size)
    }

    /** The current `worker.json`, or null when it is missing, over [Limits.MAX_JSON_BYTES] or not valid JSON. */
    fun worker(): WorkerInfo? {
        val file = workerFile
        if (!file.isFile) return null
        return read(file, ::parseWorker)
    }

    /** Cancels a pending request by deleting its request file. */
    fun cancel(id: String) {
        File(requests, "$id.json").delete()
    }

    /** Deletes status files in a final state (of those [list] reads). */
    fun clearFinished() {
        list()
            .items
            .filter { it.isFinal }
            .forEach { item ->
                File(statuses, "${item.id}.json").delete()
            }
    }

    /** Ids of the request or status files in [directory] (names only; nothing is read). */
    private fun jsonIds(directory: File): Set<String> =
        directory
            .list()
            ?.filterTo(HashSet()) { name ->
                name.endsWith(".json") && !LibraryFormat.isHidden(name) && !LibraryFormat.isConflictCopy(name)
            }
            ?.mapTo(HashSet()) { it.removeSuffix(".json") }
            .orEmpty()

    /** [file] parsed by [parse], or null when it is over [Limits.MAX_JSON_BYTES], gone or unreadable. */
    private fun <T> read(file: File, parse: (String) -> T?): T? =
        try {
            parse(Limits.readText(file, Limits.MAX_JSON_BYTES))
        } catch (_: IOException) {
            null
        }

    companion object {
        private val json = Json { ignoreUnknownKeys = true }
        private val prettyJson = Json {
            prettyPrint = true
            prettyPrintIndent = "  "
            encodeDefaults = true
            explicitNulls = true
        }

        /**
         * A request file's text as [bounded] request, or null when it is not a request: half-synced or foreign JSON, picked up again on the
         * next refresh.
         */
        fun parseRequest(text: String): DownloadRequest? = decodeOrNull { bounded(json.decodeFromString<DownloadRequest>(text)) }

        /** A status file's text as [bounded] status, or null when it is not a status. */
        fun parseStatus(text: String): DownloadStatus? = decodeOrNull { bounded(json.decodeFromString<DownloadStatus>(text)) }

        /** `worker.json`'s text, or null when it is not valid. */
        fun parseWorker(text: String): WorkerInfo? = decodeOrNull { json.decodeFromString<WorkerInfo>(text) }

        /** kotlinx.serialization reports invalid JSON and mismatched fields as IllegalArgumentException (SerializationException). */
        private inline fun <T> decodeOrNull(decode: () -> T): T? =
            try {
                decode()
            } catch (_: IllegalArgumentException) {
                null
            }

        /** Numeric `id` prefix, used for ordering. */
        fun sortKey(id: String): Long = id.substringBefore('-').toLongOrNull() ?: 0L

        /** [request] with its texts cut to [Limits]. */
        fun bounded(request: DownloadRequest): DownloadRequest =
            request.copy(
                input = Limits.cutUtf8(request.input, Limits.MAX_REQUEST_INPUT_BYTES),
                playlist = request.playlist?.let { Limits.cutUtf8(it, Limits.MAX_REQUEST_PLAYLIST_BYTES) },
            )

        /** [status] with its texts cut and its lists shortened to [Limits]. */
        fun bounded(status: DownloadStatus): DownloadStatus {
            fun text(value: String) = Limits.cutUtf8(value, Limits.MAX_STATUS_TEXT_BYTES)
            return status.copy(
                input = Limits.cutUtf8(status.input, Limits.MAX_REQUEST_INPUT_BYTES),
                state = text(status.state),
                title = status.title?.let(::text),
                message = status.message?.let { Limits.cutUtf8(it, Limits.MAX_STATUS_MESSAGE_BYTES) },
                tracks = status.tracks.take(Limits.MAX_STATUS_TRACKS),
                failures = status.failures.take(Limits.MAX_STATUS_FAILURES).map { DownloadFailure(text(it.item), text(it.error)) },
                warnings = status.warnings.take(Limits.MAX_STATUS_WARNINGS).map(::text),
            )
        }

        fun newId(now: Instant): String = "${now.toEpochMilli()}-${AtomicFiles.randomHex(8)}"

        fun newRequest(input: String, playlist: String?, playlistFromSource: Boolean, requestedBy: String, now: Instant) =
            DownloadRequest(
                id = newId(now),
                input = input,
                playlist = playlist,
                playlistFromSource = playlistFromSource && playlist == null,
                requestedBy = requestedBy,
                requestedAt = now.truncatedTo(ChronoUnit.SECONDS).toString(),
            )

        fun encode(request: DownloadRequest): String = prettyJson.encodeToString(request) + "\n"

        /** `Alex's Pixel 8` → `alexs-pixel-8`. */
        fun deviceSlug(name: String): String =
            name.lowercase().replace("'", "").replace("\u2019", "").replace(Regex("[^a-z0-9]+"), "-").trim('-').ifEmpty { "android" }
    }
}
