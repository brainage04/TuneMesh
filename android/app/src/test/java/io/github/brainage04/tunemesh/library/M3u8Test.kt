package io.github.brainage04.tunemesh.library

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class M3u8Test {
    private val root = "/storage/emulated/0/Music/TuneMesh"

    private fun track(path: String, title: String, artist: String, durationMs: Long) =
        Track(path, title, artist, "Album", artist, null, null, durationMs, 1, 1)

    @Test
    fun roundTripKeepsUnicodePathsMissingEntriesAndOrder() {
        val text = """
            #EXTM3U
            #EXTINF:213,Björk - Human Behaviour
            ../Björk/Début/Human Behaviour [abc123].opus
            #EXTINF:180,宇多田ヒカル - First Love
            ../宇多田ヒカル/First Love/First Love [xyz].m4a
            #EXTINF:99,Gone - Missing Song
            ../Gone/Somewhere/Missing Song [gone1].opus
            ../Björk/Début/Human Behaviour [abc123].opus
        """.trimIndent() + "\n"
        val entries = M3u8.parse(text, root)
        assertEquals(
            listOf(
                "Björk/Début/Human Behaviour [abc123].opus",
                "宇多田ヒカル/First Love/First Love [xyz].m4a",
                "Gone/Somewhere/Missing Song [gone1].opus",
                "Björk/Début/Human Behaviour [abc123].opus",
            ),
            entries.map { it.path },
        )
        val known = mapOf(
            "Björk/Début/Human Behaviour [abc123].opus" to track("Björk/Début/Human Behaviour [abc123].opus", "Human Behaviour", "Björk", 212_600),
            "宇多田ヒカル/First Love/First Love [xyz].m4a" to track("宇多田ヒカル/First Love/First Love [xyz].m4a", "First Love", "宇多田ヒカル", 180_000),
        )
        val written = M3u8.write(entries, known::get)
        assertEquals(
            """
                #EXTM3U
                #EXTINF:213,Björk - Human Behaviour
                ../Björk/Début/Human Behaviour [abc123].opus
                #EXTINF:180,宇多田ヒカル - First Love
                ../宇多田ヒカル/First Love/First Love [xyz].m4a
                #EXTINF:99,Gone - Missing Song
                ../Gone/Somewhere/Missing Song [gone1].opus
                #EXTINF:213,Björk - Human Behaviour
                ../Björk/Début/Human Behaviour [abc123].opus
            """.trimIndent() + "\n",
            written,
        )
        assertEquals(entries.map { it.path }, M3u8.parse(written, root).map { it.path })
        assertEquals(false, written.contains('\r'))
        assertEquals(false, written.startsWith("\uFEFF"))
    }

    @Test
    fun readerAcceptsBomCrlfCommentsBlankLinesAndAbsolutePaths() {
        val text = "\uFEFF#EXTM3U\r\n#PLAYLIST:Ignored\r\n\r\n$root/A/B/Song.mp3\r\n./../A/B/Other.flac\r\n/home/user/Music/TuneMesh/A/B/Song.mp3\r\n"
        val entries = M3u8.parse(text, root)
        assertEquals(listOf("A/B/Song.mp3", "A/B/Other.flac", null), entries.map { it.path })
        assertNull(entries[0].extinf)
    }

    @Test
    fun entriesOutsideTheLibraryAreKeptVerbatim() {
        val entries = M3u8.parse("#EXTM3U\n../../outside.mp3\n/home/user/Music/TuneMesh/A/B/Song.mp3\n", root)
        assertEquals(listOf(null, null), entries.map { it.path })
        assertEquals(
            "#EXTM3U\n#EXTINF:-1,outside\n../../outside.mp3\n#EXTINF:-1,Song\n/home/user/Music/TuneMesh/A/B/Song.mp3\n",
            M3u8.write(entries) { null },
        )
    }

    @Test
    fun unknownDurationIsMinusOne() {
        val t = track("A/B/x.opus", "x", "A", 0)
        assertEquals("#EXTINF:-1,A - x", M3u8.extinf(t))
    }

    /** As tunemesh-core's `serialize`: an entry holding a CR (only a lone CR survives parsing) is dropped. */
    @Test
    fun entriesWithLineBreaksAreDroppedOnWrite() {
        val text = "#EXTM3U\r\n#EXTINF:5,Ok\r\n../A/B/ok.mp3\r\n#EXTINF:6,Bro\rken\n../A/B/bro\rken.mp3\n" +
            "#EXTINF:7,Gone\r\r\n../Gone/x\ry/missing.mp3\n../A/B/last.mp3\r\r\n"
        val entries = M3u8.parse(text, root)
        assertEquals(
            listOf("../A/B/ok.mp3", "../A/B/bro\rken.mp3", "../Gone/x\ry/missing.mp3", "../A/B/last.mp3"),
            entries.map { it.raw },
        )
        assertEquals(
            "#EXTM3U\n#EXTINF:5,Ok\n../A/B/ok.mp3\n#EXTINF:-1,last\n../A/B/last.mp3\n",
            M3u8.write(entries) { null },
        )
        // A kept entry's old #EXTINF never carries a CR into the file either.
        val label = M3u8.parse("#EXTINF:3,Two\rParts\n../A/B/c.mp3\n", root)
        assertEquals("#EXTM3U\n#EXTINF:3,Two Parts\n../A/B/c.mp3\n", M3u8.write(label) { null })
    }

    /** As tunemesh-core's `resolve`, lines are resolved exactly as written: spaces are part of the name. */
    @Test
    fun linesAreNotTrimmedBeforeResolving() {
        assertEquals("Playlists/  ../A/B/x.mp3", M3u8.resolve("  ../A/B/x.mp3", root))
        assertEquals("A/B/x.mp3 ", M3u8.resolve("../A/B/x.mp3 ", root))
        assertEquals("Playlists/\t../x.mp3", M3u8.resolve("\t../x.mp3", root))
        // A leading space makes an absolute path relative to Playlists/, as in the core.
        assertEquals("Playlists/ $root/A/x.mp3", M3u8.resolve(" $root/A/x.mp3", root))
        val entries = M3u8.parse("#EXTM3U\n  ../A/B/x.mp3\n../A/B/y.mp3  \r\n", root)
        assertEquals(listOf("Playlists/  ../A/B/x.mp3", "A/B/y.mp3  "), entries.map { it.path })
        assertEquals(listOf("  ../A/B/x.mp3", "../A/B/y.mp3  "), entries.map { it.raw })
    }

    @Test
    fun resolveMatchesTheCoreForEveryShape() {
        val cases = mapOf(
            "../A/B/x.mp3" to "A/B/x.mp3",
            "A/x.mp3" to "Playlists/A/x.mp3",
            "./../A//B/./x.mp3" to "A/B/x.mp3",
            "../A/B/x.mp3/" to "A/B/x.mp3",
            "../../x.mp3" to null,
            ".." to null,
            "../" to null,
            "$root/A/x.mp3" to "A/x.mp3",
            "$root//A/../x.mp3" to "x.mp3",
            "$root/" to null,
            "${root}x/A.mp3" to null,
            "/elsewhere/A.mp3" to null,
            "../.hidden/..a/x.mp3" to ".hidden/..a/x.mp3",
        )
        for ((line, path) in cases) assertEquals(line, path, M3u8.resolve(line, root))
        assertEquals("A/x.mp3", M3u8.resolve("/A/x.mp3", "/"))
    }

    @Test
    fun canonicalEntriesKeepNoSecondCopyOfTheirLine() {
        val shared = "A/B/x.mp3"
        val entries = ArrayList<PlaylistEntry>()
        M3u8.read(
            "#EXTM3U\n#EXTINF:3,A - x\n../A/B/x.mp3\n./../A/B/x.mp3\n".byteInputStream(),
            root,
            keepExtinf = false,
            canonical = { if (it == shared) shared else null },
        ) { entries += it }
        assertSame(shared, entries[0].path)
        assertSame(shared, entries[1].path)
        assertEquals("../A/B/x.mp3", entries[0].raw)
        assertEquals("./../A/B/x.mp3", entries[1].raw)
        assertNull(entries[0].extinf)
        // The line is derived from the path, not stored: the field holding it is null.
        val line = PlaylistEntry::class.java.getDeclaredField("line").apply { isAccessible = true }
        assertNull(line.get(entries[0]))
        assertEquals("./../A/B/x.mp3", line.get(entries[1]))
    }

    @Test
    fun limitsRefuseInsteadOfTruncating() {
        fun read(text: String, maxBytes: Long = Limits.MAX_PLAYLIST_BYTES, maxEntries: Int = Limits.MAX_PLAYLIST_ENTRIES): Int {
            var count = 0
            M3u8.read(text.byteInputStream(), root, keepExtinf = false, maxBytes = maxBytes, maxEntries = maxEntries) { count++ }
            return count
        }
        val three = "#EXTM3U\n../a.mp3\n#comment\n\n../b.mp3\n../c.mp3"
        assertEquals(3, read(three, maxEntries = 3))
        assertThrows(PlaylistTooLargeException::class.java) { read(three, maxEntries = 2) }
        assertEquals(3, read(three, maxBytes = three.length.toLong()))
        assertThrows(PlaylistTooLargeException::class.java) { read(three, maxBytes = three.length - 1L) }
        val longLine = "../" + "x".repeat(Limits.MAX_PLAYLIST_LINE_BYTES - 3)
        assertEquals(1, read("$longLine\n"))
        assertThrows(PlaylistTooLargeException::class.java) { read("${longLine}y\n") }
    }

    /** Lines split across read buffers, multi-byte characters and a final line without LF. */
    @Test
    fun streamingMatchesWholeTextParsing() {
        val text = buildString {
            append("\uFEFF#EXTM3U\r\n")
            repeat(20_000) { append("#EXTINF:$it,Sigur Rós - ☃ $it\r\n../Sigur Rós/Ágætis byrjun/Song $it 🎵.opus\r\n") }
            append("../last.mp3")
        }
        val entries = M3u8.parse(text, root)
        assertEquals(20_001, entries.size)
        assertEquals("Sigur Rós/Ágætis byrjun/Song 19999 🎵.opus", entries[19_999].path)
        assertEquals("#EXTINF:19999,Sigur Rós - ☃ 19999", entries[19_999].extinf)
        assertEquals("last.mp3", entries.last().path)
    }

    /** docs/library-format.md: the 32 KiB line limit counts bytes without the line ending (LF and CRs before it). */
    @Test
    fun lineLimitLeavesOutTheLineEnding() {
        fun count(text: String): Int {
            var n = 0
            M3u8.read(text.byteInputStream(), root, keepExtinf = false) { n++ }
            return n
        }
        val full = "../" + "é".repeat((Limits.MAX_PLAYLIST_LINE_BYTES - 3) / 2) + "x".repeat((Limits.MAX_PLAYLIST_LINE_BYTES - 3) % 2)
        assertEquals(Limits.MAX_PLAYLIST_LINE_BYTES, full.toByteArray().size)
        assertEquals(1, count("$full\n"))
        assertEquals(1, count("$full\r\n"))
        assertEquals(2, count("$full\r\r\r\n../b.mp3"))
        assertEquals(1, count("$full\r"))
        assertThrows(PlaylistTooLargeException::class.java) { count("${full}x\r\n") }
        assertThrows(PlaylistTooLargeException::class.java) { count("$full\rx\n") }
        assertThrows(PlaylistTooLargeException::class.java) { count("$full\r\rx") }
    }

    @Test
    fun writerFollowsTheCore() {
        val entries = M3u8.parse(
            "#EXTM3U\n#EXTINF: 12 ,  Kept label  \n$root/Gone/x.mp3\n#EXTINF:abc,Bad duration\n../Gone/y.mp3\n" +
                "#EXTINF:\u0663,Arabic digit\n../Gone/z.mp3\n#EXTINF:5\n../Gone/No Label [id].mp3\n" +
                "#EXTINF:99999999999,Long\n/elsewhere/w.mp3\n",
            root,
        )
        assertEquals(
            "#EXTM3U\n#EXTINF:12,Kept label\n../Gone/x.mp3\n#EXTINF:-1,Bad duration\n../Gone/y.mp3\n" +
                "#EXTINF:-1,Arabic digit\n../Gone/z.mp3\n#EXTINF:5,No Label\n../Gone/No Label [id].mp3\n" +
                "#EXTINF:2147483646,Long\n/elsewhere/w.mp3\n",
            M3u8.write(entries) { null },
        )
    }

    @Test
    fun labelsAreCutToOneKibibyteAtACharacterBoundary() {
        val long = track("A/x.mp3", "é".repeat(600), "A", 1_000)
        val line = M3u8.extinf(long)
        val label = line.substringAfter(',')
        assertEquals(Limits.MAX_LABEL_BYTES, label.toByteArray().size)
        assertEquals("A - " + "é".repeat(510), label)
        assertEquals("A - " + "é".repeat(510), M3u8.cleanLabel("A - " + "é".repeat(511)))
        assertEquals("ab" + "é".repeat(511), M3u8.cleanLabel("ab" + "é".repeat(512)))
        assertEquals("a b c", M3u8.cleanLabel("a\nb\rc"))
        assertEquals(1023, M3u8.cleanLabel("a" + "é".repeat(600)).toByteArray().size)
        val kept = M3u8.parse("#EXTINF:1,${"🎵".repeat(300)}\n../Gone/a.mp3\n", root)
        val written = M3u8.write(kept) { null }.lines()[1].substringAfter(',')
        assertEquals("🎵".repeat(256), written)
    }

    @Test
    fun overLimitPlaylistsAreNotWritten() {
        val entry = M3u8.entryFor(track("A/x.mp3", "x", "A", 1_000))
        assertEquals(Limits.MAX_PLAYLIST_ENTRIES, M3u8.write(List(Limits.MAX_PLAYLIST_ENTRIES) { entry }) { null }.lines().count { it == "../A/x.mp3" })
        assertThrows(PlaylistTooLargeException::class.java) { M3u8.write(List(Limits.MAX_PLAYLIST_ENTRIES + 1) { entry }) { null } }
        val longPath = "A/" + "x".repeat(Limits.MAX_PLAYLIST_LINE_BYTES - 5)
        assertEquals(Limits.MAX_PLAYLIST_LINE_BYTES, M3u8.entryLine(longPath).toByteArray().size)
        M3u8.write(listOf(PlaylistEntry(longPath, M3u8.entryLine(longPath)))) { null }
        assertThrows(PlaylistTooLargeException::class.java) {
            M3u8.write(listOf(PlaylistEntry(longPath + "x", M3u8.entryLine(longPath + "x")))) { null }
        }
    }

    /** R2-astra-1: a long entry without metadata gets a fallback label cut like any other, so it reads back. */
    @Test
    fun longEntriesWithoutMetadataRoundTrip() {
        val line = "../" + "x".repeat(32765)
        val entries = M3u8.parse("$line\n", "/r")
        assertEquals(1, entries.size)
        val written = M3u8.write(entries) { null }
        assertTrue(written.lines().all { it.toByteArray().size <= Limits.MAX_PLAYLIST_LINE_BYTES })
        assertEquals("#EXTINF:-1,${"x".repeat(Limits.MAX_LABEL_BYTES)}", written.lines()[1])
        var reread = 0
        M3u8.read(written.byteInputStream(), "/r", keepExtinf = true) { assertEquals(line, it.raw); reread++ }
        assertEquals(1, reread)
    }

    @Test
    fun everyWrittenLineIsCheckedAgainstTheLimit() {
        val outside = "/elsewhere/" + "y".repeat(Limits.MAX_PLAYLIST_LINE_BYTES)
        val entry = PlaylistEntry(null, outside)
        assertThrows(PlaylistTooLargeException::class.java) { M3u8.write(listOf(entry)) { null } }
        val fits = PlaylistEntry(null, outside.take(Limits.MAX_PLAYLIST_LINE_BYTES))
        assertEquals(outside.take(Limits.MAX_PLAYLIST_LINE_BYTES), M3u8.write(listOf(fits)) { null }.lines()[2])
    }
}
