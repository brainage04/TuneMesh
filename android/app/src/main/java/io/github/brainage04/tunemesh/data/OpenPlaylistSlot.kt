package io.github.brainage04.tunemesh.data

/**
 * Which playlist screen is open, numbered: every open of another playlist and every close starts a new generation. A load takes a [Ticket]
 * before it reads the file and applies its result through [ifCurrent], which runs only while that generation is still current and excludes
 * [open] and [close] while it runs. So a load that started before the screen closed (or another one opened) can never put its entries back
 * into the state.
 */
class OpenPlaylistSlot {
    /** The open playlist ([name], null when none) as a load saw it. */
    data class Ticket(val name: String?, val generation: Long)

    private var name: String? = null
    private var generation = 0L

    val current: Ticket
        @Synchronized get() = Ticket(name, generation)

    /** Opens [name]; false when it was open already (its generation stays, so a load of it under way still applies). */
    @Synchronized
    fun open(name: String): Boolean {
        if (this.name == name) return false
        this.name = name
        generation++
        return true
    }

    /** Closes [name] when it is the open playlist and runs [onClosed], before any load of it can apply; false when it was not open. */
    @Synchronized
    fun close(name: String, onClosed: () -> Unit): Boolean {
        if (this.name != name) return false
        this.name = null
        generation++
        onClosed()
        return true
    }

    /** Runs [apply] when [ticket] is still the current generation; returns whether it ran. */
    @Synchronized
    fun ifCurrent(ticket: Ticket, apply: () -> Unit): Boolean {
        if (ticket.generation != generation) return false
        apply()
        return true
    }
}
