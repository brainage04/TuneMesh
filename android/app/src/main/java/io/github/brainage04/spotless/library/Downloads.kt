package io.github.brainage04.spotless.library

import java.io.File
import java.io.IOException
import java.io.InputStream
import java.nio.file.Files
import java.nio.file.attribute.BasicFileAttributes
import java.nio.file.attribute.FileTime
import java.time.Duration
import java.time.Instant
import java.time.OffsetDateTime
import java.time.format.DateTimeParseException
import java.time.temporal.ChronoUnit
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/** A request file, as this app writes it and (cut by [DownloadStore.bounded]) as it reads one. */
@Serializable
data class DownloadRequest(
    val version: Long = 1,
    val id: String,
    val input: String,
    // Optional when read, as in spotless-core (`#[serde(default)]`); always written.
    val playlist: String? = null,
    val playlistFromSource: Boolean = false,
    val requestedBy: String = "",
    val requestedAt: String = "",
)

/**
 * A status file as read ([DownloadStore.parseStatus], cut by [DownloadStore.bounded]); only the worker writes them. Of `tracks` only the
 * number is kept: the reader counts the paths without decoding them. `updatedAt` is not read. A [state] other than [STATES] or an integer
 * outside 0..[DownloadStore.MAX_U32] makes the file invalid (contract, "Limits").
 */
data class DownloadStatus(
    val version: Long = 1,
    val id: String,
    val input: String = "",
    val state: String,
    val title: String? = null,
    val progress: Double? = null,
    val message: String? = null,
    /** Paths in `tracks`, at most [Limits.MAX_STATUS_TRACKS]. */
    val trackCount: Int = 0,
    /** Items finished so far (downloaded, already present or failed), once [itemsTotal] is known. */
    val itemsDone: Long? = null,
    /** Items the request covers once the source is listed: 1 for a track, the entry count for a playlist or album. */
    val itemsTotal: Long? = null,
    /** Items that could not be downloaded; the request went on without them. At most [Limits.MAX_STATUS_FAILURES] are read. */
    val failures: List<DownloadFailure> = emptyList(),
    /**
     * Things to know although the request went on, for example that a source was cut to the item limit. At most
     * [Limits.MAX_STATUS_WARNINGS].
     */
    val warnings: List<String> = emptyList(),
) {
    companion object {
        val STATES = setOf("queued", "downloading", "done", "failed", "cancelled")
        val FINAL_STATES = setOf("done", "failed", "cancelled")
    }
}

data class DownloadFailure(
    val item: String = "",
    val error: String = "",
)

/**
 * A request as the Downloads list keeps it: the request file, the status file, or both, reduced to what the card shows. Neither file is
 * kept, so 200 requests cost at most a few megabytes whatever their files hold (see `DownloadsMemoryTest`).
 */
data class DownloadItem(
    /** From the file name (the JSON `id` is not used). */
    val id: String,
    val hasRequest: Boolean,
    /** The status `state` (cut to [Limits.MAX_STATUS_TEXT_BYTES]), or null while there is no status file. */
    val state: String?,
    val progress: Double?,
    val itemsDone: Long?,
    val itemsTotal: Long?,
    val trackCount: Int,
    /** The request's target playlist, as shown ([Limits.shownLine]). */
    val playlist: String?,
    val playlistFromSource: Boolean,
    /** What the Downloads tab shows, worked out once here (off the main thread), bounded whatever the files hold. */
    val shown: ShownDownload,
) {
    val isFinal: Boolean
        get() = state in DownloadStatus.FINAL_STATES

    val canCancel: Boolean
        get() = hasRequest && !isFinal

    /** Numeric `id` prefix, used for ordering. */
    val sortKey: Long
        get() = DownloadStore.sortKey(id)

    companion object {
        /** The item for [id] from its [bounded][DownloadStore.bounded] request and status; keeps none of their texts beyond [shown]. */
        fun of(id: String, request: DownloadRequest?, status: DownloadStatus?): DownloadItem =
            DownloadItem(
                id = id,
                hasRequest = request != null,
                state = status?.state,
                progress = status?.progress,
                itemsDone = status?.itemsDone,
                itemsTotal = status?.itemsTotal,
                trackCount = status?.trackCount ?: 0,
                playlist = request?.playlist?.let(Limits::shownLine),
                playlistFromSource = request?.playlistFromSource == true,
                shown = ShownDownload.of(request?.input ?: status?.input.orEmpty(), status),
            )
    }
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
    /**
     * `item: error` per failure, then the warnings the message does not already say; each as its row shows it, in at most 4 lines:
     * [Limits.shownMessage], then cut to [LINE_PREVIEW_CHARS] (200 rows of 2,000 characters would keep 400 KB per request).
     */
    val lines: List<String>,
) {
    companion object {
        const val PREVIEW_CHARS = 8 * 80
        const val LINE_PREVIEW_CHARS = 4 * 80

        fun of(input: String, status: DownloadStatus?): ShownDownload {
            val message = status?.message?.takeIf { it.isNotBlank() }
            val failures = status?.failures.orEmpty()
            // The final message repeats the warnings; list only what it does not already say. Both are bounded
            // when read (Limits.MAX_STATUS_*), so this costs at most 50 searches of 4 KiB.
            val warnings = status?.warnings.orEmpty().filterNot { message != null && it.isNotEmpty() && message.contains(it) }
            val lines = ArrayList<String>(minOf(failures.size + warnings.size, Limits.SHOWN_LIST_ROWS))
            fun row(text: String) = Limits.cutChars(Limits.shownMessage(text), LINE_PREVIEW_CHARS)
            for (failure in failures) {
                if (lines.size == Limits.SHOWN_LIST_ROWS) break
                lines += row(listOf(failure.item, failure.error).filter { it.isNotBlank() }.joinToString(": "))
            }
            for (warning in warnings) {
                if (lines.size == Limits.SHOWN_LIST_ROWS) break
                lines += row(warning)
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
 * `.spotless/worker.json`: the device whose worker currently takes requests for this library. The worker also writes `instance`, `pid` and
 * `workerVersion`, which clients do not need.
 */
@Serializable
data class WorkerInfo(
    val version: Long = 1,
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

/**
 * The request and status files of the library at [root]. One store per library: it keeps the [DownloadItem] it made of each listed request
 * with the identity, modification time and size of the files it came from, and reads the files again only when one of those changed.
 */
class DownloadStore(val root: File) {
    /** A file's identity, modification time and size: a file replaced (as every writer does, atomically) or rewritten differs. */
    private data class Stamp(val key: Any?, val modified: FileTime, val size: Long)

    private class Listed(val request: Stamp?, val status: Stamp?, val item: DownloadItem?)

    /** What [list] made of each request it listed last, by id. */
    private var listed: Map<String, Listed> = emptyMap()

    private var worker: Pair<Stamp, WorkerInfo?>? = null

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
        val trimmed = checkedInput(input)
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
        // Every line is checked before the first request is written, so a refused paste writes nothing.
        lines.forEach(::checkedInput)
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
     * most [Limits.MAX_JSON_BYTES], cut to [Limits] and reduced to a [DownloadItem] before the next one is read. Older ones are only
     * counted. A request whose files did not change since the last call is not read again.
     */
    @Synchronized
    fun list(): DownloadList {
        val requestIds = jsonIds(requests)
        val statusIds = jsonIds(statuses)
        val all = (requestIds + statusIds).sortedWith(compareByDescending<String> { sortKey(it) }.thenByDescending { it })
        val newest = all.take(Limits.MAX_DOWNLOADS_READ)
        val previous = listed
        val current = HashMap<String, Listed>(newest.size * 2)
        val items = newest.mapNotNull { id ->
            val requestFile = File(requests, "$id.json").takeIf { id in requestIds }
            val statusFile = File(statuses, "$id.json").takeIf { id in statusIds }
            val requestStamp = requestFile?.let(::stamp)
            val statusStamp = statusFile?.let(::stamp)
            val known = previous[id]?.takeIf { it.request == requestStamp && it.status == statusStamp }
            val entry =
                known
                    ?: run {
                        val request = requestFile?.let { read(it, requestStamp, ::readRequest) }
                        val status = statusFile?.let { read(it, statusStamp, ::readStatus) }
                        val item = if (request == null && status == null) null else DownloadItem.of(id, request, status)
                        Listed(requestStamp, statusStamp, item)
                    }
            current[id] = entry
            entry.item
        }
        listed = current
        return DownloadList(items, all.size - newest.size)
    }

    /**
     * The current `worker.json`, or null when it is missing, over [Limits.MAX_JSON_BYTES] or not valid JSON; read again only when the file
     * changed.
     */
    @Synchronized
    fun worker(): WorkerInfo? {
        val file = workerFile
        val stamp = stamp(file) ?: return null
        if (worker?.first != stamp) {
            val info =
                try {
                    parseWorker(Limits.readText(file, Limits.MAX_JSON_BYTES))
                } catch (_: IOException) {
                    null
                }
            worker = stamp to info
        }
        return worker?.second
    }

    /** Cancels a pending request by deleting its request file. */
    fun cancel(id: String) {
        File(requests, "$id.json").delete()
    }

    /**
     * Deletes every status file in a final state whose request file is gone (contract, "Download status"), not only those [list] shows: one
     * file at a time, keeping only its `state`.
     */
    fun clearFinished() {
        val requestIds = jsonIds(requests)
        for (id in jsonIds(statuses)) {
            if (id in requestIds) continue
            val file = File(statuses, "$id.json")
            if (read(file, stamp(file), ::readState) in DownloadStatus.FINAL_STATES) file.delete()
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

    /** [file]'s [Stamp], or null when it is not a regular file (gone, say). */
    private fun stamp(file: File): Stamp? =
        try {
            val attributes = Files.readAttributes(file.toPath(), BasicFileAttributes::class.java)
            if (attributes.isRegularFile) Stamp(attributes.fileKey(), attributes.lastModifiedTime(), attributes.size()) else null
        } catch (_: IOException) {
            null
        }

    /**
     * [file] streamed through [parse], or null when it is not a regular file ([stamp] null), over [Limits.MAX_JSON_BYTES] (also when it
     * grows while read), gone or unreadable.
     */
    private fun <T> read(file: File, stamp: Stamp?, parse: (InputStream, Long) -> T?): T? {
        if (stamp == null || stamp.size > Limits.MAX_JSON_BYTES) return null
        return try {
            file.inputStream().use { parse(it, Limits.MAX_JSON_BYTES.toLong()) }
        } catch (_: IOException) {
            null
        }
    }

    @Suppress("TooManyFunctions") // one parser and one bounded projection per synced format, kept together
    companion object {
        private const val KIB = 1024

        /** More than the longest [DownloadStatus.STATES]: a longer `state` is not one of them. */
        private const val STATE_BYTES = 16

        /** The largest integer a request, status or `worker.json` file may hold (contract, "Limits"). */
        const val MAX_U32 = (1L shl 32) - 1

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
        fun parseRequest(text: String): DownloadRequest? = readRequest(text.byteInputStream(), Long.MAX_VALUE)

        /** A status file's text as [bounded] status, or null when it is not a valid status. */
        fun parseStatus(text: String): DownloadStatus? = readStatus(text.byteInputStream(), Long.MAX_VALUE)

        /** [parseRequest] of a stream of at most [maxBytes]; each text is decoded only up to its limit. */
        private fun readRequest(input: InputStream, maxBytes: Long): DownloadRequest? = decodeOrNull {
            val reader = JsonReader(input, maxBytes)
            var version = 1L
            var id: String? = null
            var text: String? = null
            var playlist: String? = null
            var playlistFromSource = false
            var requestedBy = ""
            var requestedAt = ""
            reader.forEachField { name ->
                when (name) {
                    "version" -> version = reader.long()
                    "id" -> id = reader.string(Limits.MAX_DOWNLOAD_ID_BYTES)
                    "input" -> text = reader.string(Limits.MAX_REQUEST_INPUT_BYTES)
                    "playlist" -> playlist = if (reader.nextIsNull()) null else reader.string(Limits.MAX_REQUEST_PLAYLIST_BYTES)
                    "playlistFromSource" -> playlistFromSource = reader.boolean()
                    "requestedBy" -> requestedBy = reader.string(Limits.MAX_DEVICE_NAME_BYTES)
                    "requestedAt" -> requestedAt = reader.string(Limits.MAX_TIMESTAMP_BYTES)
                    else -> reader.skipValue()
                }
            }
            reader.end()
            DownloadRequest(version, required(id, "id"), required(text, "input"), playlist, playlistFromSource, requestedBy, requestedAt)
                .takeIf { it.version in 0..MAX_U32 }
                ?.let(::bounded)
        }

        /**
         * [parseStatus] of a stream of at most [maxBytes]: each text is decoded only up to its limit, the failures and warnings past their
         * limits and every track path are checked and skipped, never decoded.
         */
        @Suppress("CyclomaticComplexMethod", "LongMethod") // one branch per field of the format
        private fun readStatus(input: InputStream, maxBytes: Long): DownloadStatus? = decodeOrNull {
            val reader = JsonReader(input, maxBytes)
            var version = 1L
            var id: String? = null
            var text = ""
            var state: String? = null
            var title: String? = null
            var progress: Double? = null
            var message: String? = null
            var trackCount = 0
            var itemsDone: Long? = null
            var itemsTotal: Long? = null
            val failures = ArrayList<DownloadFailure>()
            val warnings = ArrayList<String>()
            reader.forEachField { name ->
                when (name) {
                    "version" -> version = reader.long()
                    "id" -> id = reader.string(Limits.MAX_DOWNLOAD_ID_BYTES)
                    "input" -> text = reader.string(Limits.MAX_REQUEST_INPUT_BYTES)
                    "state" -> state = reader.string(STATE_BYTES).takeUnless { reader.wasCut } ?: throw JsonReader.Malformed("not a state")
                    "title" -> title = if (reader.nextIsNull()) null else reader.string(Limits.MAX_STATUS_TEXT_BYTES)
                    "progress" -> progress = if (reader.nextIsNull()) null else reader.double()
                    "message" -> message = if (reader.nextIsNull()) null else reader.string(Limits.MAX_STATUS_MESSAGE_BYTES)
                    "tracks" -> {
                        trackCount = 0
                        reader.forEachElement {
                            reader.skipString()
                            if (trackCount < Limits.MAX_STATUS_TRACKS) trackCount++
                        }
                    }
                    "itemsDone" -> itemsDone = if (reader.nextIsNull()) null else reader.long()
                    "itemsTotal" -> itemsTotal = if (reader.nextIsNull()) null else reader.long()
                    "failures" -> {
                        failures.clear()
                        reader.forEachElement {
                            readFailure(reader, keep = failures.size < Limits.MAX_STATUS_FAILURES)?.let(failures::add)
                        }
                    }
                    "warnings" -> {
                        warnings.clear()
                        reader.forEachElement {
                            if (warnings.size < Limits.MAX_STATUS_WARNINGS) warnings += reader.string(Limits.MAX_STATUS_TEXT_BYTES)
                            else reader.skipString()
                        }
                    }
                    else -> reader.skipValue()
                }
            }
            reader.end()
            DownloadStatus(
                    version = version,
                    id = required(id, "id"),
                    input = text,
                    state = required(state, "state"),
                    title = title,
                    progress = progress,
                    message = message,
                    trackCount = trackCount,
                    itemsDone = itemsDone,
                    itemsTotal = itemsTotal,
                    failures = failures,
                    warnings = warnings,
                )
                .takeIf(::isValid)
                ?.let(::bounded)
        }

        /** One failure object; null (only checked) unless [keep]. */
        private fun readFailure(reader: JsonReader, keep: Boolean): DownloadFailure? {
            var item = ""
            var error = ""
            reader.forEachField { name ->
                when {
                    name == "item" && keep -> item = reader.string(Limits.MAX_STATUS_TEXT_BYTES)
                    name == "error" && keep -> error = reader.string(Limits.MAX_STATUS_TEXT_BYTES)
                    name == "item" || name == "error" -> reader.skipString()
                    else -> reader.skipValue()
                }
            }
            return if (keep) DownloadFailure(item, error) else null
        }

        private fun <T> required(value: T?, name: String): T = value ?: throw JsonReader.Malformed("no $name")

        private fun isValid(status: DownloadStatus): Boolean =
            status.state in DownloadStatus.STATES &&
                status.version in 0..MAX_U32 &&
                (status.itemsDone ?: 0) in 0..MAX_U32 &&
                (status.itemsTotal ?: 0) in 0..MAX_U32

        /** Only the `state` of a status file (the other fields are checked and skipped), or null when it is not valid JSON. */
        private fun readState(input: InputStream, maxBytes: Long): String? = decodeOrNull {
            val reader = JsonReader(input, maxBytes)
            var state: String? = null
            reader.forEachField { name -> if (name == "state") state = reader.string(STATE_BYTES) else reader.skipValue() }
            reader.end()
            state
        }

        /** `worker.json`'s text, or null when it is not valid. */
        fun parseWorker(text: String): WorkerInfo? = decodeOrNull {
            json.decodeFromString<WorkerInfo>(text).takeIf { it.version in 0..MAX_U32 }?.let(::bounded)
        }

        /**
         * [input] trimmed, as a request may hold it: a client never writes one over [Limits.MAX_REQUEST_INPUT_BYTES] (contract, "Download
         * requests") and says so instead.
         */
        fun checkedInput(input: String): String {
            val trimmed = input.trim()
            require(trimmed.isNotEmpty()) { "Enter a link or search text" }
            require(Limits.utf8Length(trimmed) <= Limits.MAX_REQUEST_INPUT_BYTES) {
                "Too long to request: at most ${Limits.MAX_REQUEST_INPUT_BYTES / KIB} KiB per line"
            }
            return trimmed
        }

        /**
         * kotlinx.serialization reports invalid JSON and mismatched fields as IllegalArgumentException (SerializationException), and so
         * does [JsonReader] ([JsonReader.Malformed]).
         */
        private inline fun <T> decodeOrNull(decode: () -> T): T? =
            try {
                decode()
            } catch (_: IllegalArgumentException) {
                null
            }

        /** Numeric `id` prefix, used for ordering. */
        fun sortKey(id: String): Long = id.substringBefore('-').toLongOrNull() ?: 0L

        /** [request] with every text cut to [Limits]. */
        fun bounded(request: DownloadRequest): DownloadRequest =
            request.copy(
                id = Limits.cutUtf8(request.id, Limits.MAX_DOWNLOAD_ID_BYTES),
                input = Limits.cutUtf8(request.input, Limits.MAX_REQUEST_INPUT_BYTES),
                playlist = request.playlist?.let { Limits.cutUtf8(it, Limits.MAX_REQUEST_PLAYLIST_BYTES) },
                requestedBy = Limits.cutUtf8(request.requestedBy, Limits.MAX_DEVICE_NAME_BYTES),
                requestedAt = Limits.cutUtf8(request.requestedAt, Limits.MAX_TIMESTAMP_BYTES),
            )

        /** [status] with every text cut and every list shortened to [Limits]. */
        fun bounded(status: DownloadStatus): DownloadStatus {
            fun text(value: String) = Limits.cutUtf8(value, Limits.MAX_STATUS_TEXT_BYTES)
            return status.copy(
                id = Limits.cutUtf8(status.id, Limits.MAX_DOWNLOAD_ID_BYTES),
                input = Limits.cutUtf8(status.input, Limits.MAX_REQUEST_INPUT_BYTES),
                state = text(status.state),
                title = status.title?.let(::text),
                message = status.message?.let { Limits.cutUtf8(it, Limits.MAX_STATUS_MESSAGE_BYTES) },
                trackCount = status.trackCount.coerceIn(0, Limits.MAX_STATUS_TRACKS),
                failures = status.failures.take(Limits.MAX_STATUS_FAILURES).map { DownloadFailure(text(it.item), text(it.error)) },
                warnings = status.warnings.take(Limits.MAX_STATUS_WARNINGS).map(::text),
            )
        }

        /** [info] with its texts cut to [Limits]. */
        fun bounded(info: WorkerInfo): WorkerInfo =
            info.copy(
                device = Limits.cutUtf8(info.device, Limits.MAX_DEVICE_NAME_BYTES),
                heartbeatAt = Limits.cutUtf8(info.heartbeatAt, Limits.MAX_TIMESTAMP_BYTES),
            )

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
