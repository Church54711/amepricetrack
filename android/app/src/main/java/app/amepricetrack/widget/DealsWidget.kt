package app.amepricetrack.widget

import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.net.Uri
import android.text.format.DateFormat
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.action.ActionParameters
import androidx.glance.action.clickable
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.action.ActionCallback
import androidx.glance.appwidget.action.actionRunCallback
import androidx.glance.appwidget.action.actionStartActivity
import androidx.glance.appwidget.cornerRadius
import androidx.glance.appwidget.lazy.LazyColumn
import androidx.glance.appwidget.lazy.items
import androidx.glance.appwidget.provideContent
import androidx.glance.appwidget.updateAll
import androidx.glance.background
import androidx.glance.layout.Alignment
import androidx.glance.layout.Column
import androidx.glance.layout.Row
import androidx.glance.layout.Spacer
import androidx.glance.layout.fillMaxSize
import androidx.glance.layout.fillMaxWidth
import androidx.glance.layout.height
import androidx.glance.layout.padding
import androidx.glance.layout.width
import androidx.glance.text.FontWeight
import androidx.glance.text.Text
import androidx.glance.text.TextStyle
import androidx.glance.unit.ColorProvider
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
import app.amepricetrack.MainActivity
import app.amepricetrack.data.ApiException
import app.amepricetrack.data.DealRow
import app.amepricetrack.data.WatchItem
import app.amepricetrack.data.dealRows
import app.amepricetrack.data.dealRowsFromJson
import app.amepricetrack.data.dealRowsToJson
import app.amepricetrack.ui.Palette
import app.amepricetrack.ui.paletteFor
import java.util.Date
import java.util.concurrent.TimeUnit

/** What the widget last showed, kept so it can draw without the network. */
object WidgetStore {
    private fun prefs(context: Context) = context.getSharedPreferences("widget", Context.MODE_PRIVATE)

    fun save(context: Context, rows: List<DealRow>) {
        prefs(context).edit().putString("rows", dealRowsToJson(rows)).putLong("saved_at", System.currentTimeMillis()).apply()
    }

    fun rows(context: Context): List<DealRow> = dealRowsFromJson(prefs(context).getString("rows", null))

    /** When the rows were fetched, in milliseconds; 0 for never. */
    fun savedAt(context: Context): Long = prefs(context).getLong("saved_at", 0)

    fun clear(context: Context) {
        prefs(context).edit().clear().apply()
    }
}

/** The home-screen widget: the best prices on the watchlist right now. */
class DealsWidget : GlanceAppWidget() {
    override suspend fun provideGlance(context: Context, id: GlanceId) {
        val app = context.applicationContext as App
        val session = app.session.state.value
        val night = context.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES
        val palette = paletteFor(session.shownTheme, session.shownAccent, night)
        val rows = if (session.signedIn) WidgetStore.rows(context) else emptyList()
        val savedAt = WidgetStore.savedAt(context)
        val updated = if (savedAt > 0) DateFormat.getTimeFormat(context).format(Date(savedAt)) else null
        provideContent { Content(context, palette, session.signedIn, rows, updated) }
    }

    companion object {
        /** Shows a watchlist the app has just loaded anyway. */
        suspend fun show(context: Context, items: List<WatchItem>) {
            WidgetStore.save(context, dealRows(items))
            DealsWidget().updateAll(context)
        }

        /** Fetches the watchlist and redraws. */
        suspend fun refresh(app: App) {
            show(app, app.api.watches())
        }

        /** Redraws from what is stored, after signing in or out or a change of theme. */
        suspend fun redraw(context: Context) {
            DealsWidget().updateAll(context)
        }
    }
}

private fun openApp(context: Context, productId: Long? = null): Intent {
    val intent = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
    if (productId != null) {
        intent.action = Intent.ACTION_VIEW
        intent.data = Uri.parse("amepricetrack://products/$productId")
    }
    return intent
}

@Composable
private fun Content(context: Context, p: Palette, signedIn: Boolean, rows: List<DealRow>, updated: String?) {
    val muted = TextStyle(color = ColorProvider(p.muted), fontSize = 12.sp)
    Column(GlanceModifier.fillMaxSize().background(p.bg).cornerRadius(18.dp).padding(12.dp)) {
        Row(GlanceModifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(
                "Best prices",
                style = TextStyle(color = ColorProvider(p.accent), fontSize = 15.sp, fontWeight = FontWeight.Bold),
                modifier = GlanceModifier.defaultWeight().clickable(actionStartActivity(openApp(context))),
            )
            if (signedIn) {
                Text(
                    if (updated == null) "Refresh" else "$updated ↻",
                    style = muted,
                    modifier = GlanceModifier.clickable(actionRunCallback<RefreshWidget>()).padding(start = 8.dp, top = 2.dp, bottom = 2.dp),
                )
            }
        }
        Spacer(GlanceModifier.height(6.dp))
        when {
            !signedIn -> Note("Open AmePriceTrack and sign in to your server.", p, context)
            rows.isEmpty() && updated == null -> Note("Open AmePriceTrack once to fill this in.", p, context)
            rows.isEmpty() -> Note("Nothing on your watchlist has a price yet.", p, context)
            else -> LazyColumn(GlanceModifier.fillMaxSize()) {
                items(rows, itemId = { it.productId }) { row ->
                    Row(
                        GlanceModifier.fillMaxWidth().padding(vertical = 5.dp).clickable(actionStartActivity(openApp(context, row.productId))),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Column(GlanceModifier.defaultWeight()) {
                            Text(row.title, maxLines = 1, style = TextStyle(color = ColorProvider(p.text), fontSize = 13.sp))
                            Text(
                                if (row.note == null) row.store else "${row.store} · ${row.note}",
                                maxLines = 1,
                                style = TextStyle(color = ColorProvider(if (row.good) p.good else p.muted), fontSize = 12.sp),
                            )
                        }
                        Spacer(GlanceModifier.width(8.dp))
                        Text(
                            row.price,
                            style = TextStyle(color = ColorProvider(if (row.good) p.accent else p.text), fontSize = 15.sp, fontWeight = FontWeight.Bold),
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun Note(text: String, p: Palette, context: Context) {
    Text(
        text,
        style = TextStyle(color = ColorProvider(p.muted), fontSize = 13.sp),
        modifier = GlanceModifier.fillMaxWidth().clickable(actionStartActivity(openApp(context))),
    )
}

/** Tapping the time on the widget fetches the prices again. */
class RefreshWidget : ActionCallback {
    override suspend fun onAction(context: Context, glanceId: GlanceId, parameters: ActionParameters) {
        WidgetWorker.runNow(context)
    }
}

class DealsWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget: GlanceAppWidget = DealsWidget()

    override fun onEnabled(context: Context) {
        super.onEnabled(context)
        WidgetWorker.schedule(context)
        WidgetWorker.runNow(context)
    }

    override fun onDisabled(context: Context) {
        super.onDisabled(context)
        WidgetWorker.cancel(context)
    }
}

/** Keeps the widget current while one is on the home screen: once an hour, when the server can be reached. */
class WidgetWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val app = applicationContext as App
        if (!app.session.state.value.signedIn) {
            DealsWidget.redraw(app)
            return Result.success()
        }
        return try {
            DealsWidget.refresh(app)
            Result.success()
        } catch (e: ApiException) {
            // Away from the server, or signed out there: what is shown stays until the next hour.
            Result.success()
        }
    }

    companion object {
        private const val WORK_NAME = "widget"
        private val online = Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()

        fun schedule(context: Context) {
            val request = PeriodicWorkRequestBuilder<WidgetWorker>(1, TimeUnit.HOURS).setConstraints(online).build()
            WorkManager.getInstance(context).enqueueUniquePeriodicWork(WORK_NAME, ExistingPeriodicWorkPolicy.KEEP, request)
        }

        fun runNow(context: Context) {
            val request = OneTimeWorkRequestBuilder<WidgetWorker>().setConstraints(online).build()
            WorkManager.getInstance(context).enqueueUniqueWork("$WORK_NAME-now", ExistingWorkPolicy.REPLACE, request)
        }

        fun cancel(context: Context) {
            WorkManager.getInstance(context).cancelUniqueWork(WORK_NAME)
        }
    }
}
