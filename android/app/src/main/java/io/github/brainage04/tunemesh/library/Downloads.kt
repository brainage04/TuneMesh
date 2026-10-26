package io.github.brainage04.tunemesh.library

import java.io.File
import java.io.IOException
import java.time.Duration
import java.time.Instant
import java.time.OffsetDateTime
import java.time.format.DateTimeParseException
import java.time.temporal.ChronoUnit
import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationException
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.encoding.CompositeDecoder
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.Json
import kotlinx.serialization.serializer

/** A request file, as this app writes it and (cut by [DownloadStore.bounded]) as it reads one. */
@Serializable
data class DownloadRequest(
    val version: Long = 1,
    val id: String,
    val input: String,
    // Optional when read, as in tunemesh-core (`#[serde(default)]`); always written.
    val playlist: String? = null,
    val playlistFromSource: Boolean = false,
    val requestedBy: String = "",
    val requestedAt: String = "",
)

/**
 * A status file as read (cut by [DownloadStore.bounded]); only the worker writes them. Of `tracks` only the number is kept: the reader
 * counts the paths without keeping them ([CountSerializer]). `updatedAt` is not read. A [state] other than [STATES] or an integer outside
 * 0..[DownloadStore.MAX_U32] makes the file invalid (contract, "Limits").
 */
@Serializable
data class DownloadStatus(
    val version: Long = 1,
    val id: String,
    val input: String = "",
    val state: String,
    val title: String? = null,
    val progress: Double? = null,
    val message: String? = null,
    /** Paths in `tracks`, at most [Limits.MAX_STATUS_TRACKS]. */
    @SerialName("tracks") @Serializable(with = CountSerializer::class) val trackCount: Int = 0,
    /** Items finished so far (downloaded, already present or failed), once [itemsTotal] is known. */
    val itemsDone: Long? = null,
    /** Items the request covers once the source is listed: 1 for a track, the entry count for a playlist or album. */
    val itemsTotal: Long? = null,
    /** Items that could not be downloaded; the request went on without them. At most [Limits.MAX_STATUS_FAILURES] are read. */
    @Serializable(with = FailuresSerializer::class) val failures: List<DownloadFailure> = emptyList(),
    /**
     * Things to know although the request went on, for example that a source was cut to the item limit. At most
     * [Limits.MAX_STATUS_WARNINGS].
     */
    @Serializable(with = WarningsSerializer::class) val warnings: List<String> = emptyList(),
) {
    companion object {
        val STATES = setOf("queued", "downloading", "done", "failed", "cancelled")
        val FINAL_STATES = setOf("done", "failed", "cancelled")
    }
}

@Serializable
data class DownloadFailure(
    val item: String = "",
    val error: String = "",
)

/**
 * Reads a JSON list of strings as its length, at most [Limits.MAX_STATUS_TRACKS]: each element is decoded and dropped, so a status with
 * 10,000 paths costs one path at a time. Read only; status files are written by the worker.
 */
object CountSerializer : KSerializer<Int> {
    private val list = ListSerializer(String.serializer())
    override val descriptor: SerialDescriptor = list.descriptor

    override fun deserialize(decoder: Decoder): Int {
        var count = 0
        val structure = decoder.beginStructure(descriptor)
        while (true) {
            val index = structure.decodeElementIndex(descriptor)
            if (index == CompositeDecoder.DECODE_DONE) break
            structure.decodeStringElement(descriptor, index)
            if (count < Limits.MAX_STATUS_TRACKS) count++
        }
        structure.endStructure(descriptor)
        return count
    }

    override fun serialize(encoder: Encoder, value: Int): Unit = throw SerializationException("a status track count is read only")
}

/**
 * Reads a JSON list keeping its first [max] elements; the others are decoded and dropped, so a status file of small entries never holds
 * more than [max] of them. Writes a plain list.
 */
open class CappedListSerializer<T>(private val element: KSerializer<T>, private val max: Int) : KSerializer<List<T>> {
    private val list = ListSerializer(element)
    override val descriptor: SerialDescriptor = list.descriptor

    override fun deserialize(decoder: Decoder): List<T> {
        val kept = ArrayList<T>()
        val structure = decoder.beginStructure(descriptor)
        while (true) {
            val index = structure.decodeElementIndex(descriptor)
            if (index == CompositeDecoder.DECODE_DONE) break
            val value = structure.decodeSerializableElement(descriptor, index, element)
            if (kept.size < max) kept += value
        }
        structure.endStructure(descriptor)
        return kept
    }

    override fun serialize(encoder: Encoder, value: List<T>) = list.serialize(encoder, value)
}

object FailuresSerializer : CappedListSerializer<DownloadFailure>(serializer(), Limits.MAX_STATUS_FAILURES)

object WarningsSerializer : CappedListSerializer<String>(String.serializer(), Limits.MAX_STATUS_WARNINGS)

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
 * `.tunemesh/worker.json`: the device whose worker currently takes requests for this library. The worker also writes `instance`, `pid` and
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
     * counted.
     */
    fun list(): DownloadList {
        val requestIds = jsonIds(requests)
        val statusIds = jsonIds(statuses)
        val all = (requestIds + statusIds).sortedWith(compareByDescending<String> { sortKey(it) }.thenByDescending { it })
        val newest = all.take(Limits.MAX_DOWNLOADS_READ)
        val items = newest.mapNotNull { id ->
            val request = if (id in requestIds) read(File(requests, "$id.json"), ::parseRequest) else null
            val status = if (id in statusIds) read(File(statuses, "$id.json"), ::parseStatus) else null
            if (request == null && status == null) null else DownloadItem.of(id, request, status)
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

    /**
     * Deletes every status file in a final state whose request file is gone (contract, "Download status"), not only those [list] shows: one
     * file at a time, keeping only its `state`.
     */
    fun clearFinished() {
        val requestIds = jsonIds(requests)
        for (id in jsonIds(statuses)) {
            if (id in requestIds) continue
            val file = File(statuses, "$id.json")
            if (read(file, ::parseState) in DownloadStatus.FINAL_STATES) file.delete()
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

    @Suppress("TooManyFunctions") // one parser and one bounded projection per synced format, kept together
    companion object {
        private const val KIB = 1024

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
        fun parseRequest(text: String): DownloadRequest? = decodeOrNull {
            json.decodeFromString<DownloadRequest>(text).takeIf { it.version in 0..MAX_U32 }?.let(::bounded)
        }

        /** A status file's text as [bounded] status, or null when it is not a valid status. */
        fun parseStatus(text: String): DownloadStatus? = decodeOrNull {
            json.decodeFromString<DownloadStatus>(text).takeIf(::isValid)?.let(::bounded)
        }

        private fun isValid(status: DownloadStatus): Boolean =
            status.state in DownloadStatus.STATES &&
                status.version in 0..MAX_U32 &&
                (status.itemsDone ?: 0) in 0..MAX_U32 &&
                (status.itemsTotal ?: 0) in 0..MAX_U32

        /** Only the `state` of a status file (the other fields are skipped, not kept), or null when it is not valid. */
        private fun parseState(text: String): String? = decodeOrNull { json.decodeFromString<StatusState>(text).state }

        @Serializable private data class StatusState(val state: String)

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

        /** kotlinx.serialization reports invalid JSON and mismatched fields as IllegalArgumentException (SerializationException). */
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
