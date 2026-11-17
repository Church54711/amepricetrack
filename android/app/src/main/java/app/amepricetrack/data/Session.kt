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
    /** A custom accent colour as #rrggbb, when one was chosen. */
    val accent: String? = null,
) {
    val signedIn: Boolean get() = serverUrl != null && token != null
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

    /** The newest alert already shown as a notification. */
    var lastAlertId: Long
        get() = prefs.getLong("last_alert", 0)
        set(value) = prefs.edit().putLong("last_alert", value).apply()

    /** False until the first background check, which only notes where to start. */
    var alertsStarted: Boolean
        get() = prefs.getBoolean("alerts_started", false)
        set(value) = prefs.edit().putBoolean("alerts_started", value).apply()

    /** How often the phone reports prices by itself: "off", "daily" or "twice" (a day). */
    var autoReport: String
        get() = prefs.getString("auto_report", null) ?: "twice"
        set(value) = prefs.edit().putString("auto_report", value).apply()

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
