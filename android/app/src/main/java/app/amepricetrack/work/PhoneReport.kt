package app.amepricetrack.work

import android.content.Context
import android.os.SystemClock
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import app.amepricetrack.App
import app.amepricetrack.data.ApiException
import app.amepricetrack.data.CollectEntry
import app.amepricetrack.data.dollars
import app.amepricetrack.ui.attempt
import app.amepricetrack.ui.problem
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import java.net.URI
import java.util.concurrent.TimeUnit

/** Reading product pages on the phone and sending them to the server. */
object PhoneReport {
    /** The server accepts pages up to 12 MB; leave room for the rest of the request. */
    private const val MAX_PAGE_CHARS = 10_000_000

    /** Pages read in one automatic run at most. */
    const val MAX_AUTOMATIC = 40

    /** What happened with one page, in words for the person. */
    data class Outcome(val text: String, val ok: Boolean)

    /** "www.walmart.com/ip/1" -> "walmart.com" */
    fun hostOf(url: String): String = try {
        URI(url).host.orEmpty().removePrefix("www.")
    } catch (e: Exception) {
        ""
    }

    /**
     * The pages an automatic run reads, most useful first: the ones the
     * server cannot read itself, then the ones at stores the person opened
     * in the app, where the phone may see their own price or their own store.
     */
    fun automatic(entries: List<CollectEntry>, visitedStores: Set<String>): List<CollectEntry> =
        (entries.filter { it.needed } + entries.filter { !it.needed && hostOf(it.url) in visitedStores }).take(MAX_AUTOMATIC)

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
                val price = result.priceCents?.let(::dollars) ?: "no price"
                Outcome(if (result.recordedAs == "member") "$price, kept as your own price" else price, ok = true)
            },
            { Outcome(sent.problem(), ok = false) },
        )
    }
}

/**
 * Reports prices by itself, once or twice a day: the products the server
 * cannot read, and the ones at stores the person signed in at in the app.
 */
class ReportWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {

    override suspend fun doWork(): Result {
        val app = applicationContext as App
        if (!app.session.state.value.signedIn || app.session.autoReport == "off") return Result.success()
        return try {
            run(app)
            Result.success()
        } catch (e: ApiException) {
            // Signed out elsewhere: nothing to retry until the person signs in again.
            if (e.signedOut) Result.success() else Result.retry()
        }
    }

    companion object {
        private const val WORK_NAME = "phone_report"
        private const val WORK_NAME_NOW = "phone_report_now"

        /** Android stops background work after ten minutes; stop reading before that. */
        private const val BUDGET_MS = 8 * 60 * 1000L

        /** Reads the pages due and remembers what happened. Returns the summary. */
        suspend fun run(app: App): String {
            val entries = PhoneReport.automatic(app.api.collect(), app.session.visitedStores)
            var done = 0
            var failed = 0
            if (entries.isNotEmpty()) {
                val reader = withContext(Dispatchers.Main) { PageReader.create(app) }
                try {
                    val deadline = SystemClock.elapsedRealtime() + BUDGET_MS
                    for (entry in entries) {
                        if (SystemClock.elapsedRealtime() > deadline) break
                        if (PhoneReport.reportOne(app, reader, entry).ok) done++ else failed++
                    }
                } finally {
                    withContext(Dispatchers.Main + NonCancellable) { reader.close() }
                }
            }
            val summary = summarise(done, failed, entries.size)
            app.session.lastReportAt = System.currentTimeMillis()
            app.session.lastReportSummary = summary
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

        /** Applies the chosen setting: reports twice a day, daily, or not at all. */
        fun schedule(context: Context, mode: String) {
            val work = WorkManager.getInstance(context)
            val hours = when (mode) {
                "twice" -> 12L
                "daily" -> 24L
                else -> {
                    work.cancelUniqueWork(WORK_NAME)
                    return
                }
            }
            val constraints =
                Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).setRequiresBatteryNotLow(true).build()
            val request = PeriodicWorkRequestBuilder<ReportWorker>(hours, TimeUnit.HOURS).setConstraints(constraints).build()
            work.enqueueUniquePeriodicWork(WORK_NAME, ExistingPeriodicWorkPolicy.UPDATE, request)
        }

        /** Reports once, soon: used when the app is opened after a long gap. */
        fun runSoon(context: Context) {
            val constraints = Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()
            val request = OneTimeWorkRequestBuilder<ReportWorker>().setConstraints(constraints).build()
            WorkManager.getInstance(context).enqueueUniqueWork(WORK_NAME_NOW, ExistingWorkPolicy.KEEP, request)
        }

        fun cancel(context: Context) {
            WorkManager.getInstance(context).cancelUniqueWork(WORK_NAME)
            WorkManager.getInstance(context).cancelUniqueWork(WORK_NAME_NOW)
        }
    }
}
