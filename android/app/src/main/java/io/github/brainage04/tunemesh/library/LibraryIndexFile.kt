package io.github.brainage04.tunemesh.library

import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.nio.ByteBuffer
import java.util.zip.CRC32
import java.util.zip.CheckedInputStream
import java.util.zip.CheckedOutputStream

/**
 * The scanner's tag cache: a [LibraryIndex] saved as its arrays, so a start loads it without parsing (about 20 ms for 100,000 tracks
 * instead of 140 ms of JSON and its garbage). Layout: [MAGIC], [VERSION], the library root as UTF-8, the index's arrays in constructor
 * order (each its length, then its values big-endian), then the CRC-32 of all of that. A file of another version, for another root, cut
 * short or damaged reads as none, and the next scan reads the tags again.
 *
 * The cached tracks hold the values the app derived from the tags (titles from file names and so on); change [VERSION] when that derivation
 * changes, so the files are read again.
 */
internal object LibraryIndexFile {
    private const val MAGIC = 0x544d4958 // "TMIX"
    const val VERSION = 1
    private const val CHUNK = 64 * 1024
    private const val MAX_ROOT_BYTES = 64 * 1024

    @Suppress("MissingUseCall") // wraps the stream the caller closes
    fun write(index: LibraryIndex, root: String, output: OutputStream) {
        val buffered = BufferedOutputStream(output, CHUNK)
        val checksum = CRC32()
        val data = DataOutputStream(CheckedOutputStream(buffered, checksum))
        data.writeInt(MAGIC)
        data.writeInt(VERSION)
        val rootBytes = root.toByteArray(Charsets.UTF_8)
        data.writeInt(rootBytes.size)
        data.write(rootBytes)
        val writer = Writer(data)
        with(index) {
            writer.bytes(text)
            writer.ints(textEnds)
            writer.bytes(keys)
            writer.ints(keyEnds)
            writer.ints(trackPath)
            writer.ints(trackTitle)
            writer.ints(trackArtist)
            writer.ints(trackAlbum)
            writer.ints(trackNumbers)
            writer.ints(discNumbers)
            writer.bytes(trackFlags)
            writer.longs(durations)
            writer.longs(sizes)
            writer.longs(modified)
            writer.ints(pathSlots)
            writer.ints(byTitle)
            writer.ints(albumTitle)
            writer.ints(albumArtist)
            writer.ints(albumStart)
            writer.longs(albumAdded)
            writer.longs(albumDuration)
            writer.ints(albumSlots)
            writer.ints(recentAlbums)
            writer.ints(artistName)
            writer.ints(artistAlbumStart)
            writer.ints(artistAlbums)
            writer.ints(artistTrackEnds)
            writer.ints(artistSlots)
            writer.ints(recentArtistOrder)
        }
        data.flush()
        DataOutputStream(buffered).apply {
            writeInt(checksum.value.toInt())
            flush()
        }
    }

    /** The index saved for [root], or null when [input] holds none (another version or root, cut short, damaged). */
    @Suppress("MissingUseCall") // wraps the stream the caller closes
    fun read(input: InputStream, root: String, maxBytes: Long): LibraryIndex? {
        val buffered = BufferedInputStream(input, CHUNK)
        val checksum = CRC32()
        val data = DataInputStream(CheckedInputStream(buffered, checksum))
        return try {
            if (!readHeader(data, root)) null
            else {
                val index = readArrays(Reader(data, maxBytes))
                val expected = checksum.value.toInt()
                index.takeIf { DataInputStream(buffered).readInt() == expected && buffered.read() == -1 && isConsistent(it) }
            }
        } catch (_: IOException) {
            // Cut short (EOFException) or a length past the file.
            null
        }
    }

    /** Whether the file starts with this version's header for [root]. */
    private fun readHeader(data: DataInputStream, root: String): Boolean {
        if (data.readInt() != MAGIC || data.readInt() != VERSION) return false
        val rootLength = data.readInt()
        return rootLength in 0..MAX_ROOT_BYTES && String(ByteArray(rootLength).also(data::readFully), Charsets.UTF_8) == root
    }

    private fun readArrays(reader: Reader): LibraryIndex =
        LibraryIndex(
            text = reader.bytes(),
            textEnds = reader.ints(),
            keys = reader.bytes(),
            keyEnds = reader.ints(),
            trackPath = reader.ints(),
            trackTitle = reader.ints(),
            trackArtist = reader.ints(),
            trackAlbum = reader.ints(),
            trackNumbers = reader.ints(),
            discNumbers = reader.ints(),
            trackFlags = reader.bytes(),
            durations = reader.longs(),
            sizes = reader.longs(),
            modified = reader.longs(),
            pathSlots = reader.ints(),
            byTitle = reader.ints(),
            albumTitle = reader.ints(),
            albumArtist = reader.ints(),
            albumStart = reader.ints(),
            albumAdded = reader.longs(),
            albumDuration = reader.longs(),
            albumSlots = reader.ints(),
            recentAlbums = reader.ints(),
            artistName = reader.ints(),
            artistAlbumStart = reader.ints(),
            artistAlbums = reader.ints(),
            artistTrackEnds = reader.ints(),
            artistSlots = reader.ints(),
            recentArtistOrder = reader.ints(),
        )

    /** The array sizes fit together (the checksum covers the contents). */
    private fun isConsistent(index: LibraryIndex): Boolean =
        with(index) {
            val tracks = trackPath.size
            val albums = albumTitle.size
            val sizes =
                listOf(
                    trackTitle.size to tracks,
                    trackArtist.size to tracks,
                    trackAlbum.size to tracks,
                    trackNumbers.size to tracks,
                    discNumbers.size to tracks,
                    trackFlags.size to tracks,
                    durations.size to tracks,
                    this.sizes.size to tracks,
                    modified.size to tracks,
                    byTitle.size to tracks,
                    keyEnds.size to textEnds.size,
                    albumArtist.size to albums,
                    albumStart.size to albums + 1,
                    albumAdded.size to albums,
                    albumDuration.size to albums,
                    recentAlbums.size to albums,
                    artistAlbumStart.size to artistName.size + 1,
                    artistAlbums.size to albums,
                    artistTrackEnds.size to albums,
                    (textEnds.lastOrNull() ?: 0) to text.size,
                    (keyEnds.lastOrNull() ?: 0) to keys.size,
                )
            sizes.all { (size, expected) -> size == expected }
        }

    private class Writer(private val data: DataOutputStream) {
        private val chunk = ByteBuffer.allocate(CHUNK)

        fun bytes(values: ByteArray) {
            data.writeInt(values.size)
            data.write(values)
        }

        fun ints(values: IntArray) {
            data.writeInt(values.size)
            var from = 0
            while (from < values.size) {
                val count = minOf(values.size - from, CHUNK / Int.SIZE_BYTES)
                chunk.clear()
                chunk.asIntBuffer().put(values, from, count)
                data.write(chunk.array(), 0, count * Int.SIZE_BYTES)
                from += count
            }
        }

        fun longs(values: LongArray) {
            data.writeInt(values.size)
            var from = 0
            while (from < values.size) {
                val count = minOf(values.size - from, CHUNK / Long.SIZE_BYTES)
                chunk.clear()
                chunk.asLongBuffer().put(values, from, count)
                data.write(chunk.array(), 0, count * Long.SIZE_BYTES)
                from += count
            }
        }
    }

    /** Reads arrays whose lengths are checked against [maxBytes] (the file's size) before anything is allocated. */
    private class Reader(private val data: DataInputStream, private val maxBytes: Long) {
        private val chunk = ByteArray(CHUNK)

        private fun length(elementBytes: Int): Int {
            val length = data.readInt()
            if (length < 0 || length.toLong() * elementBytes > maxBytes) throw IOException("damaged index file")
            return length
        }

        fun bytes(): ByteArray = ByteArray(length(1)).also(data::readFully)

        fun ints(): IntArray {
            val values = IntArray(length(Int.SIZE_BYTES))
            var from = 0
            while (from < values.size) {
                val count = minOf(values.size - from, CHUNK / Int.SIZE_BYTES)
                data.readFully(chunk, 0, count * Int.SIZE_BYTES)
                ByteBuffer.wrap(chunk).asIntBuffer().get(values, from, count)
                from += count
            }
            return values
        }

        fun longs(): LongArray {
            val values = LongArray(length(Long.SIZE_BYTES))
            var from = 0
            while (from < values.size) {
                val count = minOf(values.size - from, CHUNK / Long.SIZE_BYTES)
                data.readFully(chunk, 0, count * Long.SIZE_BYTES)
                ByteBuffer.wrap(chunk).asLongBuffer().get(values, from, count)
                from += count
            }
            return values
        }
    }
}
