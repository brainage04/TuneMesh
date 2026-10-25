package io.github.brainage04.tunemesh.library

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DownloadInputsTest {
    @Test
    fun linesAreTrimmedBlankSkippedAndDeduplicated() {
        assertEquals(
            listOf("https://a.example/1", "artist – song", "https://a.example/2"),
            DownloadInputs.lines("  https://a.example/1 \r\n\n\tartist – song\nhttps://a.example/1\n https://a.example/2\n   \n"),
        )
        assertEquals(emptyList<String>(), DownloadInputs.lines(" \n "))
    }

    @Test
    fun kinds() {
        val cases =
            mapOf(
                "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=abc" to InputKind.Playlist,
                "https://open.spotify.com/intl-de/playlist/37i9dQZF1DXcBWIGoYBM5M" to InputKind.Playlist,
                "https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy" to InputKind.Album,
                "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC" to InputKind.Single,
                "https://open.spotify.com/artist/0gxyHStUsqpMadRV0Di1Qt" to InputKind.Single,
                "https://www.youtube.com/playlist?list=PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI" to InputKind.Playlist,
                "https://youtube.com/watch?v=jNQXAC9IVRw&list=PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI&index=2" to InputKind.Playlist,
                "https://m.youtube.com/playlist?list=PLx" to InputKind.Playlist,
                "https://music.youtube.com/playlist?list=RDCLAK5uy_kmPRjHDECIcuVwnKsx2Ng7fyNgFKWNJFs" to InputKind.Playlist,
                "https://music.youtube.com/playlist?list=OLAK5uy_nMr9h2VlS-2PULNz3M3XVXQj_P3C2bqaY" to InputKind.Album,
                "https://music.youtube.com/browse/MPREb_9nqEki4ZDpp" to InputKind.Album,
                "https://music.youtube.com/watch?v=jNQXAC9IVRw" to InputKind.Single,
                "https://youtu.be/jNQXAC9IVRw?list=PLx" to InputKind.Playlist,
                "https://youtu.be/jNQXAC9IVRw" to InputKind.Single,
                "https://soundcloud.com/a/sets/b" to InputKind.Single,
                "artist – song" to InputKind.Single,
                "ftp://youtube.com/playlist?list=PLx" to InputKind.Single,
                "" to InputKind.Single,
                // docs/library-format.md, "YouTube links": a video in a list downloads the list; mixes stay single.
                "https://www.youtube.com/watch?v=jNQXAC9IVRw&list=RDjNQXAC9IVRw" to InputKind.Single,
                "https://music.youtube.com/watch?v=x&list=RDAMVMx" to InputKind.Single,
                "https://music.youtube.com/watch?v=x&list=PLx&index=3" to InputKind.Playlist,
                "https://music.youtube.com/watch?v=x&list=OLAK5uy_x" to InputKind.Album,
                "https://music.youtube.com/playlist?list=RDCLAK5uy_k" to InputKind.Playlist,
                "https://youtu.be/jNQXAC9IVRw?si=a&list=PLx" to InputKind.Playlist,
                "https://youtu.be/jNQXAC9IVRw?list=RDx" to InputKind.Single,
                "HTTPS://WWW.YouTube.COM/watch?v=x&list=PLx" to InputKind.Playlist,
                "https://YOUTU.BE/x?list=PLx" to InputKind.Playlist,
                "https://www.m.youtube.com/watch?v=x&list=PLx" to InputKind.Playlist,
                "https://m.www.youtube.com/watch?v=x&list=PLx" to InputKind.Single,
                "https://www.youtube.com/watch?v=x&list=" to InputKind.Single,
                "https://www.youtube.com/watch?v=x&playlist=PLx" to InputKind.Single,
                "https://www.youtube.com/watch?v=x&List=PLx" to InputKind.Single,
                "https://www.youtube.com/watch?v=x&list=pl%20x" to InputKind.Playlist,
                "https://www.youtube.com/watch?v=x#list=PLx" to InputKind.Single,
                "https://www.youtube.com/watch?v=x&list=PLx#t=10" to InputKind.Playlist,
                "https://youtu.be?list=PLx" to InputKind.Playlist,
                "https://music.youtube.com/browse/MPREb_x?list=RDx" to InputKind.Album,
                "https://notyoutube.com/watch?v=x&list=PLx" to InputKind.Single,
                "  https://youtu.be/x?list=PLx  " to InputKind.Playlist,
            )
        for ((line, kind) in cases) assertEquals(line, kind, DownloadInputs.kind(line))
    }

    @Test
    fun onlyPlaylistLinksTurnSavingOnByDefault() {
        assertTrue(DownloadInputs.saveAsPlaylistsByDefault(listOf("artist – song", "https://open.spotify.com/playlist/x")))
        assertFalse(
            DownloadInputs.saveAsPlaylistsByDefault(listOf("https://open.spotify.com/album/x", "https://music.youtube.com/browse/MPREbx"))
        )
        assertFalse(DownloadInputs.saveAsPlaylistsByDefault(emptyList()))
    }

    @Test
    fun playlistFromSourceOnlyForCollectionsWithoutATarget() {
        assertTrue(DownloadInputs.playlistFromSource("https://open.spotify.com/playlist/x", null, true))
        assertTrue(DownloadInputs.playlistFromSource("https://open.spotify.com/album/x", " ", true))
        assertFalse(DownloadInputs.playlistFromSource("https://open.spotify.com/track/x", null, true))
        assertFalse(DownloadInputs.playlistFromSource("https://open.spotify.com/playlist/x", null, false))
        assertFalse(DownloadInputs.playlistFromSource("https://open.spotify.com/playlist/x", "Road trip", true))
    }

    @Test
    fun sharedInputIsTheFirstLinkOrTheText() {
        assertEquals("https://youtu.be/x", DownloadInputs.sharedInput("Listen: https://youtu.be/x. And https://youtu.be/y"))
        assertEquals(
            "https://en.wikipedia.org/wiki/Song_(band)",
            DownloadInputs.sharedInput("(https://en.wikipedia.org/wiki/Song_(band))."),
        )
        assertEquals("https://a.example/[x]", DownloadInputs.sharedInput("https://a.example/[x]]}"))
        assertEquals("artist – song", DownloadInputs.sharedInput("  artist – song \n"))
        assertEquals("", DownloadInputs.sharedInput(""))
    }

    /** Each trailing bracket counted the whole link again (quadratic), on the main thread of the share sheet. */
    @Test(timeout = 5_000)
    fun trailingBracketsTakeLinearTime() {
        val brackets = ")".repeat(200_000)
        assertEquals("https://a.example/x", DownloadInputs.sharedInput("https://a.example/x$brackets"))
        assertEquals("https://a.example/(x)", DownloadInputs.sharedInput("https://a.example/(x)$brackets"))
    }
}
