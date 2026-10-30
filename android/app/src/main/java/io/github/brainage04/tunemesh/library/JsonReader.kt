package io.github.brainage04.tunemesh.library

import java.io.InputStream

/**
 * A strict streaming JSON reader (RFC 8259) for the request and status files: it keeps only what the caller asks for and checks the rest
 * without holding it. A string is decoded only up to the number of UTF-8 bytes asked for (cut as [Limits.cutUtf8] cuts the whole string); a
 * skipped value, and the part of a string past its limit, allocates nothing. So a status file of 250,000 track paths costs a pass over its
 * bytes, not 250,000 strings. Reads at most [maxBytes] from [input].
 *
 * Every syntax error, value of the wrong type and input past [maxBytes] throws [Malformed]; parsers turn it into "not a valid file".
 */
@Suppress("TooManyFunctions") // one small function per JSON token
internal class JsonReader(private val input: InputStream, private val maxBytes: Long = Long.MAX_VALUE) {
    class Malformed(message: String) : IllegalArgumentException(message)

    private val buffer = ByteArray(BUFFER_BYTES)
    private var position = 0
    private var limit = 0
    private var total = 0L

    /** Raw string bytes not decoded yet (see [string]); never splits a UTF-8 sequence. */
    private val run = ByteArray(RUN_BYTES)

    /** Whether a byte is left, reading more when the buffer is used up. */
    private fun fill(): Boolean {
        if (position >= limit) {
            val count = input.read(buffer)
            if (count <= 0) return false
            total += count
            if (total > maxBytes) throw Malformed("larger than $maxBytes bytes")
            position = 0
            limit = count
        }
        return true
    }

    /** The next byte (0..255) without consuming it, or -1 at the end. */
    private fun peek(): Int = if (fill()) buffer[position].toInt() and BYTE else -1

    private fun next(): Int {
        if (!fill()) throw Malformed("unexpected end")
        return buffer[position++].toInt() and BYTE
    }

    private fun skipWhiteSpace() {
        while (true) {
            when (peek()) {
                SPACE,
                TAB,
                LF,
                CR -> position++
                else -> return
            }
        }
    }

    private fun expect(char: Char) {
        skipWhiteSpace()
        val found = next()
        if (found != char.code) throw Malformed("expected '$char', found ${describe(found)}")
    }

    private fun describe(byte: Int) = if (byte in PRINTABLE) "'${byte.toChar()}'" else "byte $byte"

    /** Consumes [char] when it is the next non-blank byte. */
    private fun consume(char: Char): Boolean {
        skipWhiteSpace()
        if (peek() != char.code) return false
        position++
        return true
    }

    /** After a member or element: true on `,`, false on [close]. */
    private fun commaOr(close: Char): Boolean {
        skipWhiteSpace()
        return when (val found = next()) {
            ','.code -> true
            close.code -> false
            else -> throw Malformed("expected ',' or '$close', found ${describe(found)}")
        }
    }

    /** Calls [onField] with each member name of an object, the reader at its value, which [onField] must read or skip. */
    inline fun forEachField(onField: (String) -> Unit) {
        beginObject()
        if (endsObject()) return
        do {
            val name = name()
            onField(name)
        } while (nextField())
    }

    /** Calls [onElement] for each element of an array, the reader at the element, which [onElement] must read or skip. */
    inline fun forEachElement(onElement: () -> Unit) {
        beginArray()
        if (endsArray()) return
        do onElement() while (nextElement())
    }

    fun beginObject() = expect('{')

    fun endsObject(): Boolean = consume('}')

    fun nextField(): Boolean = commaOr('}')

    fun beginArray() = expect('[')

    fun endsArray(): Boolean = consume(']')

    fun nextElement(): Boolean = commaOr(']')

    /** A member name and its `:`. Names longer than [NAME_BYTES] come back cut, so they match no field a parser knows. */
    fun name(): String {
        val name = string(NAME_BYTES)
        expect(':')
        return name
    }

    /** Consumes `null` when it is the next value. */
    fun nextIsNull(): Boolean {
        skipWhiteSpace()
        if (peek() != 'n'.code) return false
        literal("null")
        return true
    }

    private fun literal(word: String) {
        for (char in word) if (next() != char.code) throw Malformed("invalid literal")
    }

    fun boolean(): Boolean {
        skipWhiteSpace()
        return when (peek()) {
            't'.code -> true.also { literal("true") }
            'f'.code -> false.also { literal("false") }
            else -> throw Malformed("expected a boolean")
        }
    }

    /** An integer (no fraction or exponent) that fits a Long. */
    fun long(): Long {
        val text = number()
        if (text.any { it == '.' || it == 'e' || it == 'E' }) throw Malformed("expected an integer")
        return text.toLongOrNull() ?: throw Malformed("integer out of range")
    }

    /** A finite number. */
    fun double(): Double = number().toDouble().takeIf { it.isFinite() } ?: throw Malformed("number out of range")

    /** The next number's text, checked against the JSON grammar. */
    private fun number(): String {
        skipWhiteSpace()
        val text = StringBuilder()
        if (peek() == '-'.code) text.append(next().toChar())
        when (peek()) {
            '0'.code -> text.append(next().toChar())
            in DIGITS -> digits(text)
            else -> throw Malformed("expected a number")
        }
        if (peek() == '.'.code) {
            text.append(next().toChar())
            requireDigits(text)
        }
        if (peek() == 'e'.code || peek() == 'E'.code) {
            text.append(next().toChar())
            if (peek() == '+'.code || peek() == '-'.code) text.append(next().toChar())
            requireDigits(text)
        }
        return text.toString()
    }

    /** Appends the digits that follow to [text]; returns how many. */
    private fun digits(text: StringBuilder): Int {
        var count = 0
        while (peek() in DIGITS) {
            text.append(next().toChar())
            count++
        }
        return count
    }

    private fun requireDigits(text: StringBuilder) {
        if (digits(text) == 0) throw Malformed("expected a digit")
    }

    /**
     * The next string, cut to at most [maxBytes] UTF-8 bytes exactly as [Limits.cutUtf8] cuts the decoded string (malformed UTF-8 decodes
     * as by `String(bytes, UTF_8)`). Nothing past the cut is kept; [wasCut] then tells whether anything was dropped.
     */
    fun string(maxBytes: Int): String = StringCollector(maxBytes).also { readString(it) }.result()

    /** Whether the last [string] call dropped part of its string. */
    var wasCut: Boolean = false
        private set

    /** Checks the next value is a string and skips it. */
    fun skipString() = readString(null)

    private inner class StringCollector(private val maxBytes: Int) {
        private val out = StringBuilder()
        private var bytes = 0
        private var full = false
        private var high: Char? = null

        /** Adds one decoded code unit; a high surrogate waits for its low one. */
        fun add(char: Char) {
            if (full) return
            val pending = high
            if (pending != null) {
                high = null
                if (Character.isLowSurrogate(char)) {
                    keep(UTF8_PAIR, pending, char)
                    return
                }
                keep(UTF8_LONE_SURROGATE, pending)
            }
            when {
                Character.isHighSurrogate(char) -> high = char
                Character.isLowSurrogate(char) -> keep(UTF8_LONE_SURROGATE, char)
                char.code < UTF8_ONE_BYTE_END -> keep(1, char)
                char.code < UTF8_TWO_BYTES_END -> keep(2, char)
                else -> keep(UTF8_BMP, char)
            }
        }

        private fun keep(length: Int, first: Char, second: Char? = null) {
            if (full) return
            if (bytes + length > maxBytes) {
                full = true
                return
            }
            bytes += length
            out.append(first)
            if (second != null) out.append(second)
        }

        val isFull: Boolean
            get() = full

        fun result(): String {
            high?.let {
                high = null
                keep(UTF8_LONE_SURROGATE, it)
            }
            wasCut = full
            return out.toString()
        }
    }

    /** Reads a string into [into] (null: only checks it), stopping decoding once [into] is full. */
    private fun readString(into: StringCollector?) {
        expect('"')
        val run = Utf8Run(into)
        while (true) {
            when (val byte = next()) {
                '"'.code -> {
                    run.flush()
                    return
                }
                '\\'.code -> {
                    run.flush()
                    val char = escape()
                    into?.add(char)
                }
                else -> run.add(byte)
            }
        }
    }

    /**
     * Raw UTF-8 bytes of a string between escapes, decoded in runs of up to [RUN_BYTES] as `String(bytes, UTF_8)` decodes them (a run never
     * ends inside a sequence). Bytes are not even kept once [into] is full, or when there is no [into].
     */
    private inner class Utf8Run(private val into: StringCollector?) {
        private var length = 0

        fun add(byte: Int) {
            if (into == null || into.isFull) return
            if (length == run.size) flush(sequenceStart())
            run[length++] = byte.toByte()
        }

        /** Where the last, possibly incomplete, multi-byte sequence starts; [length] when there is none. */
        private fun sequenceStart(): Int {
            var start = length - 1
            while (start > length - MAX_SEQUENCE && run[start].toInt() and CONTINUATION_MASK == CONTINUATION) start--
            return if (run[start].toInt() and LEAD_MASK == LEAD) start else length
        }

        /** Decodes the bytes before [upTo] into [into] and keeps the rest. */
        fun flush(upTo: Int = length) {
            if (upTo == 0) return
            if (into != null && !into.isFull) {
                for (char in String(run, 0, upTo, Charsets.UTF_8)) into.add(char)
            }
            System.arraycopy(run, upTo, run, 0, length - upTo)
            length -= upTo
        }
    }

    private fun escape(): Char =
        when (val byte = next()) {
            '"'.code -> '"'
            '\\'.code -> '\\'
            '/'.code -> '/'
            'b'.code -> '\b'
            'f'.code -> '\u000C'
            'n'.code -> '\n'
            'r'.code -> '\r'
            't'.code -> '\t'
            'u'.code -> {
                var code = 0
                repeat(HEX_DIGITS) {
                    val digit = Character.digit(next(), HEX)
                    if (digit < 0) throw Malformed("invalid \\u escape")
                    code = code * HEX + digit
                }
                code.toChar()
            }
            else -> throw Malformed("invalid escape ${describe(byte)}")
        }

    /** Checks the next value, whatever it is, and skips it; nesting is counted, not recursed into. */
    fun skipValue() {
        val open = Containers()
        do {
            // Into containers until at a whole value.
            var opened = enter(open)
            while (opened) opened = enter(open)
        } while (nextInside(open))
    }

    /**
     * At a value: skips a whole value (a scalar, an empty object or array) and returns false, or opens a non-empty container, moves to its
     * first value and returns true.
     */
    private fun enter(open: Containers): Boolean {
        skipWhiteSpace()
        val first = peek()
        if (first != '{'.code && first != '['.code) {
            skipScalar()
            return false
        }
        position++
        val isObject = first == '{'.code
        val opened = !consume(if (isObject) '}' else ']')
        if (opened) {
            open.push(isObject)
            if (isObject) {
                skipString()
                expect(':')
            }
        }
        return opened
    }

    private fun skipScalar() {
        when (peek()) {
            '"'.code -> skipString()
            't'.code -> literal("true")
            'f'.code -> literal("false")
            'n'.code -> literal("null")
            else -> number()
        }
    }

    /** After a value: closes containers until one goes on with another member or element (true, at its value), or none is open (false). */
    private fun nextInside(open: Containers): Boolean {
        while (open.depth > 0) {
            val isObject = open.isObject()
            if (commaOr(if (isObject) '}' else ']')) {
                if (isObject) {
                    skipString()
                    expect(':')
                }
                return true
            }
            open.pop()
        }
        return false
    }

    /** The containers [skipValue] is inside, innermost last: one bit each (set: an object), so deep nesting costs a bit per level. */
    private class Containers {
        private var objects = LongArray(1)
        var depth = 0
            private set

        fun push(isObject: Boolean) {
            if (depth == objects.size * Long.SIZE_BITS) objects = objects.copyOf(objects.size * 2)
            val word = depth / Long.SIZE_BITS
            val bit = 1L shl (depth % Long.SIZE_BITS)
            objects[word] = if (isObject) objects[word] or bit else objects[word] and bit.inv()
            depth++
        }

        fun pop() {
            depth--
        }

        /** Whether the innermost container is an object. */
        fun isObject(): Boolean = objects[(depth - 1) / Long.SIZE_BITS] and (1L shl ((depth - 1) % Long.SIZE_BITS)) != 0L
    }

    /** Checks that only white space follows the value read. */
    fun end() {
        skipWhiteSpace()
        if (peek() != -1) throw Malformed("content after the value")
    }

    companion object {
        private const val BUFFER_BYTES = 16 * 1024
        private const val RUN_BYTES = 4 * 1024
        private const val MAX_SEQUENCE = 4
        private const val CONTINUATION_MASK = 0xC0
        private const val CONTINUATION = 0x80
        private const val LEAD_MASK = 0xC0
        private const val LEAD = 0xC0
        private const val NAME_BYTES = 64
        private const val BYTE = 0xFF
        private const val HEX = 16
        private const val SPACE = 0x20
        private const val TAB = 0x09
        private const val LF = 0x0A
        private const val CR = 0x0D
        private const val UTF8_PAIR = 4
        private const val UTF8_BMP = 3
        private const val UTF8_ONE_BYTE_END = 0x80
        private const val UTF8_TWO_BYTES_END = 0x800
        private const val HEX_DIGITS = 4
        private const val UTF8_LONE_SURROGATE = 3
        private val DIGITS = '0'.code..'9'.code
        private val PRINTABLE = 0x21..0x7E
    }
}
