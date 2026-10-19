package io.github.brainage04.tunemesh.library

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.time.Instant

class DownloadStoreTest {
    @get:Rule
    val temp = TemporaryFolder()

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
        File(status, "${running.id}.json").writeText(
            """{"version":1,"id":"${running.id}","input":"second","state":"downloading","title":"Song","progress":0.42,"message":null,"tracks":[],"updatedAt":"2026-10-06T05:01:52Z","future":"ignored"}""",
        )
        File(status, "3000-deadbeef.json").writeText(
            """{"version":1,"id":"3000-deadbeef","input":"third","state":"failed","title":null,"progress":null,"message":"Video unavailable","tracks":[],"updatedAt":"2026-10-06T05:01:52Z"}""",
        )
        File(status, "broken.json").writeText("{")
        val items = store.list()
        assertEquals(listOf("3000-deadbeef", running.id, waiting.id), items.map { it.id })
        assertEquals(null, items[2].status)
        assertTrue(items[2].canCancel)
        assertEquals(0.42, items[1].status!!.progress!!, 0.0)
        assertTrue(items[0].isFinal)
        assertFalse(items[0].canCancel)
        assertEquals("third", items[0].input)

        store.cancel(waiting.id)
        store.clearFinished()
        assertEquals(listOf(running.id), store.list().map { it.id })
        assertTrue(File(status, "${running.id}.json").exists())
    }

    @Test
    fun statusProgressFieldsAreReadAndOptional() {
        val status = File(temp.root, ".tunemesh/status").apply { mkdirs() }
        File(status, "1000-aaaaaaaa.json").writeText(
            """{"version":1,"id":"1000-aaaaaaaa","input":"x","state":"downloading","done":3,"total":12,"warnings":["Track 2: Video unavailable"]}""",
        )
        File(status, "2000-bbbbbbbb.json").writeText("""{"version":1,"id":"2000-bbbbbbbb","state":"queued"}""")
        val items = DownloadStore(temp.root).list().associateBy { it.id }
        val counted = items.getValue("1000-aaaaaaaa").status!!
        assertEquals(3, counted.done)
        assertEquals(12, counted.total)
        assertEquals(listOf("Track 2: Video unavailable"), counted.warnings)
        val plain = items.getValue("2000-bbbbbbbb").status!!
        assertEquals(null, plain.done)
        assertEquals(null, plain.total)
        assertEquals(emptyList<String>(), plain.warnings)
    }

    @Test
    fun submitAllWritesOneRequestPerLineInPasteOrder() {
        val store = DownloadStore(temp.root)
        val text = """
            https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M

            https://www.youtube.com/watch?v=jNQXAC9IVRw
            https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy
            https://www.youtube.com/watch?v=jNQXAC9IVRw
        """.trimIndent()
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
        assertEquals(requests.map { it.id }.reversed(), store.list().map { it.id })
        assertTrue(requests.zipWithNext().all { (a, b) -> a.id.substringBefore('-').toLong() < b.id.substringBefore('-').toLong() })

        val targeted = store.submitAll("https://open.spotify.com/playlist/x\nartist - song", "Road trip", saveAsPlaylists = true, requestedBy = "x", now = now)
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
        File(state, "worker.json").writeText(
            """{"version":1,"device":"home-server","heartbeatAt":"2026-10-06T05:00:00Z","future":"ignored"}""",
        )
        val info = store.worker()!!
        assertEquals("home-server", info.device)
        assertEquals(Downloader.Active("home-server"), Downloader.of(info, now))
        assertEquals("Downloads are handled by home-server", Downloader.of(info, now).description)
        // Stale after three minutes without a heartbeat.
        assertEquals(Downloader.Active("home-server"), Downloader.of(info, Instant.parse("2026-10-06T05:03:00Z")))
        assertEquals(Downloader.None, Downloader.of(info, Instant.parse("2026-10-06T05:03:01Z")))
        assertEquals("No device is downloading right now", Downloader.None.description)
        // A heartbeat far in the future (wrong clock) is not trusted.
        assertEquals(Downloader.None, Downloader.of(info, Instant.parse("2026-10-06T04:50:00Z")))

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
}
