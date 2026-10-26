package io.github.brainage04.tunemesh.library

import java.lang.reflect.Modifier
import java.util.IdentityHashMap

/**
 * What an object graph keeps on the heap, estimated the same way on every JVM: 16 bytes per object header, 8 per reference or field,
 * strings at 2 bytes per character (as ART stores non-ASCII text), JDK collections by their size. Shared strings and objects count once.
 * Deterministic, so memory budgets can be asserted exactly; a real heap holds about as much or less (compact ASCII strings).
 */
object RetainedSize {
    fun of(vararg roots: Any?): Long {
        val seen = IdentityHashMap<Any, Unit>()
        val pending = ArrayDeque<Any>()
        roots.filterNotNull().forEach(pending::addLast)
        var total = 0L
        while (pending.isNotEmpty()) {
            val value = pending.removeLast()
            if (seen.put(value, Unit) != null) continue
            total += shallow(value) { it?.let(pending::addLast) }
        }
        return total
    }

    private const val HEADER = 16L
    private const val REFERENCE = 8L

    private fun shallow(value: Any, refer: (Any?) -> Unit): Long =
        when {
            // This app's classes by their fields, also those that are lists (PlaylistEntries).
            value.javaClass.name.startsWith("io.github.brainage04.") -> fields(value, refer)
            else -> library(value, refer)
        }

    private fun library(value: Any, refer: (Any?) -> Unit): Long =
        when (value) {
            is String -> HEADER + 8 + HEADER + 2L * value.length
            is Array<*> -> {
                value.forEach(refer)
                HEADER + REFERENCE * value.size
            }
            is Number,
            is Boolean,
            is Char,
            is Enum<*>,
            is Class<*> -> HEADER + 8
            is Map<*, *> -> {
                value.forEach { (k, v) ->
                    refer(k)
                    refer(v)
                }
                48 + 40L * value.size
            }
            // JDK and Kotlin collections: their backing arrays are not reflected into.
            is Collection<*> -> {
                value.forEach(refer)
                40 + REFERENCE * value.size
            }
            else -> primitiveArray(value) ?: fields(value, refer)
        }

    private fun primitiveArray(value: Any): Long? =
        when (value) {
            is ByteArray -> HEADER + value.size
            is BooleanArray -> HEADER + value.size
            is CharArray -> HEADER + 2L * value.size
            is ShortArray -> HEADER + 2L * value.size
            is IntArray -> HEADER + 4L * value.size
            is FloatArray -> HEADER + 4L * value.size
            is LongArray -> HEADER + 8L * value.size
            is DoubleArray -> HEADER + 8L * value.size
            else -> null
        }

    private fun fields(value: Any, refer: (Any?) -> Unit): Long {
        var size = HEADER
        var type: Class<*>? = value.javaClass
        while (type != null && type != Any::class.java) {
            check(!type.name.startsWith("java.")) { "no estimate for ${type.name}" }
            for (field in type.declaredFields) {
                if (Modifier.isStatic(field.modifiers)) continue
                size += REFERENCE
                if (!field.type.isPrimitive) {
                    field.isAccessible = true
                    refer(field.get(value))
                }
            }
            type = type.superclass
        }
        return size
    }
}
