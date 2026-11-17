package app.amepricetrack.data

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/** Who is signed in, where, and how the app should look. */
data class SessionState(
    val serverUrl: String? = null,
    val token: String? = null,
    val username: String? = null,
    /** The theme chosen on the website: oled-gold, midnight, light or system. */
    val theme: String = "oled-gold",
    /** The custom accent colour chosen on the website as #rrggbb, when there is one. */
    val accent: String? = null,
    /** The theme chosen for this device alone, or null to look like the website. */
    val deviceTheme: String? = null,
) {
    val signedIn: Boolean get() = serverUrl != null && token != null

    /** The theme this device shows. */
    val shownTheme: String get() = deviceTheme ?: theme

    /** The accent this device shows. The website's own accent goes with the website's theme only. */
    val shownAccent: String? get() = if (deviceTheme == null) accent else null
}

/**
 * Keeps the sign-in on the device. The token lives in the app's private
 * storage, which other apps cannot read and which is excluded from backups.
 */
class Session(context: Context) {
    private val prefs = context.getSharedPreferences("session", Context.MODE_PRIVATE)
    private val _state = MutableStateFlow(load())
    val state: StateFlow<SessionState> = _state

    private fun load() = SessionState(
        serverUrl = prefs.getString("server", null),
        token = prefs.getString("token", null),
        username = prefs.getString("username", null),
        theme = prefs.getString("theme", null) ?: "oled-gold",
        accent = prefs.getString("accent", null),
        deviceTheme = prefs.getString("device_theme", null),
    )

    fun signIn(serverUrl: String, token: String, username: String) {
        prefs.edit().putString("server", serverUrl).putString("token", token).putString("username", username).apply()
        _state.value = _state.value.copy(serverUrl = serverUrl, token = token, username = username)
    }

    /** Forgets the sign-in but keeps the server address, to make signing in again quick. */
    fun signOut() {
        prefs.edit().remove("token").remove("last_alert").remove("alerts_started").apply()
        _state.value = _state.value.copy(token = null)
    }

    fun setLook(theme: String, accent: String?) {
        if (theme == _state.value.theme && accent == _state.value.accent) return
        prefs.edit().putString("theme", theme).putString("accent", accent).apply()
        _state.value = _state.value.copy(theme = theme, accent = accent)
    }

    /** Chooses a theme for this device alone; null goes back to looking like the website. */
    fun setDeviceTheme(theme: String?) {
        if (theme == null) prefs.edit().remove("device_theme").apply() else prefs.edit().putString("device_theme", theme).apply()
        _state.value = _state.value.copy(deviceTheme = theme)
    }

    /** The newest alert already shown as a notification. */
    var lastAlertId: Long
        get() = prefs.getLong("last_alert", 0)
        set(value) = prefs.edit().putLong("last_alert", value).apply()

    /** False until the first background check, which only notes where to start. */
    var alertsStarted: Boolean
        get() = prefs.getBoolean("alerts_started", false)
        set(value) = prefs.edit().putBoolean("alerts_started", value).apply()

    /**
     * How often the phone reports prices by itself: "off", "daily" or "twice"
     * (a day). Off unless the person turns it on: the server is the one that
     * reads prices, and the phone only helps where the server cannot.
     */
    var autoReport: String
        get() = prefs.getString("auto_report", null) ?: "off"
        set(value) = prefs.edit().putString("auto_report", value).apply()

    /** Automatic reports only run on Wi-Fi, where the home server usually is. */
    var reportOnWifiOnly: Boolean
        get() = prefs.getBoolean("report_wifi_only", true)
        set(value) = prefs.edit().putBoolean("report_wifi_only", value).apply()

    /** Automatic reports only run while the phone is charging. */
    var reportWhileCharging: Boolean
        get() = prefs.getBoolean("report_charging", false)
        set(value) = prefs.edit().putBoolean("report_charging", value).apply()

    /** Stores (site names) left out of automatic reports. */
    var reportSkippedStores: Set<String>
        get() = prefs.getStringSet("report_skipped", emptySet()) ?: emptySet()
        set(value) = prefs.edit().putStringSet("report_skipped", value).apply()

    /** When this phone last reported each product, in seconds: product id -> time. */
    var reportedAt: Map<Long, Long>
        get() = prefs.getString("reported_at", null).orEmpty().split(',').mapNotNull { pair ->
            val (id, at) = pair.split(':').takeIf { it.size == 2 } ?: return@mapNotNull null
            (id.toLongOrNull() ?: return@mapNotNull null) to (at.toLongOrNull() ?: return@mapNotNull null)
        }.toMap()
        set(value) {
            // Only the most recent few hundred are worth remembering.
            val kept = value.entries.sortedByDescending { it.value }.take(300)
            prefs.edit().putString("reported_at", kept.joinToString(",") { "${it.key}:${it.value}" }).apply()
        }

    /** How often the app asks the server for new alerts, in minutes; 0 for never. */
    var alertEveryMinutes: Int
        get() = prefs.getInt("alert_every", 30)
        set(value) = prefs.edit().putInt("alert_every", value).apply()

    /** How often the server checks a product at its usual pace, in minutes, as it last said. */
    var checkIntervalMinutes: Int
        get() = prefs.getInt("check_interval", 240)
        set(value) = prefs.edit().putInt("check_interval", value).apply()

    /** When the phone last reported prices by itself, in milliseconds; 0 for never. */
    var lastReportAt: Long
        get() = prefs.getLong("last_report_at", 0)
        set(value) = prefs.edit().putLong("last_report_at", value).apply()

    /** What the last automatic report did, in words. */
    var lastReportSummary: String?
        get() = prefs.getString("last_report_summary", null)
        set(value) = prefs.edit().putString("last_report_summary", value).apply()

    /** Stores the person opened in the in-app browser, where they may be signed in. */
    var visitedStores: Set<String>
        get() = prefs.getStringSet("visited_stores", emptySet()) ?: emptySet()
        set(value) = prefs.edit().putStringSet("visited_stores", value).apply()
}
