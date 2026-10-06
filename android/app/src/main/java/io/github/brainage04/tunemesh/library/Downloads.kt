package io.github.brainage04.tunemesh.library

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.io.File
import java.io.IOException
import java.time.Instant
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

class DownloadStore(private val root: File) {
    private val requests get() = File(root, "${LibraryFormat.STATE_DIR}/requests")
    private val statuses get() = File(root, "${LibraryFormat.STATE_DIR}/status")

    fun submit(input: String, playlist: String?, playlistFromSource: Boolean, requestedBy: String, now: Instant = Instant.now()): DownloadRequest {
        val trimmed = input.trim()
        require(trimmed.isNotEmpty()) { "Enter a link or search text" }
        val request = newRequest(trimmed, playlist?.trim()?.takeIf { it.isNotEmpty() }, playlistFromSource, requestedBy, now)
        AtomicFiles.write(File(requests, "${request.id}.json"), encode(request))
        return request
    }

    fun list(): List<DownloadItem> {
        val requestMap = readAll(requests) { json.decodeFromString<DownloadRequest>(it) }
        val statusMap = readAll(statuses) { json.decodeFromString<DownloadStatus>(it) }
        return (requestMap.keys + statusMap.keys)
            .map { DownloadItem(it, requestMap[it], statusMap[it]) }
            .sortedByDescending { it.sortKey }
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

        /** `Thomas's S21` → `thomass-s21`. */
        fun deviceSlug(name: String): String =
            name.lowercase().replace("'", "").replace("\u2019", "")
                .replace(Regex("[^a-z0-9]+"), "-").trim('-').ifEmpty { "android" }
    }
}
