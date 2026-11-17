package app.amepricetrack.data

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import app.amepricetrack.BuildConfig
import app.amepricetrack.MainActivity
import app.amepricetrack.R
import app.amepricetrack.work.AlertWorker
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.withContext
import okhttp3.OkHttpClient
import okhttp3.Request
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.util.concurrent.TimeUnit

/** A published version of the app. */
data class AppRelease(val version: String, val apkUrl: String, val bytes: Long)

/** The numbers of a version such as "v1.2.3" or "1.2", or null when it is not one. */
fun versionNumbers(text: String): List<Int>? {
    val parts = text.trim().removePrefix("v").substringBefore('-').split('.')
    if (parts.isEmpty() || parts.size > 4) return null
    return parts.map { it.toIntOrNull()?.takeIf { n -> n >= 0 } ?: return null }
}

/** True when `candidate` is a later version than `installed`. */
fun isNewerVersion(candidate: String, installed: String): Boolean {
    val (a, b) = (versionNumbers(candidate) ?: return false) to (versionNumbers(installed) ?: return false)
    for (i in 0 until maxOf(a.size, b.size)) {
        val (x, y) = a.getOrElse(i) { 0 } to b.getOrElse(i) { 0 }
        if (x != y) return x > y
    }
    return false
}

/** Reads GitHub's description of a release; null when it carries no app to install. */
fun parseAppRelease(json: JSONObject): AppRelease? {
    val version = json.optString("tag_name").removePrefix("v")
    if (versionNumbers(version) == null) return null
    val assets = json.optJSONArray("assets") ?: return null
    for (i in 0 until assets.length()) {
        val asset = assets.optJSONObject(i) ?: continue
        val address = asset.optString("browser_download_url")
        if (asset.optString("name").endsWith(".apk", ignoreCase = true) && address.startsWith("https://")) {
            return AppRelease(version, address, asset.optLong("size"))
        }
    }
    return null
}

/** What the server says about its own version. */
data class ServerUpdate(
    val version: String,
    /** The newer version that has been published, when there is one and this person may know. */
    val newer: String?,
    /** True when the server can install it by itself, at this person's word. */
    val canInstall: Boolean,
    /** Why the last update asked for did not go through, when it did not. */
    val lastProblem: String?,
)

fun parseServerUpdate(json: JSONObject): ServerUpdate {
    val last = json.optJSONObject("last")
    return ServerUpdate(
        version = json.optString("version"),
        newer = json.optJSONObject("newer")?.optString("version")?.takeIf { it.isNotEmpty() },
        canInstall = json.optBoolean("can_install"),
        lastProblem = last?.takeIf { it.optString("state") == "failed" }?.optString("message")?.takeIf { it.isNotEmpty() },
    )
}

/** What the updater is doing, for the Settings screen. */
sealed interface UpdateStep {
    data object Idle : UpdateStep
    data class Downloading(val percent: Int) : UpdateStep
    /** Handed to Android, which may ask the person to confirm. */
    data object Installing : UpdateStep
    data class Failed(val message: String) : UpdateStep
}

/** Finds a newer version of the app where releases are published, downloads it and has Android install it. */
class Updater(private val context: Context, private val releasesUrl: String = RELEASES_URL) {
    companion object {
        const val RELEASES_URL = "https://api.github.com/repos/${BuildConfig.RELEASES_REPO}/releases/latest"
        private const val MAX_BYTES = 200L * 1024 * 1024

        private val _step = MutableStateFlow<UpdateStep>(UpdateStep.Idle)
        val step: StateFlow<UpdateStep> = _step

        internal fun report(step: UpdateStep) {
            _step.value = step
        }

        private val http: OkHttpClient = OkHttpClient.Builder()
            .connectTimeout(15, TimeUnit.SECONDS)
            .readTimeout(60, TimeUnit.SECONDS)
            .build()
    }

    val installedVersion: String get() = BuildConfig.VERSION_NAME

    /** The published version, when it is newer than this one; null when this one is current. */
    suspend fun newer(): AppRelease? = withContext(Dispatchers.IO) {
        val request = Request.Builder().url(releasesUrl).header("Accept", "application/vnd.github+json").build()
        val text = try {
            http.newCall(request).execute().use { response ->
                // No release has been published yet.
                if (response.code == 404) return@withContext null
                if (!response.isSuccessful) throw ApiException("Could not look for updates (${response.code}). Try again later.")
                response.body?.string().orEmpty()
            }
        } catch (e: IOException) {
            throw ApiException("Could not look for updates. Check the internet connection.")
        }
        val release = runCatching { parseAppRelease(JSONObject(text)) }.getOrNull()
        release?.takeIf { isNewerVersion(it.version, installedVersion) }
    }

    /** Downloads the release and hands it to Android. Progress and problems show in [step]. */
    suspend fun install(release: AppRelease) {
        report(UpdateStep.Downloading(0))
        try {
            val file = download(release)
            report(UpdateStep.Installing)
            handToAndroid(file)
        } catch (e: ApiException) {
            report(UpdateStep.Failed(e.message ?: "The update could not be installed."))
        } catch (e: Exception) {
            report(UpdateStep.Failed("The update could not be installed (${e.message ?: e.javaClass.simpleName})."))
        }
    }

    private suspend fun download(release: AppRelease): File = withContext(Dispatchers.IO) {
        val folder = File(context.cacheDir, "updates").apply { mkdirs() }
        folder.listFiles()?.forEach { it.delete() }
        val file = File(folder, "update.apk")
        try {
            http.newCall(Request.Builder().url(release.apkUrl).build()).execute().use { response ->
                if (!response.isSuccessful) throw ApiException("The download failed (${response.code}). Try again later.")
                val body = response.body ?: throw ApiException("The download was empty.")
                val total = body.contentLength().takeIf { it > 0 } ?: release.bytes
                if (total > MAX_BYTES) throw ApiException("The download is larger than an app update should be.")
                body.byteStream().use { input ->
                    file.outputStream().use { output ->
                        val buffer = ByteArray(64 * 1024)
                        var done = 0L
                        while (true) {
                            val read = input.read(buffer)
                            if (read < 0) break
                            output.write(buffer, 0, read)
                            done += read
                            if (done > MAX_BYTES) throw ApiException("The download is larger than an app update should be.")
                            if (total > 0) report(UpdateStep.Downloading((done * 100 / total).toInt().coerceIn(0, 100)))
                        }
                    }
                }
            }
        } catch (e: IOException) {
            throw ApiException("The download was interrupted. Check the internet connection and try again.")
        }
        file
    }

    /**
     * Android checks that the file is this same app, signed with the same key,
     * before it replaces anything; a file from anywhere else is refused.
     */
    private suspend fun handToAndroid(file: File) = withContext(Dispatchers.IO) {
        val installer = context.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(context.packageName)
            setSize(file.length())
            // After the first update Android can install later ones without asking again.
            if (Build.VERSION.SDK_INT >= 31) setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
        }
        val id = installer.createSession(params)
        installer.openSession(id).use { session ->
            session.openWrite("app", 0, file.length()).use { out ->
                file.inputStream().use { it.copyTo(out) }
                session.fsync(out)
            }
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or (if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0)
            val result = PendingIntent.getBroadcast(context, id, Intent(context, UpdateReceiver::class.java), flags)
            session.commit(result.intentSender)
        }
    }
}

/** What Android's installer said in words. */
fun installProblem(status: Int, detail: String?): String = when (status) {
    PackageInstaller.STATUS_FAILURE_ABORTED -> "The update was cancelled."
    PackageInstaller.STATUS_FAILURE_STORAGE -> "There is not enough free space to install the update."
    PackageInstaller.STATUS_FAILURE_CONFLICT, PackageInstaller.STATUS_FAILURE_INCOMPATIBLE ->
        "This copy of the app cannot be updated in place, because it was installed from a different build. " +
            "Uninstall it, then install the new version from the releases page once. Updates work from then on."
    PackageInstaller.STATUS_FAILURE_BLOCKED -> "This phone blocked the update. Allow AmePriceTrack to install apps in Android's settings."
    else -> "The update could not be installed" + if (detail.isNullOrBlank()) "." else " ($detail)."
}

/**
 * Android closes the app while it replaces it and does not open it again.
 * This runs once the new version is in, and says so with a notification
 * that opens the app.
 */
class UpdatedReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_MY_PACKAGE_REPLACED || !AlertWorker.canNotify(context)) return
        AlertWorker.createChannel(context)
        val open = PendingIntent.getActivity(
            context,
            0,
            Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(context, AlertWorker.CHANNEL)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("AmePriceTrack was updated")
            .setContentText("Version ${BuildConfig.VERSION_NAME} is installed. Tap to open.")
            .setContentIntent(open)
            .setAutoCancel(true)
            .build()
        runCatching { NotificationManagerCompat.from(context).notify(UPDATED_NOTIFICATION, notification) }
    }

    private companion object {
        const val UPDATED_NOTIFICATION = -2
    }
}

/** Hears from Android how an installation went. */
class UpdateReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                // Android wants the person to confirm; show its question.
                @Suppress("DEPRECATION")
                val confirm = if (Build.VERSION.SDK_INT >= 33) {
                    intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                } else {
                    intent.getParcelableExtra(Intent.EXTRA_INTENT)
                }
                val shown = confirm != null && runCatching { context.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }.isSuccess
                if (!shown) Updater.report(UpdateStep.Failed("Android could not ask to install the update. Open the app and try again."))
            }
            // The app is replaced and restarted by Android; nothing is left to show.
            PackageInstaller.STATUS_SUCCESS -> Updater.report(UpdateStep.Idle)
            else -> Updater.report(UpdateStep.Failed(installProblem(status, intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE))))
        }
    }
}
