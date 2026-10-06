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
        val request = DownloadStore(temp.root).submit("  https://www.youtube.com/watch?v=jNQXAC9IVRw ", null, false, "thomass-s21", now)
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
        assertEquals(JsonPrimitive("thomass-s21"), json["requestedBy"])
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
    fun deviceSlug() {
        assertEquals("thomass-s21", DownloadStore.deviceSlug("Thomas's S21"))
        assertEquals("thomass-s21", DownloadStore.deviceSlug("Thomas’s S21"))
        assertEquals("sm-g991b", DownloadStore.deviceSlug("SM-G991B"))
        assertEquals("android", DownloadStore.deviceSlug("  "))
    }
}
