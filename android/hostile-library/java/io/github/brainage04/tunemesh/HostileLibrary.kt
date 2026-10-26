package io.github.brainage04.tunemesh

import android.graphics.Bitmap
import java.io.BufferedOutputStream
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.FileOutputStream
import java.io.OutputStream
import java.util.Locale
import kotlin.random.Random

/**
 * Builds the hostile library of `docs/library-format.md` "Limits" on the device, in the shape of tunemesh-core's `hostile_library` example
 * (same names), scaled by [Scale]. Every file is generated; nothing is downloaded.
 */
class HostileLibrary(private val root: File, private val scale: Scale) {
    /** Sizes of the generated library. [FULL] is the core generator's full profile. */
    data class Scale(
        val albumTracks: Int,
        val audioFiles: Int,
        val bigPlaylists: Int,
        val smallPlaylists: Int,
        val statuses: Int,
        val requests: Int,
    ) {
        companion object {
            val FULL =
                Scale(albumTracks = 40_000, audioFiles = 100_000, bigPlaylists = 50, smallPlaylists = 1_050, statuses = 500, requests = 600)
            val SMALL =
                Scale(albumTracks = 4_000, audioFiles = 10_000, bigPlaylists = 3, smallPlaylists = 1_050, statuses = 20, requests = 600)
        }
    }

    /** What a client should end up with (the core generator's expectations, for this scale). */
    data class Expected(val tracks: Int, val albums: Int, val playlistsListed: Int, val tooLarge: Int, val downloads: Int)

    val expected: Expected
        get() {
            val otherAlbums = (scale.audioFiles - scale.albumTracks + 9) / 10
            return Expected(
                // + big tag, big cover, small cover, the track at level 10 (the one at level 200 is too deep)
                tracks = scale.audioFiles + 4,
                albums = 1 + otherAlbums + 4,
                playlistsListed = minOf(1_000, scale.bigPlaylists + scale.smallPlaylists + 4),
                tooLarge = 3,
                downloads = 200,
            )
        }

    fun generate() {
        root.deleteRecursively()
        root.mkdirs()
        audio()
        huge()
        deep()
        playlists()
        downloads()
        // 2 MiB: left unchanged, never read.
        write(".stignore") { out -> repeat(2 * 1024 * 1024 / 16) { out.write("// hostile rule\n".toByteArray()) } }
    }

    private fun audio() {
        val random = Random(1)
        val order = (1..scale.albumTracks).shuffled(random)
        for (i in 0 until scale.albumTracks) {
            val n = order[i]
            writeBytes(
                "Hostile Artist/One Album/%05d.mp3".format(Locale.ROOT, i),
                mp3(title = "Track $n", artist = "Hostile Artist", album = "One Album", track = n),
            )
        }
        var made = scale.albumTracks
        var album = 0
        while (made < scale.audioFiles) {
            val artist = "Artist %04d".format(Locale.ROOT, album / 10)
            val albumName = "Album %02d".format(Locale.ROOT, album % 10)
            for (t in 1..10) {
                if (made == scale.audioFiles) break
                writeBytes(
                    "$artist/$albumName/%02d.mp3".format(Locale.ROOT, t),
                    mp3(title = "Song $t", artist = artist, album = albumName, track = t),
                )
                made++
            }
            album++
        }
    }

    private fun huge() {
        val big = "x".repeat(1024 * 1024)
        val artists = (1..40).joinToString("\u0000") { "Artist $it" }
        writeBytes("Huge/Tags/big tag.mp3", mp3(title = big, artist = artists, album = big, track = 1))
        val cover = hugeJpeg(20_000_000)
        writeBytes("Huge/Cover/big cover.mp3", mp3(title = "Big Cover", artist = "Huge", album = "Cover", track = 1, picture = cover))
        writeBytes("Huge/Cover/cover.jpg", cover)
        writeBytes(
            "Huge/Small Cover/small cover.mp3",
            mp3(title = "Small Cover", artist = "Huge", album = "Small Cover", track = 1, picture = smallJpeg()),
        )
    }

    private fun deep() {
        var path = "Deep"
        for (level in 1..200) {
            path += "/d%03d".format(Locale.ROOT, level)
            if (level == 10 || level == 200) {
                writeBytes("$path/deep.mp3", mp3(title = "Level $level", artist = "Deep", album = "Level $level", track = 1))
            }
        }
    }

    private fun playlists() {
        val entry = { i: Int -> "../Hostile Artist/One Album/%05d.mp3\n".format(Locale.ROOT, i % scale.albumTracks).toByteArray() }
        for (p in 0 until scale.bigPlaylists) {
            write("Playlists/Big %02d.m3u8".format(Locale.ROOT, p)) { out ->
                out.write("#EXTM3U\n".toByteArray())
                repeat(250_000) { out.write(entry(it)) }
            }
        }
        for (p in 0 until scale.smallPlaylists) {
            write("Playlists/Small %04d.m3u8".format(Locale.ROOT, p)) { out -> repeat(5) { out.write(entry(p * 5 + it)) } }
        }
        write("Playlists/Over Entries.m3u8") { out -> repeat(250_001) { out.write(entry(it)) } }
        write("Playlists/Over Line.m3u8") { out -> out.write(("../" + "x".repeat(32 * 1024 - 2) + "\n").toByteArray()) }
        write("Playlists/Over Bytes.m3u8") { out ->
            val comment = ("#" + "c".repeat(30 * 1024) + "\n").toByteArray()
            repeat(32 * 1024 * 1024 / comment.size + 2) { out.write(comment) }
        }
        write("Playlists/Long Entry.m3u8") { out -> out.write(("../" + "x".repeat(32765) + "\n").toByteArray()) }
        // Every entry with an #EXTINF label over the 1 KiB limit, the file just under 32 MiB.
        writeBytes("Playlists/Long Labels.m3u8", HostileFiles.labelPlaylist().toByteArray())
    }

    /**
     * Requests with every field long, the newest 210 just under 1 MiB and older ones of about 61 KB; statuses alternating between every
     * text field long and many small entries ([HostileFiles]); a request and a status over 1 MiB, one with an over-long input; a
     * `worker.json` with every field long.
     */
    private fun downloads() {
        for (i in 0 until scale.requests) {
            val id = "${1_700_000_000_000L + i}-%08x".format(Locale.ROOT, i)
            val bytes = if (i >= scale.requests - 210) HostileFiles.FILE_BYTES else 61 * 1024
            writeBytes(".tunemesh/requests/$id.json", HostileFiles.request(id, bytes).toByteArray())
        }
        for (i in 10_000 until 10_000 + scale.statuses) {
            val id = "${1_700_000_000_000L + i}-%08x".format(Locale.ROOT, i)
            val text = if (i % 2 == 0) HostileFiles.longStatus(id) else HostileFiles.manyStatus(id)
            writeBytes(".tunemesh/status/$id.json", text.toByteArray())
        }
        write(".tunemesh/requests/${1_700_000_900_000L}-hostile0.json") { out -> padded(out, 2 * 1024 * 1024) }
        val longInput = "y".repeat(9 * 1024)
        writeBytes(
            ".tunemesh/requests/${1_700_000_900_001L}-hostile1.json",
            """{"version":1,"id":"x","input":"$longInput","playlist":null,"playlistFromSource":false,"requestedBy":"h","requestedAt":"x"}"""
                .toByteArray(),
        )
        write(".tunemesh/status/${1_700_000_900_002L}-hostile2.json") { out -> padded(out, 2 * 1024 * 1024) }
        writeBytes(".tunemesh/worker.json", HostileFiles.worker().toByteArray())
    }

    private fun padded(out: OutputStream, bytes: Int) {
        val head = """{"version":1,"device":"hostile","heartbeatAt":"2026-10-20T00:00:00Z","id":"x","input":"x","pad":""""
        out.write(head.toByteArray())
        val chunk = ByteArray(64 * 1024) { 'p'.code.toByte() }
        var left = bytes - head.length - 2
        while (left > 0) {
            val n = minOf(left, chunk.size)
            out.write(chunk, 0, n)
            left -= n
        }
        out.write("\"}".toByteArray())
    }

    private fun writeBytes(path: String, bytes: ByteArray) = write(path) { it.write(bytes) }

    private fun write(path: String, body: (OutputStream) -> Unit) {
        val file = File(root, path)
        file.parentFile!!.mkdirs()
        BufferedOutputStream(FileOutputStream(file), 64 * 1024).use(body)
    }

    companion object {
        /** An MP3: an ID3v2.4 tag (UTF-8 text frames, optional APIC) and a few silent MPEG-1 Layer III frames. */
        fun mp3(title: String, artist: String, album: String, track: Int, picture: ByteArray? = null): ByteArray {
            val frames = ByteArrayOutputStream()
            fun frame(id: String, body: ByteArray) {
                frames.write(id.toByteArray())
                frames.write(synchsafe(body.size))
                frames.write(byteArrayOf(0, 0))
                frames.write(body)
            }
            fun text(id: String, value: String) = frame(id, byteArrayOf(3) + value.toByteArray(Charsets.UTF_8))
            text("TIT2", title)
            text("TPE1", artist)
            text("TALB", album)
            text("TRCK", track.toString())
            if (picture != null) frame("APIC", byteArrayOf(0) + "image/jpeg".toByteArray() + byteArrayOf(0, 3, 0) + picture)
            val tag = frames.toByteArray()
            val out = ByteArrayOutputStream(10 + tag.size + 4 * 417)
            out.write("ID3".toByteArray())
            out.write(byteArrayOf(4, 0, 0))
            out.write(synchsafe(tag.size))
            out.write(tag)
            // 128 kbit/s, 44.1 kHz, no padding: 417 bytes per frame.
            repeat(4) {
                out.write(byteArrayOf(0xFF.toByte(), 0xFB.toByte(), 0x90.toByte(), 0x64))
                out.write(ByteArray(413))
            }
            return out.toByteArray()
        }

        private fun synchsafe(size: Int) =
            byteArrayOf(
                (size shr 21 and 0x7F).toByte(),
                (size shr 14 and 0x7F).toByte(),
                (size shr 7 and 0x7F).toByte(),
                (size and 0x7F).toByte(),
            )

        /** [bytes] of JPEG whose header states 20,000 x 20,000 pixels (the rest is padding). */
        fun hugeJpeg(bytes: Int): ByteArray {
            val out = ByteArray(bytes)
            val sof =
                byteArrayOf(
                    0xFF.toByte(),
                    0xD8.toByte(),
                    0xFF.toByte(),
                    0xC0.toByte(),
                    0,
                    11,
                    8,
                    (20_000 shr 8).toByte(),
                    (20_000 and 0xFF).toByte(),
                    (20_000 shr 8).toByte(),
                    (20_000 and 0xFF).toByte(),
                    1,
                    1,
                    0x11,
                    0,
                )
            sof.copyInto(out)
            out[bytes - 2] = 0xFF.toByte()
            out[bytes - 1] = 0xD9.toByte()
            return out
        }

        /** A real 500 x 500 JPEG, which must be shown. */
        fun smallJpeg(): ByteArray {
            val bitmap = Bitmap.createBitmap(500, 500, Bitmap.Config.ARGB_8888)
            bitmap.eraseColor(0xFF1ED760.toInt())
            return ByteArrayOutputStream().use { out ->
                bitmap.compress(Bitmap.CompressFormat.JPEG, 80, out)
                out.toByteArray()
            }
        }
    }
}
