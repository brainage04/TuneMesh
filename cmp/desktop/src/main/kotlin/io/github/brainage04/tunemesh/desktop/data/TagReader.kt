package io.github.brainage04.tunemesh.desktop.data

import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.RawTags
import org.jaudiotagger.audio.AudioFileIO
import org.jaudiotagger.tag.FieldKey
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.EOFException
import java.io.File
import java.io.RandomAccessFile
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.Base64
import java.util.logging.Level
import java.util.logging.Logger

/**
 * Tags and embedded pictures. Ogg (Opus, Vorbis) is parsed here: jaudiotagger has no Opus support, and the
 * worker keeps YouTube's Opus streams as they are. Everything else (M4A, MP3, FLAC, WAV) goes to jaudiotagger.
 */
object TagReader {
    // java.util.logging keeps loggers weakly: hold this one, or its level is lost and jaudiotagger logs every file.
    private val quietLogger: Logger = Logger.getLogger("org.jaudiotagger").apply { level = Level.OFF }

    private val oggExtensions = setOf("opus", "ogg", "oga")

    fun read(file: File): RawTags = runCatching {
        if (LibraryFormat.extension(file.name) in oggExtensions) Ogg.read(file, wantPicture = false).tags else readOther(file)
    }.getOrElse { RawTags() }

    fun picture(file: File): ByteArray? = runCatching {
        if (LibraryFormat.extension(file.name) in oggExtensions) {
            Ogg.read(file, wantPicture = true).picture
        } else {
            AudioFileIO.read(file).tag?.firstArtwork?.binaryData
        }
    }.getOrNull()

    private fun readOther(file: File): RawTags {
        val audio = AudioFileIO.read(file)
        val tag = audio.tag
        fun text(key: FieldKey) = runCatching { tag?.getFirst(key) }.getOrNull()?.trim()?.takeIf { it.isNotEmpty() }
        return RawTags(
            title = text(FieldKey.TITLE),
            artist = tag?.let { t -> runCatching { t.getAll(FieldKey.ARTIST) }.getOrNull()?.filter { it.isNotBlank() }?.joinToString("; ") }
                ?.takeIf { it.isNotEmpty() },
            album = text(FieldKey.ALBUM),
            albumArtist = text(FieldKey.ALBUM_ARTIST),
            trackNumber = text(FieldKey.TRACK)?.leadingInt(),
            discNumber = text(FieldKey.DISC_NO)?.leadingInt(),
            durationMs = (audio.audioHeader.preciseTrackLength * 1000).toLong(),
        )
    }

    private fun String.leadingInt(): Int? = substringBefore('/').trim().toIntOrNull()

    private class OggResult(val tags: RawTags, val picture: ByteArray?)

    private object Ogg {
        fun read(file: File, wantPicture: Boolean): OggResult {
            RandomAccessFile(file, "r").use { raf ->
                val input = DataInputStream(java.io.BufferedInputStream(java.io.FileInputStream(raf.fd), 16 * 1024))
                val packets = PacketReader(input)
                val head = packets.next() ?: return OggResult(RawTags(), null)
                val serial = packets.serial
                val isOpus = head.startsWith("OpusHead")
                val preSkip = if (isOpus) ByteBuffer.wrap(head, 10, 2).order(ByteOrder.LITTLE_ENDIAN).short.toLong() and 0xffff else 0
                val rate = if (isOpus) 48_000L else ByteBuffer.wrap(head, 12, 4).order(ByteOrder.LITTLE_ENDIAN).int.toLong()
                val comments = packets.next() ?: return OggResult(RawTags(), null)
                val offset = if (isOpus) 8 else 7
                val fields = parseComments(comments, offset, wantPicture)
                val granule = lastGranule(raf, serial)
                val durationMs = if (granule > 0 && rate > 0) ((granule - preSkip).coerceAtLeast(0) * 1000 / rate) else 0
                fun first(vararg keys: String) = keys.firstNotNullOfOrNull { key -> fields[key]?.firstOrNull { it.isNotBlank() } }
                val tags = RawTags(
                    title = first("TITLE"),
                    artist = fields["ARTIST"]?.filter { it.isNotBlank() }?.joinToString("; ")?.takeIf { it.isNotEmpty() },
                    album = first("ALBUM"),
                    albumArtist = first("ALBUMARTIST", "ALBUM_ARTIST", "ALBUM ARTIST"),
                    trackNumber = first("TRACKNUMBER", "TRACK")?.leadingInt(),
                    discNumber = first("DISCNUMBER", "DISC")?.leadingInt(),
                    durationMs = durationMs,
                )
                val picture = if (wantPicture) fields["METADATA_BLOCK_PICTURE"]?.firstOrNull()?.let(::flacPicture) else null
                return OggResult(tags, picture)
            }
        }

        private fun ByteArray.startsWith(prefix: String) = size >= prefix.length && prefix.indices.all { this[it] == prefix[it].code.toByte() }

        private fun parseComments(packet: ByteArray, offset: Int, wantPicture: Boolean): Map<String, List<String>> {
            val buffer = ByteBuffer.wrap(packet).order(ByteOrder.LITTLE_ENDIAN)
            buffer.position(offset)
            val vendor = buffer.int
            buffer.position(buffer.position() + vendor)
            val count = buffer.int
            val fields = HashMap<String, MutableList<String>>()
            repeat(count) {
                val length = buffer.int
                if (length < 0 || length > buffer.remaining()) return fields
                val start = buffer.position()
                var eq = start
                while (eq < start + length && packet[eq] != '='.code.toByte()) eq++
                val key = String(packet, start, eq - start, Charsets.UTF_8).uppercase()
                if (eq < start + length && (wantPicture || key != "METADATA_BLOCK_PICTURE")) {
                    fields.getOrPut(key) { mutableListOf() } += String(packet, eq + 1, start + length - eq - 1, Charsets.UTF_8)
                }
                buffer.position(start + length)
            }
            return fields
        }

        private fun flacPicture(base64: String): ByteArray? {
            val block = ByteBuffer.wrap(Base64.getDecoder().decode(base64.trim()))
            block.int // picture type
            repeat(2) { // MIME type, description
                val length = block.int
                block.position(block.position() + length)
            }
            block.position(block.position() + 16) // width, height, depth, colours
            val length = block.int
            return ByteArray(length).also { block.get(it) }
        }

        /** Granule position of the stream's last page, found in the file's last 64 KiB. */
        private fun lastGranule(raf: RandomAccessFile, serial: Int): Long {
            val size = raf.length()
            val tailSize = minOf(size, 64L * 1024).toInt()
            val tail = ByteArray(tailSize)
            raf.seek(size - tailSize)
            raf.readFully(tail)
            var i = tailSize - 27
            while (i >= 0) {
                if (tail[i] == 'O'.code.toByte() && tail[i + 1] == 'g'.code.toByte() && tail[i + 2] == 'g'.code.toByte() && tail[i + 3] == 'S'.code.toByte()) {
                    val page = ByteBuffer.wrap(tail, i, 27).order(ByteOrder.LITTLE_ENDIAN)
                    val granule = page.getLong(i + 6)
                    val pageSerial = page.getInt(i + 14)
                    if (pageSerial == serial && granule >= 0) return granule
                }
                i--
            }
            return 0
        }
    }

    /** Reassembles packets from the pages of the first logical stream. */
    private class PacketReader(private val input: DataInputStream) {
        var serial = 0
            private set
        private var started = false
        private val lacing = ArrayDeque<Int>()

        fun next(): ByteArray? {
            val packet = ByteArrayOutputStream()
            while (true) {
                if (lacing.isEmpty() && !readPage()) return null
                while (lacing.isNotEmpty()) {
                    val length = lacing.removeFirst()
                    val bytes = ByteArray(length)
                    input.readFully(bytes)
                    packet.write(bytes)
                    if (length < 255) return packet.toByteArray()
                }
            }
        }

        private fun readPage(): Boolean {
            val header = ByteArray(27)
            try {
                input.readFully(header)
            } catch (_: EOFException) {
                return false
            }
            if (String(header, 0, 4, Charsets.ISO_8859_1) != "OggS") return false
            val pageSerial = ByteBuffer.wrap(header, 14, 4).order(ByteOrder.LITTLE_ENDIAN).int
            if (!started) {
                serial = pageSerial
                started = true
            }
            val segments = header[26].toInt() and 0xff
            val table = ByteArray(segments).also { input.readFully(it) }
            if (pageSerial != serial) {
                input.skipNBytes(table.sumOf { it.toInt() and 0xff }.toLong())
                return true
            }
            table.forEach { lacing.addLast(it.toInt() and 0xff) }
            return true
        }
    }
}
