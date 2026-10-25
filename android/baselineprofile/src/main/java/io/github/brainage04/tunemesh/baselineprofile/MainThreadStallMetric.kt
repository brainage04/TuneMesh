package io.github.brainage04.tunemesh.baselineprofile

import androidx.benchmark.macro.ExperimentalMetricApi
import androidx.benchmark.macro.TraceMetric
import androidx.benchmark.traceprocessor.TraceProcessor

/**
 * The app's main-thread stalls in the trace of one iteration, as the hostile-library test's watchdog sees them: how long a message posted
 * to the main thread can wait. The trace has no slice per looper message, so a stall is a top-level main-thread slice (a frame, an activity
 * start, a service start; each runs inside one message), or untraced work: a stretch in which the main thread does not sleep (pauses under
 * [GAP_NS] included) and that overlaps no such slice. Reports
 * - `mainThreadStallMaxMs`: the longest stall,
 * - `mainThreadWorkMaxMs`: the longest stall less the time the main thread waits for its render thread: in `postAndWait`, or asleep while
 *   the render thread draws a frame (the first frames wait there without a `postAndWait` slice). On an emulator without a GPU the software
 *   renderer alone can take 250 ms for a frame; it is not the app's work,
 * - `mainThreadStallsOver50Ms`: how many stalls took more than 50 ms,
 * - `frameMaxMs`: the longest `Choreographer#doFrame`.
 *
 * With [afterFirstFrame], work that starts before the end of the process's first frame (process start, `bindApplication`, the first
 * activity start, the first frame) is left out: the startup metrics cover it. The first frame is the first one after the first activity
 * start (`clientTransactionExecuted`): a new process runs an empty `Choreographer#doFrame` before it.
 */
@OptIn(ExperimentalMetricApi::class)
class MainThreadStallMetric(private val afterFirstFrame: Boolean = false) : TraceMetric() {
    override fun getMeasurements(captureInfo: CaptureInfo, traceSession: TraceProcessor.Session): List<Measurement> {
        val slices = topLevelSlices(traceSession)
        val launchEnd = slices.filter { it.launch }.groupBy { it.utid }.mapValues { (_, launches) -> launches.minOf { it.end } }
        val firstFrameEnd =
            slices
                .filter { it.frame && it.start >= (launchEnd[it.utid] ?: Long.MIN_VALUE) }
                .groupBy { it.utid }
                .mapValues { (_, frames) -> frames.minBy { it.start }.end }
        fun counted(span: Span) = !afterFirstFrame || span.start >= (firstFrameEnd[span.utid] ?: Long.MAX_VALUE)
        val stalls = (slices + untraced(slices, busyStretches(traceSession))).filter(::counted)
        val waits = merge(renderWaits(traceSession), gap = 0).groupBy { it.utid }
        fun work(stall: Span) =
            stall.length - waits[stall.utid].orEmpty().sumOf { maxOf(0L, minOf(it.end, stall.end) - maxOf(it.start, stall.start)) }
        return listOf(
            Measurement("mainThreadStallMaxMs", (stalls.maxOfOrNull { it.length } ?: 0L).toDouble() / NS_PER_MS),
            Measurement("mainThreadWorkMaxMs", (stalls.maxOfOrNull(::work) ?: 0L).toDouble() / NS_PER_MS),
            Measurement("mainThreadStallsOver50Ms", stalls.count { it.length > STALL_REPORTED_NS }.toDouble()),
            Measurement("frameMaxMs", (slices.filter { it.frame && counted(it) }.maxOfOrNull { it.length } ?: 0L).toDouble() / NS_PER_MS),
        )
    }

    private class Span(val utid: Long, val start: Long, var end: Long, val frame: Boolean = false, val launch: Boolean = false) {
        val length: Long
            get() = end - start
    }

    private fun topLevelSlices(session: TraceProcessor.Session): List<Span> =
        session
            .query(
                """
                SELECT thread_track.utid AS utid, slice.ts AS ts, slice.dur AS dur, slice.name AS name FROM slice
                JOIN thread_track ON slice.track_id = thread_track.id
                WHERE thread_track.utid IN ($MAIN_THREADS) AND slice.depth = 0 AND slice.dur > 0
                """
                    .trimIndent()
            )
            .map {
                val name = it.string("name")
                Span(
                    it.long("utid"),
                    it.long("ts"),
                    it.long("ts") + it.long("dur"),
                    frame = name.startsWith(FRAME),
                    launch = name == LAUNCH,
                )
            }
            .toList()

    /**
     * Where the main thread waits for the render thread: `postAndWait` (inside a frame's draw), and any sleep while its process's render
     * thread draws a frame (`DrawFrames`). The two overlap; [merge] joins them.
     */
    private fun renderWaits(session: TraceProcessor.Session): List<Span> =
        session
            .query(
                """
                SELECT thread_track.utid AS utid, slice.ts AS start, slice.ts + slice.dur AS end FROM slice
                JOIN thread_track ON slice.track_id = thread_track.id
                WHERE thread_track.utid IN ($MAIN_THREADS) AND slice.name = 'postAndWait' AND slice.dur > 0
                UNION ALL
                SELECT state.utid AS utid, MAX(state.ts, draw.ts) AS start, MIN(state.ts + state.dur, draw.ts + draw.dur) AS end
                FROM thread_state state
                JOIN thread main ON main.utid = state.utid
                JOIN thread render ON render.upid = main.upid AND render.name = 'RenderThread'
                JOIN thread_track ON thread_track.utid = render.utid
                JOIN slice draw ON draw.track_id = thread_track.id AND draw.depth = 0 AND draw.name LIKE 'DrawFrames%'
                WHERE state.utid IN ($MAIN_THREADS) AND state.state = 'S' AND state.dur > 0
                  AND draw.ts < state.ts + state.dur AND draw.ts + draw.dur > state.ts
                """
                    .trimIndent()
            )
            .map { Span(it.long("utid"), it.long("start"), it.long("end")) }
            .toList()

    /** Stretches in which the main thread does not sleep. */
    private fun busyStretches(session: TraceProcessor.Session): List<Span> =
        merge(
            session
                .query("SELECT utid, ts, dur FROM thread_state WHERE utid IN ($MAIN_THREADS) AND dur > 0 AND state != 'S'")
                .map { Span(it.long("utid"), it.long("ts"), it.long("ts") + it.long("dur")) }
                .toList()
        )

    /** The [busy] stretches that overlap none of the [slices] (which do not overlap each other on one thread). */
    private fun untraced(slices: List<Span>, busy: List<Span>): List<Span> {
        val byThread = slices.groupBy { it.utid }.mapValues { (_, spans) -> spans.sortedBy { it.start } }
        return busy.filter { stretch ->
            val thread = byThread[stretch.utid].orEmpty()
            // The last slice that starts before the stretch ends; only it can overlap the stretch.
            var low = 0
            var high = thread.size
            while (low < high) {
                val middle = (low + high) / 2
                if (thread[middle].start < stretch.end) low = middle + 1 else high = middle
            }
            low == 0 || thread[low - 1].end <= stretch.start
        }
    }

    /** [spans] of each thread joined where they overlap or are less than [gap] apart. */
    private fun merge(spans: List<Span>, gap: Long = GAP_NS): List<Span> {
        val merged = ArrayList<Span>()
        for (span in spans.sortedWith(compareBy<Span>({ it.utid }, { it.start }))) {
            val last = merged.lastOrNull()
            if (last != null && last.utid == span.utid && span.start <= last.end + gap) {
                last.end = maxOf(last.end, span.end)
            } else {
                merged += Span(span.utid, span.start, span.end)
            }
        }
        return merged
    }

    private companion object {
        const val MAIN_THREADS =
            "SELECT utid FROM thread JOIN process USING (upid) WHERE process.name = '$PACKAGE' AND thread.tid = process.pid"
        const val FRAME = "Choreographer#doFrame"
        const val LAUNCH = "clientTransactionExecuted"
        const val NS_PER_MS = 1_000_000L
        const val STALL_REPORTED_NS = 50 * NS_PER_MS

        /** A pause shorter than this (a garbage collector handshake, a short lock wait) does not end untraced work. */
        const val GAP_NS = NS_PER_MS
    }
}
