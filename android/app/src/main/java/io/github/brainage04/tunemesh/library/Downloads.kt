package io.github.brainage04.tunemesh.library

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.io.File
import java.io.IOException
import java.time.Duration
import java.time.Instant
import java.time.format.DateTimeParseException
import java.time.temporal.ChronoUnit

@Serializable
data class DownloadRequest(
    val version: Int = 1,
    val id: String,
    val input: String,
    val playlist: String?,
    val playlistFromSource: Boolean,
    val requestedBy: String,
    val requestedAt: String,
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
    /** Items of the request finished so far (downloaded, matched or skipped), once [total] is known. */
    val done: Int? = null,
    /** Items the request resolved to: 1 for a single track, the entry count for a playlist or album. */
    val total: Int? = null,
    /** Per-item problems that did not fail the whole request, for example an unavailable playlist entry. */
    val warnings: List<String> = emptyList(),
) {
    val isFinal: Boolean get() = state in FINAL_STATES

    companion object {
        val FINAL_STATES = setOf("done", "failed", "cancelled")
    }
}

/** A request as shown in the Downloads list: the request file, the status file, or both. */
data class DownloadItem(
    val id: String,
    val request: DownloadRequest?,
    val status: DownloadStatus?,
) {
    val input: String get() = request?.input ?: status?.input.orEmpty()
    val isFinal: Boolean get() = status?.isFinal == true
    val canCancel: Boolean get() = request != null && !isFinal

    /** Numeric `id` prefix, used for ordering. */
    val sortKey: Long get() = id.substringBefore('-').toLongOrNull() ?: 0L
}

/** `.tunemesh/worker.json`: the device whose worker currently takes requests for this library. */
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
        get() = when (this) {
            is Active -> "Downloads are handled by $device"
            None -> "No device is downloading right now"
        }

    companion object {
        /** A worker counts as active while its heartbeat is at most this old. */
        val FRESH: Duration = Duration.ofMinutes(3)

        fun of(info: WorkerInfo?, now: Instant = Instant.now()): Downloader {
            val heartbeat = info?.heartbeatAt?.let {
                try {
                    Instant.parse(it)
                } catch (_: DateTimeParseException) {
                    null
                }
            } ?: return None
            val device = info.device.trim()
            if (device.isEmpty()) return None
            // A clock a little ahead of ours still counts; a heartbeat far in the future is not trusted forever.
            val age = Duration.between(heartbeat, now)
            return if (age <= FRESH && age >= FRESH.negated()) Active(device) else None
        }
    }
}

class DownloadStore(private val root: File) {
    private val requests get() = File(root, "${LibraryFormat.STATE_DIR}/requests")
    private val statuses get() = File(root, "${LibraryFormat.STATE_DIR}/status")
    private val workerFile get() = File(root, "${LibraryFormat.STATE_DIR}/worker.json")

    fun submit(input: String, playlist: String?, playlistFromSource: Boolean, requestedBy: String, now: Instant = Instant.now()): DownloadRequest {
        val trimmed = input.trim()
        require(trimmed.isNotEmpty()) { "Enter a link or search text" }
        val request = newRequest(trimmed, playlist?.trim()?.takeIf { it.isNotEmpty() }, playlistFromSource, requestedBy, now)
        AtomicFiles.write(File(requests, "${request.id}.json"), encode(request))
        return request
    }

    /**
     * Submits one request per line of [text] (see [DownloadInputs]). With [saveAsPlaylists], playlist and album
     * lines get `playlistFromSource`; a [playlist] target applies to every line instead. Request IDs keep paste
     * order, so the worker downloads the lines top to bottom.
     */
    fun submitAll(text: String, playlist: String?, saveAsPlaylists: Boolean, requestedBy: String, now: Instant = Instant.now()): List<DownloadRequest> {
        val lines = DownloadInputs.lines(text)
        require(lines.isNotEmpty()) { "Enter a link or search text" }
        return lines.mapIndexed { index, line ->
            submit(line, playlist, DownloadInputs.playlistFromSource(line, playlist, saveAsPlaylists), requestedBy, now.plusMillis(index.toLong()))
        }
    }

    fun list(): List<DownloadItem> {
        val requestMap = readAll(requests) { json.decodeFromString<DownloadRequest>(it) }
        val statusMap = readAll(statuses) { json.decodeFromString<DownloadStatus>(it) }
        return (requestMap.keys + statusMap.keys)
            .map { DownloadItem(it, requestMap[it], statusMap[it]) }
            .sortedByDescending { it.sortKey }
    }

    /** The current `worker.json`, or null when it is missing or not valid JSON. */
    fun worker(): WorkerInfo? {
        val file = workerFile
        if (!file.isFile) return null
        return try {
            json.decodeFromString<WorkerInfo>(file.readText(Charsets.UTF_8))
        } catch (_: IOException) {
            null
        } catch (_: IllegalArgumentException) {
            null
        }
    }

    /** Cancels a pending request by deleting its request file. */
    fun cancel(id: String) {
        File(requests, "$id.json").delete()
    }

    /** Deletes status files in a final state. */
    fun clearFinished() {
        list().filter { it.isFinal }.forEach { item ->
            File(statuses, "${item.id}.json").delete()
        }
    }

    private fun <T> readAll(directory: File, decode: (String) -> T): Map<String, T> {
        val files = directory.listFiles { file ->
            file.isFile && file.name.endsWith(".json") && !LibraryFormat.isHidden(file.name) &&
                !LibraryFormat.isConflictCopy(file.name)
        } ?: return emptyMap()
        val result = HashMap<String, T>()
        for (file in files) {
            try {
                result[file.name.removeSuffix(".json")] = decode(file.readText(Charsets.UTF_8))
            } catch (_: IOException) {
            } catch (_: IllegalArgumentException) {
                // Half-synced or foreign JSON; it is picked up again on the next refresh.
            }
        }
        return result
    }

    companion object {
        private val json = Json { ignoreUnknownKeys = true }
        private val prettyJson = Json {
            prettyPrint = true
            prettyPrintIndent = "  "
            encodeDefaults = true
            explicitNulls = true
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

        fun encode(request: DownloadRequest): String = prettyJson.encodeToString(DownloadRequest.serializer(), request) + "\n"

        /** `Alex's Pixel 8` → `alexs-pixel-8`. */
        fun deviceSlug(name: String): String =
            name.lowercase().replace("'", "").replace("\u2019", "")
                .replace(Regex("[^a-z0-9]+"), "-").trim('-').ifEmpty { "android" }
    }
}
