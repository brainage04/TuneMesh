package io.github.brainage04.tunemesh.library

/** Hashing, comparing and searching texts stored as UTF-8 bytes ([LibraryIndex]), without decoding them into strings. */
internal object Utf8 {
    private const val HASH_FACTOR = 31
    private const val BYTE = 0xFF
    private const val CONTINUATION = 0x3F
    private const val TWO_BYTES = 0xE0
    private const val THREE_BYTES = 0xF0
    private const val ONE_BYTE_END = 0x80
    private const val TWO_BYTE_BITS = 0x1F
    private const val THREE_BYTE_BITS = 0x0F
    private const val FOUR_BYTE_BITS = 0x07
    private const val BITS = 6
    private const val SUPPLEMENTARY = 0x10000
    private const val HIGH_SURROGATE = 0xD800
    private const val LOW_SURROGATE = 0xDC00
    private const val SURROGATE_BITS = 10
    private const val SURROGATE_MASK = 0x3FF

    /**
     * The running hash of `bytes[from, to)` after [seed]; [spread] it before use. Hashing two ranges in turn hashes their concatenation.
     */
    fun hashOf(bytes: ByteArray, from: Int, to: Int, seed: Int = 1): Int {
        var hash = seed
        for (i in from until to) hash = HASH_FACTOR * hash + bytes[i]
        return hash
    }

    fun spread(hash: Int): Int = hash xor (hash ushr Short.SIZE_BITS)

    fun hash(bytes: ByteArray, from: Int, to: Int): Int = spread(hashOf(bytes, from, to))

    /** Whether `a[aFrom, aTo)` and `b[bFrom, bTo)` hold the same bytes. */
    @Suppress("LongParameterList") // two byte ranges
    fun equal(a: ByteArray, aFrom: Int, aTo: Int, b: ByteArray, bFrom: Int, bTo: Int): Boolean {
        var same = aTo - aFrom == bTo - bFrom
        var i = 0
        while (same && i < aTo - aFrom) {
            same = a[aFrom + i] == b[bFrom + i]
            i++
        }
        return same
    }

    /** Whether `text[from, to)` contains [word]. On valid UTF-8 this is [String.contains] of the decoded texts. */
    fun contains(text: ByteArray, from: Int, to: Int, word: ByteArray): Boolean {
        val last = to - word.size
        var start = from
        while (start <= last) {
            var i = 0
            while (i < word.size && text[start + i] == word[i]) i++
            if (i == word.size) return true
            start++
        }
        return false
    }

    /** Reads the UTF-16 units of valid UTF-8 (as [String] would hold them) one at a time. */
    class Cursor(private val text: ByteArray) {
        private var position = 0
        private var end = 0
        private var low = 0

        fun reset(from: Int, to: Int) {
            position = from
            end = to
            low = 0
        }

        /** The next UTF-16 unit, or -1 at the end. */
        fun next(): Int =
            when {
                low != 0 -> low.also { low = 0 }
                position >= end -> -1
                else -> nextCodePoint()
            }

        /** Decodes the code point at [position]; a supplementary one returns its high surrogate and keeps the low one for later. */
        private fun nextCodePoint(): Int {
            val first = text[position].toInt() and BYTE
            fun continuation(offset: Int) = text[position + offset].toInt() and CONTINUATION
            val codePoint =
                when {
                    first < ONE_BYTE_END -> first.also { position += 1 }
                    first < TWO_BYTES -> ((first and TWO_BYTE_BITS) shl BITS or continuation(1)).also { position += 2 }
                    first < THREE_BYTES ->
                        ((first and THREE_BYTE_BITS) shl (2 * BITS) or (continuation(1) shl BITS) or continuation(2)).also { position += 3 }
                    else ->
                        ((first and FOUR_BYTE_BITS) shl
                                (3 * BITS) or
                                (continuation(1) shl (2 * BITS)) or
                                (continuation(2) shl BITS) or
                                continuation(3))
                            .also { position += 4 }
                }
            if (codePoint >= SUPPLEMENTARY) low = LOW_SURROGATE + ((codePoint - SUPPLEMENTARY) and SURROGATE_MASK)
            return if (codePoint < SUPPLEMENTARY) codePoint else HIGH_SURROGATE + ((codePoint - SUPPLEMENTARY) shr SURROGATE_BITS)
        }
    }

    /** [String.CASE_INSENSITIVE_ORDER] of texts stored in [text], without decoding them. Not thread-safe. */
    class CaseInsensitive(text: ByteArray) {
        private val a = Cursor(text)
        private val b = Cursor(text)

        fun compare(aFrom: Int, aTo: Int, bFrom: Int, bTo: Int): Int {
            a.reset(aFrom, aTo)
            b.reset(bFrom, bTo)
            while (true) {
                val first = a.next()
                val second = b.next()
                if (first < 0 || second < 0) return first.compareTo(second)
                if (first != second) {
                    val upperFirst = Character.toUpperCase(first.toChar())
                    val upperSecond = Character.toUpperCase(second.toChar())
                    if (upperFirst != upperSecond) {
                        val order = Character.toLowerCase(upperFirst) - Character.toLowerCase(upperSecond)
                        if (order != 0) return order
                    }
                }
            }
        }
    }
}

/** A stable sort of int arrays by a comparator of their values, without boxing them. */
internal object IntSort {
    fun interface Order {
        fun compare(a: Int, b: Int): Int
    }

    fun sort(values: IntArray, order: Order) {
        if (values.size < 2) return
        mergeSort(values, values.copyOf(), 0, values.size, order)
    }

    /** Sorts `values[from, to)`, using [scratch] (a copy of the same range) as the other buffer. */
    private fun mergeSort(values: IntArray, scratch: IntArray, from: Int, to: Int, order: Order) {
        if (to - from < 2) return
        val middle = (from + to) ushr 1
        mergeSort(scratch, values, from, middle, order)
        mergeSort(scratch, values, middle, to, order)
        var left = from
        var right = middle
        for (i in from until to) {
            values[i] =
                if (right >= to || (left < middle && order.compare(scratch[left], scratch[right]) <= 0)) scratch[left++]
                else scratch[right++]
        }
    }
}
