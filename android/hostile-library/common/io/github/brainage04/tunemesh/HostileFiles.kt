package io.github.brainage04.tunemesh

import java.io.File

/**
 * Synced files with every field and list past its limit, each file just under the 1 MiB JSON limit (contract: "Limits"), in the shapes of
 * tunemesh-core's `hostile` generator. Plain Kotlin, shared by the JVM unit tests, the instrumented tests and the macrobenchmarks.
 */
object HostileFiles {
    /** Every file stays this much under 1 MiB, so it is read. */
    const val FILE_BYTES = 1024 * 1024 - 4096

    /** A request with every field long, the file about [bytes] (the generator writes older ones of about 61 KB). */
    fun request(id: String, bytes: Int = FILE_BYTES): String {
        val each = (bytes - 200) / 5
        return """{"version":4294967295,"id":"${fill(each, 'i')}","input":"${fill(each, 'n')}","playlist":"${fill(each, 'p')}",""" +
            """"playlistFromSource":true,"requestedBy":"${fill(each, 'b')}","requestedAt":"${fill(each, 'a')}","future":"$id"}"""
    }

    /** Status shape (a): every text field long, and every list at its limit with long texts. */
    fun longStatus(id: String): String {
        val text = 1100
        val failures = (1..250).joinToString(",") { """{"item":"${fill(text, 'é')}","error":"${fill(text, 'e')}"}""" }
        val warnings = (1..60).joinToString(",") { "\"${fill(text, 'w')}\"" }
        val head =
            """{"version":4294967295,"state":"done","progress":1.0,"itemsDone":4294967295,"itemsTotal":4294967295,""" +
                """"failures":[$failures],"warnings":[$warnings],"tracks":[${(1..10).joinToString(",") { "\"${fill(5000, 't')}\"" }}],"future":"$id""""
        val each = (FILE_BYTES - head.toByteArray().size - 200) / 5
        return head +
            ""","id":"${fill(each, 'i')}","input":"${fill(each, 'n')}","title":"${fill(each, 'T')}","message":"${fill(each, 'm')}",""" +
            """"updatedAt":"${fill(each, 'u')}"}"""
    }

    /**
     * Status shape (b): many small entries, what a status costs most per byte: one-character tracks filling the file (about 250,000), 300
     * empty failures and 80 one-character warnings.
     */
    fun manyStatus(id: String): String {
        val out = StringBuilder(FILE_BYTES)
        out.append("""{"version":1,"id":"$id","input":"x","state":"done","message":"m","failures":[""")
        repeat(300) { out.append(if (it == 0) "{}" else ",{}") }
        out.append("""],"warnings":[""")
        repeat(80) { out.append(if (it == 0) "" else ",").append("\"w\"") }
        out.append("""],"tracks":[""")
        var n = 0
        while (out.length < FILE_BYTES - 16) out.append(if (n++ == 0) "" else ",").append("\"t\"")
        return out.append("]}").toString()
    }

    /** A `worker.json` with every field long. */
    fun worker(): String {
        val each = (FILE_BYTES - 200) / 5
        return """{"version":1,"device":"${fill(each, 'd')}","machine":"${fill(each, 'm')}","instance":"${fill(each, 'i')}",""" +
            """"pid":4294967295,"workerVersion":"${fill(each, 'v')}","heartbeatAt":"${fill(each, 'h')}"}"""
    }

    /**
     * A playlist just under the 32 MiB limit whose every entry has an `#EXTINF` label over the 1 KiB limit (with leading white space, which
     * is trimmed before it is cut) and an absolute path outside the library, so every entry keeps its own line.
     */
    fun labelPlaylist(maxBytes: Long = 32L * 1024 * 1024 - 64 * 1024): String {
        val label = "  " + fill(1100, 'l')
        val out = StringBuilder()
        out.append("#EXTM3U\n")
        var n = 0
        while (true) {
            val entry = "#EXTINF:123,$label\n/outside/hostile/${(n++).toString().padStart(8, '0')}.mp3\n"
            if (out.length + entry.length > maxBytes) break
            out.append(entry)
        }
        return out.toString()
    }

    /** A playlist at the entry limit (250,000 entries), each a distinct path outside the library. */
    fun entryPlaylist(entries: Int = 250_000): String {
        val out = StringBuilder(entries * 40)
        out.append("#EXTM3U\n")
        for (n in 0 until entries) out.append("../Hostile/").append(n).append(".mp3\n")
        return out.toString()
    }

    /**
     * Writes [count] requests ([request]) and a status for each (alternating [longStatus] and [manyStatus]) below `<root>/.tunemesh/`: what
     * clients list at most (contract: the newest 200 ids), every file just under 1 MiB. Returns the bytes written.
     */
    fun writeDownloads(root: File, count: Int = 200): Long {
        val requests = File(root, ".tunemesh/requests").apply { mkdirs() }
        val statuses = File(root, ".tunemesh/status").apply { mkdirs() }
        var bytes = 0L
        for (i in 0 until count) {
            val id = "${1_700_000_000_000L + i}-${i.toString(16).padStart(8, '0')}"
            val request = request(id).toByteArray()
            val status = (if (i % 2 == 0) longStatus(id) else manyStatus(id)).toByteArray()
            File(requests, "$id.json").writeBytes(request)
            File(statuses, "$id.json").writeBytes(status)
            bytes += request.size + status.size
        }
        return bytes
    }

    /** [bytes] UTF-8 bytes of [char] (fewer when [char] takes more than one byte). */
    private fun fill(bytes: Int, char: Char): String = char.toString().repeat(bytes / char.toString().toByteArray().size)
}
