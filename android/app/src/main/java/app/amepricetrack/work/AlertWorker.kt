package app.amepricetrack.work

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import app.amepricetrack.App
import app.amepricetrack.MainActivity
import app.amepricetrack.R
import app.amepricetrack.data.Alert
import app.amepricetrack.data.ApiException
import java.util.concurrent.TimeUnit

/**
 * Asks the server for new alerts every so often (half an hour unless the
 * person chose otherwise) and shows them as notifications. One small request
 * each time; when the server cannot be reached it simply waits for the next
 * turn rather than trying again and again.
 */
class AlertWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {

    override suspend fun doWork(): Result {
        val app = applicationContext as App
        if (!app.session.state.value.signedIn) return Result.success()
        return try {
            check(app)
            Result.success()
        } catch (e: ApiException) {
            // Away from home, or signed out elsewhere: the next turn is soon enough.
            Result.success()
        }
    }

    companion object {
        const val CHANNEL = "alerts"
        const val EXTRA_PRODUCT_ID = "product_id"
        private const val WORK_NAME = "alerts"

        fun createChannel(context: Context) {
            val channel =
                NotificationChannel(CHANNEL, context.getString(R.string.alerts_channel), NotificationManager.IMPORTANCE_DEFAULT)
            channel.description = context.getString(R.string.alerts_channel_description)
            context.getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
        }

        /** Checks every `minutes` (15 is the shortest Android allows); 0 stops checking. */
        fun schedule(context: Context, minutes: Int) {
            if (minutes <= 0) {
                cancel(context)
                return
            }
            val constraints = Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()
            val request = PeriodicWorkRequestBuilder<AlertWorker>(maxOf(minutes, 15).toLong(), TimeUnit.MINUTES)
                .setConstraints(constraints)
                .build()
            // UPDATE keeps the running schedule and only changes how often.
            WorkManager.getInstance(context).enqueueUniquePeriodicWork(WORK_NAME, ExistingPeriodicWorkPolicy.UPDATE, request)
        }

        fun cancel(context: Context) {
            WorkManager.getInstance(context).cancelUniqueWork(WORK_NAME)
        }

        fun canNotify(context: Context): Boolean {
            val granted = Build.VERSION.SDK_INT < 33 ||
                ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
            return granted && NotificationManagerCompat.from(context).areNotificationsEnabled()
        }

        /**
         * Fetches alerts newer than the last one shown and notifies about the
         * unread ones. The very first check only notes where to start, so
         * signing in does not replay old alerts. Returns how many were shown.
         */
        suspend fun check(app: App): Int {
            val session = app.session
            val fresh = app.api.alerts(after = session.lastAlertId).alerts
            val newest = fresh.maxOfOrNull { it.id } ?: session.lastAlertId
            if (!session.alertsStarted) {
                session.alertsStarted = true
                session.lastAlertId = newest
                return 0
            }
            val toShow = newAlerts(fresh, session.lastAlertId)
            session.lastAlertId = maxOf(newest, session.lastAlertId)
            if (!canNotify(app)) return 0
            toShow.forEach { show(app, it) }
            return toShow.size
        }

        /** The unread alerts newer than `lastShown`, oldest first. */
        fun newAlerts(fresh: List<Alert>, lastShown: Long): List<Alert> =
            fresh.filter { !it.read && it.id > lastShown }.sortedBy { it.id }

        @Suppress("MissingPermission") // checked by canNotify before this is called
        private fun show(context: Context, alert: Alert) {
            val open = Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
                .putExtra(EXTRA_PRODUCT_ID, alert.productId ?: -1L)
            val pending = PendingIntent.getActivity(
                context,
                alert.id.toInt(),
                open,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            val notification = NotificationCompat.Builder(context, CHANNEL)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle(alert.title)
                .setContentText(alert.body.lineSequence().firstOrNull().orEmpty())
                .setStyle(NotificationCompat.BigTextStyle().bigText(alert.body))
                .setContentIntent(pending)
                .setAutoCancel(true)
                .build()
            NotificationManagerCompat.from(context).notify(alert.id.toInt(), notification)
        }
    }
}
