package io.github.brainage04.tunemesh.library

import java.io.File
import java.time.Instant
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class DownloadStoreTest {
    @get:Rule val temp = TemporaryFolder()

    private val now = Instant.parse("2026-10-06T05:01:37.123Z")

    @Test
    fun requestFileMatchesTheContract() {
        val request = DownloadStore(temp.root).submit("  https://www.youtube.com/watch?v=jNQXAC9IVRw ", null, false, "pixel-8", now)
        val file = File(temp.root, ".tunemesh/requests/${request.id}.json")
        assertTrue(file.isFile)
        assertTrue(Regex("1791262897123-[0-9a-f]{8}").matches(request.id))
        val json = Json.parseToJsonElement(file.readText()).jsonObject
        assertEquals(
            listOf("version", "id", "input", "playlist", "playlistFromSource", "requestedBy", "requestedAt"),
            json.keys.toList(),
        )
        assertEquals(JsonPrimitive(1), json["version"])
        assertEquals(JsonPrimitive(request.id), json["id"])
        assertEquals(JsonPrimitive("https://www.youtube.com/watch?v=jNQXAC9IVRw"), json["input"])
        assertEquals(JsonNull, json["playlist"])
        assertEquals(JsonPrimitive(false), json["playlistFromSource"])
        assertEquals(JsonPrimitive("pixel-8"), json["requestedBy"])
        assertEquals(JsonPrimitive("2026-10-06T05:01:37Z"), json["requestedAt"])
        assertEquals(listOf("${request.id}.json"), File(temp.root, ".tunemesh/requests").list()!!.toList())
    }

    @Test
    fun playlistOptionsAreEncoded() {
        val store = DownloadStore(temp.root)
        val named = store.submit("daft punk one more time", "Road trip", true, "x", now)
        assertEquals("Road trip", named.playlist)
        assertFalse(named.playlistFromSource)
        val fromSource = store.submit("https://open.spotify.com/album/abc", " ", true, "x", now)
        assertEquals(null, fromSource.playlist)
        assertTrue(fromSource.playlistFromSource)
        val json = Json.parseToJsonElement(File(temp.root, ".tunemesh/requests/${fromSource.id}.json").readText()) as JsonObject
        assertEquals(JsonPrimitive(true), json["playlistFromSource"])
    }

    @Test
    fun listMergesRequestsAndStatusAndClearsOnlyFinishedOnes() {
        val store = DownloadStore(temp.root)
        val waiting = store.submit("first", null, false, "x", Instant.ofEpochMilli(1000))
        val running = store.submit("second", null, false, "x", Instant.ofEpochMilli(2000))
        val status = File(temp.root, ".tunemesh/status").apply { mkdirs() }
        File(status, "${running.id}.json")
            .writeText(
                """{"version":1,"id":"${running.id}","input":"second","state":"downloading","title":"Song","progress":0.42,"message":null,"tracks":[],"updatedAt":"2026-10-06T05:01:52Z","future":"ignored"}"""
            )
        File(status, "3000-deadbeef.json")
            .writeText(
                """{"version":1,"id":"3000-deadbeef","input":"third","state":"failed","title":null,"progress":null,"message":"Video unavailable","tracks":[],"updatedAt":"2026-10-06T05:01:52Z"}"""
            )
        File(status, "broken.json").writeText("{")
        val items = store.list().items
        assertEquals(listOf("3000-deadbeef", running.id, waiting.id), items.map { it.id })
        assertEquals(null, items[2].state)
        assertTrue(items[2].canCancel)
        assertEquals(0.42, items[1].progress!!, 0.0)
        assertTrue(items[0].isFinal)
        assertFalse(items[0].canCancel)
        assertEquals("third", items[0].shown.input)

        store.cancel(waiting.id)
        store.clearFinished()
        assertEquals(listOf(running.id), store.list().items.map { it.id })
        assertTrue(File(status, "${running.id}.json").exists())
    }

    @Test
    fun statusProgressFieldsAreReadAndOptional() {
        val status = File(temp.root, ".tunemesh/status").apply { mkdirs() }
        File(status, "1000-aaaaaaaa.json")
            .writeText(
                """{"version":1,"id":"1000-aaaaaaaa","input":"x","state":"downloading","itemsDone":3,"itemsTotal":12,""" +
                    """"failures":[{"item":"Artist - Title","error":"Video unavailable"}],"warnings":["Imported the first 100 tracks"]}"""
            )
        File(status, "2000-bbbbbbbb.json").writeText("""{"version":1,"id":"2000-bbbbbbbb","state":"queued"}""")
        val items = DownloadStore(temp.root).list().items.associateBy { it.id }
        val counted = items.getValue("1000-aaaaaaaa")
        assertEquals(3L, counted.itemsDone)
        assertEquals(12L, counted.itemsTotal)
        assertEquals(1, counted.shown.failedCount)
        assertEquals(1, counted.shown.warningCount)
        assertEquals(listOf("Artist - Title: Video unavailable", "Imported the first 100 tracks"), counted.shown.lines)
        val plain = items.getValue("2000-bbbbbbbb")
        assertEquals(null, plain.itemsDone)
        assertEquals(null, plain.itemsTotal)
        assertEquals(0, plain.shown.failedCount + plain.shown.warningCount)
        assertEquals(emptyList<String>(), plain.shown.lines)
    }

    @Test
    fun submitAllWritesOneRequestPerLineInPasteOrder() {
        val store = DownloadStore(temp.root)
        val text =
            """
            https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M

            https://www.youtube.com/watch?v=jNQXAC9IVRw
            https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy
            https://www.youtube.com/watch?v=jNQXAC9IVRw
            """
                .trimIndent()
        val requests = store.submitAll(text, null, saveAsPlaylists = true, requestedBy = "pixel-8", now = now)
        assertEquals(
            listOf(
                "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M",
                "https://www.youtube.com/watch?v=jNQXAC9IVRw",
                "https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy",
            ),
            requests.map { it.input },
        )
        assertEquals(listOf(true, false, true), requests.map { it.playlistFromSource })
        // Newest first in the list, so paste order reversed; IDs order the worker's queue.
        assertEquals(requests.map { it.id }.reversed(), store.list().items.map { it.id })
        assertTrue(requests.zipWithNext().all { (a, b) -> a.id.substringBefore('-').toLong() < b.id.substringBefore('-').toLong() })

        val targeted =
            store.submitAll(
                "https://open.spotify.com/playlist/x\nartist - song",
                "Road trip",
                saveAsPlaylists = true,
                requestedBy = "x",
                now = now,
            )
        assertEquals(listOf("Road trip", "Road trip"), targeted.map { it.playlist })
        assertEquals(listOf(false, false), targeted.map { it.playlistFromSource })
    }

    @Test(expected = IllegalArgumentException::class)
    fun submitAllRefusesBlankText() {
        DownloadStore(temp.root).submitAll(" \n\n ", null, false, "x", now)
    }

    @Test
    fun workerFileNamesTheActiveDownloader() {
        val store = DownloadStore(temp.root)
        assertEquals(null, store.worker())
        assertEquals(Downloader.None, Downloader.of(store.worker(), now))
        val state = File(temp.root, ".tunemesh").apply { mkdirs() }
        File(state, "worker.json")
            .writeText(
                """{"version":1,"device":"home-server","instance":"5f0c1d2e3a4b6c7d","pid":41234,"workerVersion":"0.1.0","heartbeatAt":"2026-10-06T05:00:00Z"}"""
            )
        val info = store.worker()!!
        assertEquals("home-server", info.device)
        assertEquals(Downloader.Active("home-server"), Downloader.of(info, now))
        assertEquals("Downloads are handled by home-server", Downloader.of(info, now).description)
        // Stale once the heartbeat is three minutes old, in either direction.
        assertEquals(Downloader.Active("home-server"), Downloader.of(info, Instant.parse("2026-10-06T05:02:59Z")))
        assertEquals(Downloader.None, Downloader.of(info, Instant.parse("2026-10-06T05:03:00Z")))
        assertEquals("No device is downloading right now", Downloader.None.description)
        assertEquals(Downloader.Active("home-server"), Downloader.of(info, Instant.parse("2026-10-06T04:57:01Z")))
        assertEquals(Downloader.None, Downloader.of(info, Instant.parse("2026-10-06T04:50:00Z")))
        File(state, "worker.json").writeText("""{"version":1,"device":"desktop","heartbeatAt":"2026-10-06T15:01:00+10:00"}""")
        assertEquals(Downloader.Active("desktop"), Downloader.of(store.worker(), now))

        File(state, "worker.json").writeText("{")
        assertEquals(null, store.worker())
        File(state, "worker.json").writeText("""{"version":1,"device":"x","heartbeatAt":"yesterday"}""")
        assertEquals(Downloader.None, Downloader.of(store.worker(), now))
        File(state, "worker.json").writeText("""{"version":1,"device":" ","heartbeatAt":"2026-10-06T05:01:00Z"}""")
        assertEquals(Downloader.None, Downloader.of(store.worker(), now))
    }

    @Test
    fun deviceSlug() {
        assertEquals("alexs-pixel-8", DownloadStore.deviceSlug("Alex's Pixel 8"))
        assertEquals("alexs-pixel-8", DownloadStore.deviceSlug("Alex’s Pixel 8"))
        assertEquals("sm-s921b", DownloadStore.deviceSlug("SM-S921B"))
        assertEquals("android", DownloadStore.deviceSlug("  "))
    }

    /** R2-opus-1: a request, status or worker file over 1 MiB is skipped, without reading it whole. */
    @Test
    fun filesOverTheJsonLimitAreSkipped() {
        val state = File(temp.root, ".tunemesh").apply { mkdirs() }
        val status = File(state, "status").apply { mkdirs() }
        val requests = File(state, "requests").apply { mkdirs() }
        File(status, "1000-aaaaaaaa.json").writeText("""{"version":1,"id":"1000-aaaaaaaa","state":"done"}""")
        // Sparse, so the test does not need the memory: larger than any heap the JVM test gets.
        java.io.RandomAccessFile(File(status, "2000-bbbbbbbb.json"), "rw").use { it.setLength(4L * 1024 * 1024 * 1024) }
        val padded = """{"version":1,"id":"3000-cccccccc","input":"x","note":"${"x".repeat(Limits.MAX_JSON_BYTES)}"}"""
        File(requests, "3000-cccccccc.json").writeText(padded)
        val exact = """{"version":1,"id":"4000-dddddddd","input":"y","requestedBy":"x","requestedAt":"2026-10-06T05:01:37Z"}"""
        File(requests, "4000-dddddddd.json").writeText(exact + " ".repeat(Limits.MAX_JSON_BYTES - exact.length))
        assertEquals(listOf("4000-dddddddd", "1000-aaaaaaaa"), DownloadStore(temp.root).list().items.map { it.id })
        java.io.RandomAccessFile(File(state, "worker.json"), "rw").use { it.setLength(Limits.MAX_JSON_BYTES + 1L) }
        assertEquals(null, DownloadStore(temp.root).worker())
    }

    /** R2-opus-4 and "Limits": statuses are cut when read, and what the Downloads tab shows is bounded again. */
    @Test
    fun statusTextsAreCutWhenReadAndShown() {
        val status =
            DownloadStore.bounded(
                DownloadStatus(
                    id = "1-a",
                    input = "i".repeat(10_000),
                    state = "done",
                    title = "t".repeat(10_000),
                    message = "Imported the first 100 tracks.\n" + "m\n".repeat(250_000),
                    trackCount = 20_000,
                    failures = List(300) { DownloadFailure("Item $it", "é".repeat(5_000)) },
                    warnings = listOf("Imported the first 100 tracks") + List(55_000) { "w$it" },
                )
            )
        assertEquals(Limits.MAX_REQUEST_INPUT_BYTES, status.input.length)
        assertEquals(Limits.MAX_STATUS_TEXT_BYTES, status.title!!.length)
        assertEquals(Limits.MAX_STATUS_MESSAGE_BYTES, status.message!!.toByteArray().size)
        assertEquals(Limits.MAX_STATUS_TRACKS, status.trackCount)
        assertEquals(Limits.MAX_STATUS_FAILURES, status.failures.size)
        assertEquals("é".repeat(Limits.MAX_STATUS_TEXT_BYTES / 2), status.failures[0].error)
        assertEquals(Limits.MAX_STATUS_WARNINGS, status.warnings.size)

        val shown = ShownDownload.of(status.input, status)
        assertEquals(Limits.MAX_STATUS_FAILURES, shown.failedCount)
        assertEquals(Limits.MAX_STATUS_WARNINGS - 1, shown.warningCount) // the message repeats the first one
        assertEquals(Limits.SHOWN_LIST_ROWS, shown.lines.size)
        assertTrue(shown.lines.all { it.length <= ShownDownload.LINE_PREVIEW_CHARS })
        assertEquals(Limits.SHOWN_MESSAGE_LINES, shown.message!!.lines().size)
        assertTrue(shown.message.endsWith("…"))
        assertEquals(Limits.SHOWN_LINE_CHARS, shown.title!!.length)
        assertTrue(shown.title.endsWith("…"))
        assertEquals(Limits.SHOWN_LINE_CHARS, shown.input.length)
        // An unbroken message is cut to what the card's 8 lines can show.
        val unbroken = ShownDownload.of("x", status.copy(message = "m".repeat(Limits.MAX_STATUS_MESSAGE_BYTES)))
        assertEquals(ShownDownload.PREVIEW_CHARS, unbroken.message!!.length)
        assertTrue(unbroken.message.endsWith("…"))

        val few =
            ShownDownload.of(
                "x",
                status.copy(failures = listOf(DownloadFailure("A", "gone")), warnings = listOf("Imported the first 100 tracks", "w")),
            )
        assertEquals(1, few.warningCount)
        assertEquals(listOf("A: gone", "w"), few.lines)
        val emoji = Limits.shownLine("a" + "🎵".repeat(Limits.SHOWN_LINE_CHARS))
        assertFalse(Character.isHighSurrogate(emoji.dropLast(1).last()))
        assertEquals("a b", Limits.shownLine("a\nb"))
    }

    /** "Limits": only the newest 200 ids are read; a request with its status counts once; older are counted. */
    @Test
    fun onlyTheNewestRequestsAreRead() {
        val requests = File(temp.root, ".tunemesh/requests").apply { mkdirs() }
        val statuses = File(temp.root, ".tunemesh/status").apply { mkdirs() }
        for (n in 1..250) {
            val id = "${1000 + n}-aaaaaaaa"
            File(requests, "$id.json")
                .writeText("""{"version":1,"id":"$id","input":"x","requestedBy":"p","requestedAt":"2026-10-06T05:01:37Z"}""")
            if (n % 2 == 0) File(statuses, "$id.json").writeText("""{"version":1,"id":"$id","state":"queued"}""")
        }
        val list = DownloadStore(temp.root).list()
        assertEquals(Limits.MAX_DOWNLOADS_READ, list.items.size)
        assertEquals(50, list.older)
        assertEquals("1250-aaaaaaaa", list.items.first().id)
        assertEquals("1051-aaaaaaaa", list.items.last().id)
        assertTrue(list.items.first().state != null && list.items.first().hasRequest)
    }

    /** R3-Opus-1: the list keeps a track count, never the paths, and none of the other texts uncut. */
    @Test
    fun statusTracksAreCountedNotKept() {
        val statuses = File(temp.root, ".tunemesh/status").apply { mkdirs() }
        val tracks = (1..12_000).joinToString(",") { "\"A/$it.mp3\"" }
        File(statuses, "1000-aaaaaaaa.json").writeText("""{"version":1,"id":"1000-aaaaaaaa","state":"done","tracks":[$tracks]}""")
        File(statuses, "2000-bbbbbbbb.json").writeText("""{"version":1,"id":"x","state":"done","tracks":["one"]}""")
        File(statuses, "3000-cccccccc.json").writeText("""{"version":1,"id":"x","state":"done","tracks":[1]}""")
        val items = DownloadStore(temp.root).list().items.associateBy { it.id }
        assertEquals(Limits.MAX_STATUS_TRACKS, items.getValue("1000-aaaaaaaa").trackCount)
        assertEquals(1, items.getValue("2000-bbbbbbbb").trackCount)
        assertFalse("a track that is not a string makes the file invalid", "3000-cccccccc" in items)
    }

    /** Contract "Limits": a `state` outside the five, or an integer outside 0..2^32-1, makes a status file invalid. */
    @Test
    fun invalidStatesAndIntegersAreSkipped() {
        fun parse(json: String) = DownloadStore.parseStatus(json)
        assertEquals("queued", parse("""{"version":1,"id":"a","state":"queued"}""")?.state)
        assertEquals(null, parse("""{"version":1,"id":"a","state":"exploded"}"""))
        assertEquals(null, parse("""{"version":1,"id":"a","state":"${"x".repeat(2000)}"}"""))
        assertEquals(4_294_967_295L, parse("""{"version":1,"id":"a","state":"done","itemsTotal":4294967295}""")?.itemsTotal)
        assertEquals(null, parse("""{"version":1,"id":"a","state":"done","itemsTotal":4294967296}"""))
        assertEquals(null, parse("""{"version":1,"id":"a","state":"done","itemsDone":-1}"""))
        assertEquals(null, parse("""{"version":-1,"id":"a","state":"done"}"""))
        assertEquals(null, DownloadStore.parseRequest("""{"version":4294967296,"id":"a","input":"x"}"""))
        assertEquals(null, DownloadStore.parseRequest("""{"version":1,"id":"a","input":"x","playlistFromSource":"yes"}"""))
        assertEquals(null, DownloadStore.parseWorker("""{"version":-5,"device":"d","heartbeatAt":"2026-10-06T05:00:00Z"}"""))
    }

    /** Contract "Download requests": a client never writes an input over 8 KiB; a paste with one such line writes nothing. */
    @Test
    fun tooLongInputIsRefused() {
        val store = DownloadStore(temp.root)
        val fits = "a".repeat(Limits.MAX_REQUEST_INPUT_BYTES)
        assertEquals(fits, store.submit("  $fits  ", null, false, "x", now).input)
        val error = assertThrows(IllegalArgumentException::class.java) { store.submitAll("ok\n${fits}é", null, false, "x", now) }
        assertTrue(error.message!!.contains("8 KiB"))
        assertEquals(1, File(temp.root, ".tunemesh/requests").list()!!.size)
    }

    /** Contract "Download status" ("Clear"): every final status whose request file is gone goes, not only the 200 listed. */
    @Test
    fun clearFinishedClearsBeyondTheList() {
        val requests = File(temp.root, ".tunemesh/requests").apply { mkdirs() }
        val statuses = File(temp.root, ".tunemesh/status").apply { mkdirs() }
        for (n in 1..300) File(statuses, "$n-aaaaaaaa.json").writeText("""{"version":1,"id":"x","state":"done"}""")
        File(statuses, "1000-running.json").writeText("""{"version":1,"id":"x","state":"downloading"}""")
        File(statuses, "1001-pending.json").writeText("""{"version":1,"id":"x","state":"done"}""")
        File(requests, "1001-pending.json").writeText("""{"version":1,"id":"x","input":"y"}""")
        File(statuses, "1002-broken.json").writeText("{")
        DownloadStore(temp.root).clearFinished()
        assertEquals(setOf("1000-running.json", "1001-pending.json", "1002-broken.json"), statuses.list()!!.toSet())
    }

    /** "Limits": `worker.json`'s device and heartbeat are cut when read. */
    @Test
    fun workerTextsAreCut() {
        val info = DownloadStore.parseWorker("""{"version":1,"device":"${"d".repeat(5000)}","heartbeatAt":"${"h".repeat(5000)}"}""")!!
        assertEquals(Limits.MAX_DEVICE_NAME_BYTES, info.device.length)
        assertEquals(Limits.MAX_TIMESTAMP_BYTES, info.heartbeatAt.length)
        assertEquals(Downloader.None, Downloader.of(info, now))
    }
}
