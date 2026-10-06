package io.github.brainage04.tunemesh.library

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
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
        val text = "\uFEFF#EXTM3U\r\n#PLAYLIST:Ignored\r\n\r\n$root/A/B/Song.mp3\r\n./../A/B/Other.flac\r\n/home/thomas/Music/TuneMesh/A/B/Song.mp3\r\n"
        val entries = M3u8.parse(text, root)
        assertEquals(listOf("A/B/Song.mp3", "A/B/Other.flac", null), entries.map { it.path })
        assertNull(entries[0].extinf)
    }

    @Test
    fun entriesOutsideTheLibraryAreKeptVerbatim() {
        val entries = M3u8.parse("#EXTM3U\n../../outside.mp3\n/home/thomas/Music/TuneMesh/A/B/Song.mp3\n", root)
        assertEquals(listOf(null, null), entries.map { it.path })
        assertEquals(
            "#EXTM3U\n#EXTINF:-1,outside\n../../outside.mp3\n#EXTINF:-1,Song\n/home/thomas/Music/TuneMesh/A/B/Song.mp3\n",
            M3u8.write(entries) { null },
        )
    }

    @Test
    fun unknownDurationIsMinusOne() {
        val t = track("A/B/x.opus", "x", "A", 0)
        assertEquals("#EXTINF:-1,A - x", M3u8.extinf(t))
    }
}
