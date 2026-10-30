package io.github.brainage04.spotless.library

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/** The streaming reader of request and status files: strict JSON, texts cut as [Limits.cutUtf8] cuts them, skipped values not kept. */
class JsonReaderTest {
    private fun reader(text: String) = JsonReader(text.toByteArray(Charsets.UTF_8).inputStream())

    private fun readerOf(bytes: ByteArray) = JsonReader(bytes.inputStream())

    /** A string read through [JsonReader] with [max] bytes, as the whole string decoded by kotlinx and then cut. */
    private fun assertCutLikeLimits(json: String, max: Int) {
        val whole = Json.decodeFromString<String>(json)
        assertEquals("$json at $max", Limits.cutUtf8(whole, max), reader(json).string(max))
    }

    @Test
    fun stringsAreCutAsLimitsCutsThem() {
        val samples =
            listOf(
                "plain ascii",
                "é".repeat(3000),
                "a" + "é".repeat(3000),
                "🎵".repeat(1500),
                "a\\u00e9\\n\\t\\\"\\\\\\/\\b\\f\\r",
                "\\ud83c\\udfb5 pair as escapes",
                "lone \\ud800 high, lone \\udc00 low, \\ud800\\ud800 two highs",
                "€".repeat(5000),
                "x".repeat(10_000),
            )
        for (sample in samples) {
            val json = "\"$sample\""
            for (max in listOf(0, 1, 2, 3, 4, 5, 7, 64, 1023, 1024, 4096, 8192, 100_000)) assertCutLikeLimits(json, max)
        }
    }

    @Test
    fun longStringsAreCutAcrossTheDecodeBuffer() {
        // Multi-byte sequences straddle the reader's 4 KiB decode run at every offset.
        for (prefix in 0..3) {
            val text = "a".repeat(prefix) + "€🎵é".repeat(5000)
            assertCutLikeLimits(JsonPrimitive(text).toString(), 30_000)
            assertCutLikeLimits(JsonPrimitive(text).toString(), Int.MAX_VALUE)
        }
    }

    @Test
    fun malformedUtf8DecodesAsStringDoes() {
        val bytes =
            byteArrayOf(
                '"'.code.toByte(),
                0xC3.toByte(),
                'a'.code.toByte(),
                0xE2.toByte(),
                0x82.toByte(),
                0x80.toByte(),
                0xFF.toByte(),
                '"'.code.toByte(),
            )
        val expected = String(bytes, 1, bytes.size - 2, Charsets.UTF_8)
        assertEquals(expected, readerOf(bytes).string(100))
    }

    @Test
    fun wasCutTellsWhetherAnythingWasDropped() {
        val reader = reader("[\"abc\",\"abcd\"]")
        reader.beginArray()
        assertEquals("abc", reader.string(3))
        assertFalse(reader.wasCut)
        assertTrue(reader.nextElement())
        assertEquals("abc", reader.string(3))
        assertTrue(reader.wasCut)
    }

    @Test
    fun skippedValuesMayNestDeeply() {
        val deep = "[".repeat(200_000) + "]".repeat(200_000)
        val reader = reader("""{"skip":$deep,"keep":"yes","more":{"a":[1,-2.5e3,true,false,null,"s",{}],"b":{}}}""")
        val kept = HashMap<String, String>()
        reader.forEachField { name -> if (name == "keep") kept[name] = reader.string(10) else reader.skipValue() }
        reader.end()
        assertEquals(mapOf("keep" to "yes"), kept)
    }

    @Test
    fun invalidJsonIsRefused() {
        val invalid =
            listOf(
                "",
                "{",
                "{\"a\":1,}",
                "{\"a\" 1}",
                "{\"a\":[1,]}",
                "{\"a\":01}",
                "{\"a\":1.}",
                "{\"a\":.5}",
                "{\"a\":tru}",
                "{\"a\":\"\\x\"}",
                "{\"a\":\"\\u12g4\"}",
                "{\"a\":\"open}",
                "{\"a\":1} x",
                "{\"a\":[}",
                "[1 2]",
                "{a:1}",
            )
        for (text in invalid) {
            assertThrows(text, JsonReader.Malformed::class.java) {
                val reader = reader(text)
                reader.skipValue()
                reader.end()
            }
        }
    }

    @Test
    fun numbersAndLiterals() {
        val reader = reader("""[-0, 4294967295, 1.5e2, true, false, null]""")
        reader.beginArray()
        assertEquals(0L, reader.long())
        reader.nextElement()
        assertEquals(4_294_967_295L, reader.long())
        reader.nextElement()
        assertEquals(150.0, reader.double(), 0.0)
        reader.nextElement()
        assertTrue(reader.boolean())
        reader.nextElement()
        assertFalse(reader.boolean())
        reader.nextElement()
        assertTrue(reader.nextIsNull())
        assertFalse(reader.nextElement())
        reader.end()
        assertThrows(JsonReader.Malformed::class.java) { reader("1.5").long() }
        assertThrows(JsonReader.Malformed::class.java) { reader("99999999999999999999").long() }
        assertThrows(JsonReader.Malformed::class.java) { reader("\"5\"").long() }
        assertThrows(JsonReader.Malformed::class.java) { reader("1e999").double() }
    }

    @Test
    fun inputPastTheLimitIsRefused() {
        val text = "\"" + "x".repeat(100_000) + "\""
        assertThrows(JsonReader.Malformed::class.java) { JsonReader(text.byteInputStream(), maxBytes = 50_000).skipValue() }
        assertEquals(10, JsonReader(text.byteInputStream(), maxBytes = 100_002).string(10).length)
    }

    @Test
    fun statusFieldsOfTheWrongTypeMakeTheFileInvalid() {
        fun status(extra: String) = DownloadStore.parseStatus("""{"version":1,"id":"a","state":"done"$extra}""")
        assertEquals("done", status("")?.state)
        assertNull(status(""","input":null"""))
        assertNull(status(""","tracks":null"""))
        assertNull(status(""","failures":[null]"""))
        assertNull(status(""","failures":[{"item":1}]"""))
        assertNull(status(""","warnings":["a",2]"""))
        assertNull(status(""","itemsDone":"3""""))
        assertNull(status(""","progress":true"""))
        assertEquals(null, status(""","progress":null,"title":null,"message":null""")?.progress)
        assertEquals(3, status(""","tracks":["a","b"],"tracks":["a","b","c"]""")?.trackCount)
        // A failure past the 200 kept is still checked.
        val failures = List(Limits.MAX_STATUS_FAILURES) { """{"item":"i","error":"e"}""" }
        assertEquals(Limits.MAX_STATUS_FAILURES, status(""","failures":[${failures.joinToString(",")}]""")?.failures?.size)
        assertNull(status(""","failures":[${failures.joinToString(",")},{"item":[]}]"""))
        assertNull(DownloadStore.parseStatus("""{"version":1,"state":"done"}"""))
        assertNull(DownloadStore.parseRequest("""{"version":1,"id":"a"}"""))
        assertNull(DownloadStore.parseRequest("""{"version":1,"id":"a","input":"x","playlistFromSource":"yes"}"""))
        assertEquals("x", DownloadStore.parseRequest("""{"version":1,"id":"a","input":"x","playlist":null,"other":[{}]}""")?.input)
    }
}
