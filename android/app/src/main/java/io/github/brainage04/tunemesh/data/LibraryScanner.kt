package io.github.brainage04.tunemesh.data

import android.media.MediaMetadataRetriever
import io.github.brainage04.tunemesh.library.AtomicFiles
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.Limits
import io.github.brainage04.tunemesh.library.RawTags
import io.github.brainage04.tunemesh.library.Track
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.io.File
import java.io.IOException
import java.nio.file.FileVisitResult
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.SimpleFileVisitor
import java.nio.file.attribute.BasicFileAttributes

@Serializable
private data class CachedTrack(val size: Long, val modifiedMs: Long, val tags: RawTags)

@Serializable
private data class TagCacheFile(val root: String, val entries: Map<String, CachedTrack>)

class ScanResult(val tracks: List<Track>, val directories: List<File>)

/**
 * Walks the library and reads tags for new or changed files only. Tags are cached in app storage keyed by
 * path, size and modification time.
 */
class LibraryScanner(private val cacheFile: File) {
    private val json = Json { ignoreUnknownKeys = true }
    private var cacheRoot: String? = null
    private var cache: MutableMap<String, CachedTrack> = HashMap()

    @Synchronized
    fun cachedTracks(root: File): List<Track> {
        load(root)
        // Filtered like a scan, so a cache from an older version with laxer rules shows nothing a scan would drop.
        return cache.filterKeys(LibraryFormat::isAudioPath)
            .map { (path, entry) -> Track.fromTags(path, entry.tags, entry.size, entry.modifiedMs) }
    }

    @Synchronized
    fun scan(root: File): ScanResult {
        load(root)
        val rootPath = root.toPath()
        val found = HashMap<String, Pair<Long, Long>>()
        val directories = mutableListOf<File>()
        if (root.isDirectory) {
            // Entries seen so far in each open directory, innermost last (Limits.MAX_FOLDER_ENTRIES).
            val seen = ArrayDeque<IntArray>()

            /** Counts one entry of the current directory; false once the rest of it must be skipped. */
            fun count(): Boolean {
                val counter = seen.lastOrNull() ?: return true
                return ++counter[0] <= Limits.MAX_FOLDER_ENTRIES
            }

            Files.walkFileTree(rootPath, object : SimpleFileVisitor<Path>() {
                override fun preVisitDirectory(dir: Path, attrs: BasicFileAttributes): FileVisitResult {
                    if (!count()) return FileVisitResult.SKIP_SIBLINGS
                    val relativePath = rootPath.relativize(dir)
                    val relative = relativePath.toString()
                    if (relative.isNotEmpty() && LibraryFormat.isExcludedDirectory(relative)) return FileVisitResult.SKIP_SUBTREE
                    if (relative.isNotEmpty() && relativePath.nameCount > Limits.MAX_DEPTH) return FileVisitResult.SKIP_SUBTREE
                    directories += dir.toFile()
                    seen.addLast(IntArray(1))
                    return FileVisitResult.CONTINUE
                }

                override fun postVisitDirectory(dir: Path, exc: IOException?): FileVisitResult {
                    seen.removeLastOrNull()
                    return FileVisitResult.CONTINUE
                }

                override fun visitFile(file: Path, attrs: BasicFileAttributes): FileVisitResult {
                    if (!count()) return FileVisitResult.SKIP_SIBLINGS
                    if (attrs.isRegularFile) {
                        val relative = rootPath.relativize(file).toString()
                        if (LibraryFormat.isAudioPath(relative)) {
                            found[relative] = attrs.size() to attrs.lastModifiedTime().toMillis()
                            if (found.size >= Limits.MAX_AUDIO_FILES) return FileVisitResult.TERMINATE
                        }
                    }
                    return FileVisitResult.CONTINUE
                }

                override fun visitFileFailed(file: Path, exc: IOException): FileVisitResult =
                    if (count()) FileVisitResult.CONTINUE else FileVisitResult.SKIP_SIBLINGS
            })
        }

        var changed = cache.keys.retainAll(found.keys)
        val tracks = ArrayList<Track>(found.size)
        for ((path, stat) in found) {
            val (size, modified) = stat
            val cached = cache[path]
            val entry = if (cached != null && cached.size == size && cached.modifiedMs == modified) {
                cached
            } else {
                changed = true
                CachedTrack(size, modified, readTags(File(root, path))).also { cache[path] = it }
            }
            tracks += Track.fromTags(path, entry.tags, size, modified)
        }
        if (changed) save()
        return ScanResult(tracks, directories)
    }

    private fun load(root: File) {
        val rootPath = root.absolutePath
        if (cacheRoot == rootPath) return
        cacheRoot = rootPath
        cache = try {
            val file = json.decodeFromString<TagCacheFile>(cacheFile.readText())
            if (file.root == rootPath) file.entries.toMutableMap() else HashMap()
        } catch (_: Exception) {
            HashMap()
        }
    }

    private fun save() {
        val root = cacheRoot ?: return
        try {
            AtomicFiles.write(cacheFile, json.encodeToString(TagCacheFile.serializer(), TagCacheFile(root, cache)))
        } catch (_: IOException) {
            // The cache is an optimisation; the next scan rebuilds it.
        }
    }

    companion object {
        fun readTags(file: File): RawTags {
            val retriever = MediaMetadataRetriever()
            return try {
                retriever.setDataSource(file.absolutePath)
                // A text item over Limits.MAX_TAG_ITEM_BYTES is not used; the others are cut to MAX_TAG_TEXT_BYTES.
                fun text(key: Int) = retriever.extractMetadata(key)
                    ?.takeUnless { it.length > Limits.MAX_TAG_ITEM_BYTES / 4 && Limits.utf8Length(it) > Limits.MAX_TAG_ITEM_BYTES }
                    ?.let { RawTags.boundedText(it) }?.takeIf { it.isNotEmpty() }
                RawTags(
                    title = text(MediaMetadataRetriever.METADATA_KEY_TITLE),
                    artist = text(MediaMetadataRetriever.METADATA_KEY_ARTIST),
                    album = text(MediaMetadataRetriever.METADATA_KEY_ALBUM),
                    albumArtist = text(MediaMetadataRetriever.METADATA_KEY_ALBUMARTIST),
                    trackNumber = text(MediaMetadataRetriever.METADATA_KEY_CD_TRACK_NUMBER)?.leadingInt(),
                    discNumber = text(MediaMetadataRetriever.METADATA_KEY_DISC_NUMBER)?.leadingInt(),
                    durationMs = text(MediaMetadataRetriever.METADATA_KEY_DURATION)?.toLongOrNull() ?: 0,
                )
            } catch (_: RuntimeException) {
                RawTags()
            } finally {
                retriever.release()
            }
        }

        private fun String.leadingInt(): Int? = substringBefore('/').trim().toIntOrNull()
    }
}
