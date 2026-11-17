package app.amepricetrack.work

import android.content.Context
import android.os.SystemClock
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import app.amepricetrack.App
import app.amepricetrack.data.ApiException
import app.amepricetrack.data.CollectEntry
import app.amepricetrack.data.Session
import app.amepricetrack.data.dollars
import app.amepricetrack.ui.attempt
import app.amepricetrack.ui.problem
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import java.net.URI
import java.util.concurrent.TimeUnit

/**
 * Reading product pages on the phone and sending them to the server.
 *
 * The server reads prices itself, and that is where this work belongs. The
 * phone is for what the server cannot do: a store that refuses the server,
 * or a store the person signed in at on this phone only. Everything here is
 * kept as light as it can be, because it runs on a battery.
 */
object PhoneReport {
    /** The server accepts pages up to 12 MB; leave room for the rest of the request. */
    private const val MAX_PAGE_CHARS = 10_000_000

    /** Pages read in one automatic run at most. */
    const val MAX_AUTOMATIC = 40

    /** What happened with one page, in words for the person. */
    data class Outcome(val text: String, val ok: Boolean)

    /** Why a store's products are, or are not, part of automatic reports. */
    enum class Part {
        /** The server cannot read this store, so the phone does. */
        ServerCannot,

        /** The person opened this store in the app, so the phone may see their own prices. */
        OpenedHere,

        /** The server reads this store fine and the phone has nothing to add. */
        NotNeeded,

        /** The person turned this store off. */
        TurnedOff,
    }

    /** "www.walmart.com/ip/1" -> "walmart.com" */
    fun hostOf(url: String): String = try {
        URI(url).host.orEmpty().removePrefix("www.")
    } catch (e: Exception) {
        ""
    }

    /** The person's choices that decide what an automatic run reads. */
    data class Plan(
        /** Stores opened in the in-app browser. */
        val visitedStores: Set<String>,
        /** Stores the person turned off. */
        val skippedStores: Set<String>,
        /** When this phone last reported each product, in seconds. */
        val reportedAt: Map<Long, Long>,
        /** A product read more recently than this many seconds ago is left alone. */
        val minGapSeconds: Long,
    )

    /** How one store's products are treated by automatic reports. */
    fun partOf(entries: List<CollectEntry>, plan: Plan): Part {
        val host = entries.firstOrNull()?.let { hostOf(it.url) }.orEmpty()
        return when {
            host in plan.skippedStores -> Part.TurnedOff
            entries.any { it.needed } -> Part.ServerCannot
            host in plan.visitedStores -> Part.OpenedHere
            else -> Part.NotNeeded
        }
    }

    /**
     * The pages an automatic run reads: only the ones worth the battery.
     * The ones the server cannot read come first, then the ones at stores
     * the person opened in the app. Anything read recently is left for next
     * time, and the longest-waiting come first so a long list takes turns.
     */
    fun automatic(entries: List<CollectEntry>, plan: Plan, nowSeconds: Long): List<CollectEntry> {
        fun fresh(entry: CollectEntry): Boolean {
            val mine = plan.reportedAt[entry.productId] ?: 0L
            // A needed product that someone has read lately (the server managed after all) can wait too.
            val anyone = if (entry.needed) entry.lastSuccessAt ?: 0L else 0L
            return nowSeconds - maxOf(mine, anyone) < plan.minGapSeconds
        }
        val wanted = entries
            .filter { hostOf(it.url) !in plan.skippedStores }
            .filter { it.needed || hostOf(it.url) in plan.visitedStores }
            .filterNot(::fresh)
        return wanted.sortedWith(compareBy({ !it.needed }, { plan.reportedAt[it.productId] ?: 0L })).take(MAX_AUTOMATIC)
    }

    /** Loads one product page and sends it to the server. */
    suspend fun reportOne(app: App, reader: PageReader, entry: CollectEntry): Outcome {
        val html = withContext(Dispatchers.Main) { reader.read(entry.url) }
        if (html.length < 500) return Outcome("The page did not load.", ok = false)
        if (html.length > MAX_PAGE_CHARS) return Outcome("The page is too large to send.", ok = false)
        // A store opened in the app before may be showing this person's own prices.
        val signedIn = hostOf(entry.url) in app.session.visitedStores
        val sent = app.attempt { app.api.ingest(entry.url, html, signedIn) }
        return sent.fold(
            { result ->
                app.session.reportedAt = app.session.reportedAt + (entry.productId to System.currentTimeMillis() / 1000)
                val price = result.priceCents?.let(::dollars) ?: "no price"
                Outcome(if (result.recordedAs == "member") "$price, kept as your own price" else price, ok = true)
            },
            { Outcome(sent.problem(), ok = false) },
        )
    }
}

/**
 * Reports prices by itself, when the person turned that on: the products
 * the server cannot read, and the ones at stores they opened in the app.
 */
class ReportWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {

    override suspend fun doWork(): Result {
        val app = applicationContext as App
        if (!app.session.state.value.signedIn || app.session.autoReport == "off") return Result.success()
        try {
            run(app)
        } catch (e: ApiException) {
            // The server is out of reach (away from home) or this phone was signed out.
            // Trying again and again would only use battery; the next turn is soon enough.
        }
        return Result.success()
    }

    companion object {
        private const val WORK_NAME = "phone_report"

        /** Android stops background work after ten minutes; stop reading before that. */
        private const val BUDGET_MS = 8 * 60 * 1000L

        /** After this many pages in a row fail at a store, the rest of that store waits for next time. */
        private const val GIVE_UP_AFTER = 2

        /** Hours between runs for a setting, or null when it is off. */
        fun hoursFor(mode: String): Long? = when (mode) {
            "twice" -> 12L
            "daily" -> 24L
            else -> null
        }

        /** Reads the pages due and remembers what happened. Returns the summary. */
        suspend fun run(app: App): String {
            val session = app.session
            val plan = PhoneReport.Plan(
                visitedStores = session.visitedStores,
                skippedStores = session.reportSkippedStores,
                reportedAt = session.reportedAt,
                // Half the time between runs: a run that was delayed does not make the next one pointless.
                minGapSeconds = (hoursFor(session.autoReport) ?: 12L) * 3600 / 2,
            )
            // One small request decides whether there is anything to do at all.
            val entries = PhoneReport.automatic(app.api.collect(), plan, System.currentTimeMillis() / 1000)
            var done = 0
            var failed = 0
            if (entries.isNotEmpty()) {
                // The browser is only started when there is a page to read.
                val reader = withContext(Dispatchers.Main) { PageReader.create(app).also { it.lean = true } }
                try {
                    val deadline = SystemClock.elapsedRealtime() + BUDGET_MS
                    val failures = mutableMapOf<String, Int>()
                    for (entry in entries) {
                        if (SystemClock.elapsedRealtime() > deadline) break
                        val store = PhoneReport.hostOf(entry.url)
                        // A store that keeps refusing is not asked again this time.
                        if ((failures[store] ?: 0) >= GIVE_UP_AFTER) continue
                        if (PhoneReport.reportOne(app, reader, entry).ok) {
                            done++
                            failures[store] = 0
                        } else {
                            failed++
                            failures[store] = (failures[store] ?: 0) + 1
                        }
                    }
                } finally {
                    withContext(Dispatchers.Main + NonCancellable) { reader.close() }
                }
            }
            val summary = summarise(done, failed, entries.size)
            session.lastReportAt = System.currentTimeMillis()
            session.lastReportSummary = summary
            return summary
        }

        /** "Reported 5 prices. 1 could not be read." */
        fun summarise(done: Int, failed: Int, due: Int): String {
            if (due == 0) return "Nothing needed reporting."
            val reported = if (done == 1) "Reported 1 price." else "Reported $done prices."
            val problems = when (failed) {
                0 -> ""
                1 -> " 1 could not be read."
                else -> " $failed could not be read."
            }
            val left = due - done - failed
            val later = if (left > 0) " $left are left for next time." else ""
            return reported + problems + later
        }

        /** Applies the person's settings: how often, and under which conditions. */
        fun schedule(context: Context, session: Session) {
            val work = WorkManager.getInstance(context)
            val hours = hoursFor(session.autoReport)
            if (hours == null) {
                work.cancelUniqueWork(WORK_NAME)
                return
            }
            val constraints = Constraints.Builder()
                .setRequiredNetworkType(if (session.reportOnWifiOnly) NetworkType.UNMETERED else NetworkType.CONNECTED)
                .setRequiresBatteryNotLow(true)
                .setRequiresCharging(session.reportWhileCharging)
                .build()
            val request = PeriodicWorkRequestBuilder<ReportWorker>(hours, TimeUnit.HOURS).setConstraints(constraints).build()
            work.enqueueUniquePeriodicWork(WORK_NAME, ExistingPeriodicWorkPolicy.UPDATE, request)
        }

        fun cancel(context: Context) {
            WorkManager.getInstance(context).cancelUniqueWork(WORK_NAME)
        }
    }
}
