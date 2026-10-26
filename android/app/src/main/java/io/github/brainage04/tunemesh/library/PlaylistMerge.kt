package io.github.brainage04.tunemesh.library

/**
 * The three-way merge of a playlist and one of its Syncthing conflict copies (contract: "Playlist conflict copies", steps 1-3), on entries
 * as [PlaylistStore] reads them. tunemesh-core's `playlist::merge`; both run `docs/test-vectors/playlist-merge/`.
 */
object PlaylistMerge {
    /** A Syncthing conflict copy of a playlist: `<Name>.sync-conflict-<YYYYMMDD>-<HHMMSS>-<ID7>.m3u8`. */
    private val conflictName = Regex("""(.+)\.sync-conflict-[0-9]{8}-[0-9]{6}-[A-Z2-7]{7}\.m3u8""")

    /** The playlist name a conflict copy's file name belongs to, or null when [fileName] is not exactly such a copy. */
    fun conflictOf(fileName: String): String? = conflictName.matchEntire(fileName)?.groupValues?.get(1)

    /**
     * Merges [copy] into [winner] against [base] (null: no base kept, so removals made elsewhere can come back). Every entry of the result
     * is one of [winner]'s or [copy]'s, with that file's `#EXTINF`.
     */
    fun merge(base: PlaylistEntries?, winner: PlaylistEntries, copy: PlaylistEntries): PlaylistEntries {
        val w = counts(winner)
        val c = counts(copy)
        val b = base?.let(::counts)
        // 1. How many entries each key keeps (see [wanted]); without a base, every count there counts as 0.
        val wanted = HashMap<String, Int>(w.size + c.size)
        for (key in w.keys + c.keys) wanted[key] = wanted(w[key] ?: 0, c[key] ?: 0, b?.get(key) ?: 0)
        // 2. The side whose order the result starts from.
        val fromCopy = base != null && b != null && reordered(copy, c, base, b) && !reordered(winner, w, base, b)
        return Result(if (fromCopy) copy else winner, wanted).fill(if (fromCopy) winner else copy)
    }

    /** The merge result: the entries kept of [start] (step 2), then those [fill] adds from the other side (step 3). */
    private class Result(private val start: PlaylistEntries, private val wanted: Map<String, Int>) {
        private val have = HashMap<String, Int>(wanted.size)
        /** Positions in [kept] by key, in order, for matching. */
        private val keptByKey = HashMap<String, MutableList<Int>>()
        /** Indices into [start]. */
        private val kept = ArrayList<Int>()

        init {
            for (i in start.indices) {
                val key = start.key(i)
                if (take(key)) {
                    keptByKey.getOrPut(key) { ArrayList(1) } += kept.size
                    kept += i
                }
            }
        }

        /** Counts one more entry with [key] when the result is still short of it. */
        private fun take(key: String): Boolean {
            val count = have[key] ?: 0
            if (count >= (wanted[key] ?: 0)) return false
            have[key] = count + 1
            return true
        }

        /** Step 3: keys still short get [other]'s entries, after the anchor (the last matched or inserted entry) or at the top. */
        fun fill(other: PlaylistEntries): PlaylistEntries {
            val top = ArrayList<Int>()
            val after = arrayOfNulls<ArrayList<Int>>(kept.size)
            var anchor = -1 // a position in kept, or -1 for the top
            val seen = HashMap<String, Int>()
            for (i in other.indices) {
                val key = other.key(i)
                val j = seen[key] ?: 0
                seen[key] = j + 1
                val match = keptByKey[key]?.getOrNull(j)
                if (match != null) {
                    anchor = match
                } else if (take(key)) {
                    if (anchor < 0) top += i else (after[anchor] ?: ArrayList<Int>().also { after[anchor] = it }) += i
                }
            }
            val result = start.builder(kept.size + top.size + after.sumOf { it?.size ?: 0 })
            for (i in top) result.add(other, i)
            for ((position, index) in kept.withIndex()) {
                result.add(start, index)
                after[position]?.forEach { result.add(other, it) }
            }
            return result.build()
        }
    }

    /** Step 1: entries with one key in the result, from its counts in the winner, the copy and the base. */
    internal fun wanted(w: Int, c: Int, b: Int): Int =
        when {
            w >= b && c >= b -> maxOf(w, c)
            w <= b && c <= b -> minOf(w, c)
            else -> maxOf(0, w + c - b)
        }

    private fun counts(entries: PlaylistEntries): HashMap<String, Int> {
        val counts = HashMap<String, Int>()
        for (i in entries.indices) counts.merge(entries.key(i), 1, Int::plus)
        return counts
    }

    /** Whether [side] changed the order: its keys whose count equals the base's, in order, differ from the base's same keys in order. */
    private fun reordered(
        side: PlaylistEntries,
        sideCounts: Map<String, Int>,
        base: PlaylistEntries,
        baseCounts: Map<String, Int>,
    ): Boolean {
        fun same(key: String) = sideCounts[key] == baseCounts[key]
        var i = 0
        var j = 0
        while (true) {
            while (i < side.size && !same(side.key(i))) i++
            while (j < base.size && !same(base.key(j))) j++
            if (i == side.size || j == base.size) return i != side.size || j != base.size
            if (side.key(i) != base.key(j)) return true
            i++
            j++
        }
    }
}
