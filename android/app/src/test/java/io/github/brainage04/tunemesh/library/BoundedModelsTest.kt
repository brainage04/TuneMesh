package io.github.brainage04.tunemesh.library

import java.io.File
import java.lang.reflect.Modifier
import java.time.Instant
import java.util.jar.JarFile
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.serializer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Every property of every model read from the synced folder, and of what the app keeps of it, has a limit here (contract: "Limits"). A new
 * property, or a new `@Serializable` model in the library package, fails [everyPropertyHasALimit] until it gets one;
 * [boundedReadsCutEveryField] then builds files with every field past its limit and checks, property by property, that what a reader keeps
 * is within it.
 */
class BoundedModelsTest {
    private sealed interface Limit

    /** At most [max] UTF-8 bytes. */
    private data class Bytes(val max: Int) : Limit

    /** At most [max] characters (display limits). */
    private data class Chars(val max: Int) : Limit

    /** One of [values]; anything else makes the file invalid. */
    private data class OneOf(val values: Set<String>) : Limit

    /** A list of at most [max] entries, each within [each]. */
    private data class Rows(val max: Int, val each: Limit) : Limit

    /** A number counting at most [max] entries of a list (status `tracks`). */
    private data class Count(val max: Int) : Limit

    /** A number or boolean: fixed size. */
    private data object Scalar : Limit

    /** An object whose own properties are listed in [limits]. */
    private data object Nested : Limit

    /** Bounded elsewhere, as [why] says (checked by MemoryBudgetTest). */
    private data class Elsewhere(val why: String) : Limit

    private val text = Bytes(Limits.MAX_STATUS_TEXT_BYTES)
    private val line = Chars(Limits.SHOWN_LINE_CHARS)
    private val path = Bytes(Limits.MAX_PLAYLIST_LINE_BYTES)
    private val tag = Bytes(Limits.MAX_TAG_TEXT_BYTES)

    /** Per class, per property (the Kotlin property, which is the JVM field). */
    private val limits: Map<Class<*>, Map<String, Limit>> =
        mapOf(
            DownloadRequest::class.java to
                mapOf(
                    "version" to Scalar,
                    "id" to Bytes(Limits.MAX_DOWNLOAD_ID_BYTES),
                    "input" to Bytes(Limits.MAX_REQUEST_INPUT_BYTES),
                    "playlist" to Bytes(Limits.MAX_REQUEST_PLAYLIST_BYTES),
                    "playlistFromSource" to Scalar,
                    "requestedBy" to Bytes(Limits.MAX_DEVICE_NAME_BYTES),
                    "requestedAt" to Bytes(Limits.MAX_TIMESTAMP_BYTES),
                ),
            DownloadStatus::class.java to
                mapOf(
                    "version" to Scalar,
                    "id" to Bytes(Limits.MAX_DOWNLOAD_ID_BYTES),
                    "input" to Bytes(Limits.MAX_REQUEST_INPUT_BYTES),
                    "state" to OneOf(DownloadStatus.STATES),
                    "title" to text,
                    "progress" to Scalar,
                    "message" to Bytes(Limits.MAX_STATUS_MESSAGE_BYTES),
                    "trackCount" to Count(Limits.MAX_STATUS_TRACKS),
                    "itemsDone" to Scalar,
                    "itemsTotal" to Scalar,
                    "failures" to Rows(Limits.MAX_STATUS_FAILURES, Nested),
                    "warnings" to Rows(Limits.MAX_STATUS_WARNINGS, text),
                ),
            DownloadFailure::class.java to mapOf("item" to text, "error" to text),
            // What DownloadStore.clearFinished reads of a status: compared with the final states, not kept.
            Class.forName("io.github.brainage04.tunemesh.library.DownloadStore\$Companion\$StatusState") to
                mapOf("state" to Elsewhere("compared, not kept")),
            WorkerInfo::class.java to
                mapOf(
                    "version" to Scalar,
                    "device" to Bytes(Limits.MAX_DEVICE_NAME_BYTES),
                    "heartbeatAt" to Bytes(Limits.MAX_TIMESTAMP_BYTES),
                ),
            // What the app state keeps.
            DownloadList::class.java to mapOf("items" to Rows(Limits.MAX_DOWNLOADS_READ, Nested), "older" to Scalar),
            DownloadItem::class.java to
                mapOf(
                    "id" to Elsewhere("a file name"),
                    "hasRequest" to Scalar,
                    "state" to OneOf(DownloadStatus.STATES),
                    "progress" to Scalar,
                    "itemsDone" to Scalar,
                    "itemsTotal" to Scalar,
                    "trackCount" to Count(Limits.MAX_STATUS_TRACKS),
                    "playlist" to line,
                    "playlistFromSource" to Scalar,
                    "shown" to Nested,
                ),
            ShownDownload::class.java to
                mapOf(
                    "input" to line,
                    "title" to line,
                    "message" to Chars(ShownDownload.PREVIEW_CHARS),
                    "failedCount" to Scalar,
                    "warningCount" to Scalar,
                    "lines" to Rows(Limits.SHOWN_LIST_ROWS, Chars(ShownDownload.LINE_PREVIEW_CHARS)),
                ),
            Downloader.Active::class.java to mapOf("device" to Bytes(Limits.MAX_DEVICE_NAME_BYTES)),
            // Playlists.
            Playlist::class.java to
                mapOf(
                    "name" to Elsewhere("a file name"),
                    "size" to Scalar,
                    "artPaths" to Rows(PlaylistStore.ART_PATHS, path),
                    "modifiedMs" to Scalar,
                    "problem" to Elsewhere("this app's own text"),
                ),
            PlaylistContents::class.java to mapOf("name" to Elsewhere("a file name"), "entries" to Nested, "modifiedMs" to Scalar),
            PlaylistEntries::class.java to
                mapOf(
                    "root" to Elsewhere("this app's setting"),
                    "shared" to Elsewhere("the library index's strings, at most 250,000"),
                    "ends" to Elsewhere("one per entry, at most 250,000"),
                    "bytes" to Elsewhere("the lines, within the 32 MiB file"),
                    "extinfs" to Elsewhere("one bounded #EXTINF line per entry (see PlaylistEntry.extinf)"),
                    "size" to Scalar,
                ),
            PlaylistEntry::class.java to
                mapOf("path" to path, "line" to path, "extinf" to Bytes(Limits.MAX_LABEL_BYTES + 32), "isShared" to Scalar),
            // Audio tags.
            RawTags::class.java to
                mapOf(
                    "title" to Elsewhere("cut in Track.fromTags"),
                    "artist" to Elsewhere("cut in Track.fromTags"),
                    "album" to Elsewhere("cut in Track.fromTags"),
                    "albumArtist" to Elsewhere("cut in Track.fromTags"),
                    "trackNumber" to Scalar,
                    "discNumber" to Scalar,
                    "durationMs" to Scalar,
                ),
            Track::class.java to
                mapOf(
                    "path" to Elsewhere("a path within the folder depth limit"),
                    "title" to tag,
                    "artist" to tag,
                    "album" to tag,
                    "albumArtist" to tag,
                    "trackNumber" to Scalar,
                    "discNumber" to Scalar,
                    "durationMs" to Scalar,
                    "size" to Scalar,
                    "modifiedMs" to Scalar,
                ),
        )

    private fun properties(type: Class<*>): Set<String> =
        type.declaredFields.filter { !Modifier.isStatic(it.modifiers) && !it.isSynthetic }.mapTo(HashSet()) { it.name }

    @Test
    fun everyPropertyHasALimit() {
        for ((type, limit) in limits) {
            assertEquals(
                "properties of ${type.simpleName} without a limit, or limits of properties it no longer has",
                limit.keys,
                properties(type),
            )
        }
        // Every @Serializable class of the library package (the kotlinx plugin adds a `$serializer` to each) is listed.
        val location = File(DownloadRequest::class.java.protectionDomain.codeSource.location.toURI())
        val prefix = "io/github/brainage04/tunemesh/library/"
        val names =
            if (location.isDirectory) File(location, prefix).list()!!.map { prefix + it }
            else JarFile(location).use { jar -> jar.entries().toList().map { it.name } }
        val serializable =
            names
                .filter { it.startsWith(prefix) && it.endsWith("\$\$serializer.class") }
                .map { Class.forName(it.removeSuffix("\$\$serializer.class").replace('/', '.')) }
        val unlisted = serializable.filter { it !in limits }
        assertTrue("serializable models without limits: $unlisted", unlisted.isEmpty())
        assertTrue(DownloadStatus::class.java in serializable && WorkerInfo::class.java in serializable)
    }

    /** A JSON value for [descriptor] with every string past its limit and every list past its length ([Limit]s of [type]). */
    private fun hostile(descriptor: SerialDescriptor, type: Class<*>): String {
        val fields = limits.getValue(type)
        return (0 until descriptor.elementsCount).joinToString(",", "{", "}") { i ->
            val name = descriptor.getElementName(i)
            val limit = fields[name] ?: fields.getValue(FIELD_OF.getValue(name))
            val element = descriptor.getElementDescriptor(i)
            "\"$name\":" + value(limit, element, type)
        }
    }

    private fun value(limit: Limit, element: SerialDescriptor, owner: Class<*>): String =
        when (limit) {
            is Bytes -> "\"" + "a" + "é".repeat(limit.max) + "\""
            is OneOf -> "\"" + limit.values.first() + "\""
            is Count -> List(limit.max + 100) { "\"t\"" }.joinToString(",", "[", "]")
            is Rows -> {
                val each = element.getElementDescriptor(0)
                val one = if (limit.each == Nested) hostile(each, NESTED.getValue(owner)) else value(limit.each, each, owner)
                List(limit.max + 10) { one }.joinToString(",", "[", "]")
            }
            Scalar ->
                when (element.kind) {
                    PrimitiveKind.BOOLEAN -> "true"
                    PrimitiveKind.DOUBLE,
                    PrimitiveKind.FLOAT -> "0.5"
                    else -> "4294967295"
                }
            is Chars,
            Nested,
            is Elsewhere -> error("no file value for $limit")
        }

    /** Checks every property of [value] against its [limits] entry, recursively. */
    private fun check(value: Any, where: String = value.javaClass.simpleName) {
        val fields = limits[value.javaClass] ?: error("no limits for ${value.javaClass}")
        for ((name, limit) in fields) {
            val field = value.javaClass.getDeclaredField(name).apply { isAccessible = true }
            check(field.get(value) ?: continue, limit, "$where.$name")
        }
    }

    private fun check(value: Any, limit: Limit, where: String) {
        when (limit) {
            is Bytes -> assertTrue("$where: ${Limits.utf8Length(value as String)} B", Limits.utf8Length(value) <= limit.max)
            is Chars -> assertTrue("$where: ${(value as String).length} chars", value.length <= limit.max)
            is OneOf -> assertTrue("$where: $value", value in limit.values)
            is Count -> assertTrue("$where: $value", (value as Int) in 0..limit.max)
            is Rows -> {
                assertTrue("$where: ${(value as List<*>).size} rows", value.size <= limit.max)
                value.filterNotNull().forEach { if (limit.each == Nested) check(it, "$where[]") else check(it, limit.each, "$where[]") }
            }
            Nested -> check(value, where)
            Scalar,
            is Elsewhere -> Unit
        }
    }

    @Test
    fun boundedReadsCutEveryField() {
        val request = DownloadStore.parseRequest(hostile(serializer<DownloadRequest>().descriptor, DownloadRequest::class.java))!!
        check(request)
        assertEquals(Limits.MAX_REQUEST_INPUT_BYTES - 1L, Limits.utf8Length(request.input)) // "a" then 2-byte characters
        val status = DownloadStore.parseStatus(hostile(serializer<DownloadStatus>().descriptor, DownloadStatus::class.java))!!
        check(status)
        assertEquals(Limits.MAX_STATUS_TRACKS, status.trackCount)
        assertEquals(Limits.MAX_STATUS_FAILURES, status.failures.size)
        val worker = DownloadStore.parseWorker(hostile(serializer<WorkerInfo>().descriptor, WorkerInfo::class.java))!!
        check(worker)

        val item = DownloadItem.of("1-a", request, status)
        check(DownloadList(List(Limits.MAX_DOWNLOADS_READ) { item }, 5))
        assertEquals(Limits.SHOWN_LIST_ROWS, item.shown.lines.size)
        check(Downloader.of(worker.copy(heartbeatAt = "2026-10-06T05:00:00Z"), Instant.parse("2026-10-06T05:00:00Z")))

        val big = "é".repeat(Limits.MAX_TAG_TEXT_BYTES)
        check(Track.fromTags("A/B/c.mp3", RawTags(big, big, big, big), 1, 1))
        val label = "#EXTINF:${"9".repeat(30)},  ${"é".repeat(Limits.MAX_LABEL_BYTES)}\n../A/b.mp3\n"
        M3u8.parse(label + label.replace("9".repeat(30), "-1"), "/lib").forEach { check(it) }
    }

    companion object {
        /** Serial names that differ from the property name. */
        private val FIELD_OF = mapOf("tracks" to "trackCount")

        /** The element class of a list of objects, by its owner. */
        private val NESTED = mapOf<Class<*>, Class<*>>(DownloadStatus::class.java to DownloadFailure::class.java)
    }
}
