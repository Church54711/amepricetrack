@file:OptIn(ExperimentalMaterial3Api::class)

package app.amepricetrack.ui

import android.Manifest
import android.content.Intent
import android.net.Uri
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.amepricetrack.App
import app.amepricetrack.data.Alert
import app.amepricetrack.data.AppRelease
import app.amepricetrack.data.ServerUpdate
import app.amepricetrack.data.UpdateStep
import app.amepricetrack.data.Updater
import app.amepricetrack.data.newNtfyAddress
import app.amepricetrack.data.ntfyAppLink
import app.amepricetrack.work.AlertWorker
import androidx.glance.appwidget.GlanceAppWidgetManager
import app.amepricetrack.widget.DealsWidget
import app.amepricetrack.widget.DealsWidgetReceiver
import app.amepricetrack.widget.WidgetStore
import app.amepricetrack.work.ReportWorker
import kotlinx.coroutines.launch

// ------------------------------------------------------------------ alerts

@Composable
fun AlertsScreen(app: App, onOpen: (Long) -> Unit, onUnread: (Int) -> Unit) {
    var state by remember { mutableStateOf<Load<List<Alert>>>(Load.Loading) }
    var refreshing by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    suspend fun load() {
        val result = app.attempt { app.api.alerts() }
        result.onSuccess {
            state = Load.Ready(it.alerts)
            onUnread(it.unread)
            // Whatever is listed here does not need a notification any more.
            it.alerts.maxOfOrNull { alert -> alert.id }?.let { newest ->
                if (newest > app.session.lastAlertId) app.session.lastAlertId = newest
            }
            app.session.alertsStarted = true
        }
        result.onFailure { if (state !is Load.Ready) state = Load.Failed(result.problem()) }
    }

    LaunchedEffect(Unit) { load() }

    when (val current = state) {
        Load.Loading -> LoadingBox()
        is Load.Failed -> ProblemBox(current.message) {
            state = Load.Loading
            scope.launch { load() }
        }
        is Load.Ready -> PullToRefreshBox(
            isRefreshing = refreshing,
            onRefresh = {
                refreshing = true
                scope.launch {
                    load()
                    refreshing = false
                }
            },
        ) {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(16.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                item {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
                        Text("Alerts", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
                        if (current.value.any { !it.read }) {
                            TextButton(onClick = {
                                scope.launch {
                                    app.attempt { app.api.markAlertsRead() }
                                    load()
                                }
                            }) { Text("Mark all read") }
                        }
                    }
                }
                if (current.value.isEmpty()) {
                    item {
                        Card {
                            SectionTitle("No alerts yet")
                            Muted("Price drops, targets reached and items back in stock appear here and as notifications.")
                        }
                    }
                }
                items(current.value, key = { it.id }) { alert ->
                    Card(if (alert.productId != null) Modifier.clickable { onOpen(alert.productId) } else Modifier) {
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                            if (!alert.read) Tag("New", TagKind.Good)
                            Text(alert.title, fontWeight = FontWeight.SemiBold)
                        }
                        Text(alert.body)
                        Muted(ago(alert.ts))
                    }
                }
            }
        }
    }
}

// --------------------------------------------------------------- assistant

private data class ChatLine(val mine: Boolean, val text: String)

@Composable
fun AssistantScreen(app: App) {
    val lines = remember { mutableStateListOf<ChatLine>() }
    var draft by rememberSaveable { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val listState = rememberLazyListState()

    fun send() {
        val question = draft.trim()
        if (question.isEmpty() || busy) return
        draft = ""
        lines += ChatLine(true, question)
        busy = true
        scope.launch {
            val result = app.attempt { app.api.chat(question) }
            lines += ChatLine(false, result.getOrNull() ?: result.problem())
            busy = false
        }
    }

    LaunchedEffect(lines.size, busy) {
        if (lines.isNotEmpty()) listState.animateScrollToItem(lines.size - 1 + if (busy) 1 else 0)
    }

    Column(Modifier.fillMaxSize().imePadding()) {
        LazyColumn(
            Modifier.weight(1f).fillMaxWidth(),
            state = listState,
            contentPadding = PaddingValues(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            item { Text("Assistant", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold) }
            if (lines.isEmpty()) {
                item {
                    Muted(
                        "Ask about the prices you track, for example “What is the best deal right now?” " +
                            "It uses the AI service connected on the website's Settings page.",
                    )
                }
            }
            items(lines.size) { index ->
                val line = lines[index]
                val p = LocalPalette.current
                Box(Modifier.fillMaxWidth(), contentAlignment = if (line.mine) Alignment.CenterEnd else Alignment.CenterStart) {
                    Text(
                        line.text,
                        color = if (line.mine) p.accentText else p.text,
                        modifier = Modifier
                            .widthIn(max = 320.dp)
                            .background(if (line.mine) p.accent else p.surface, RoundedCornerShape(14.dp))
                            .padding(horizontal = 12.dp, vertical = 8.dp),
                    )
                }
            }
            if (busy) item { Muted("Thinking…") }
        }
        Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
            OutlinedTextField(
                value = draft,
                onValueChange = { draft = it },
                placeholder = { Text("Ask a question") },
                maxLines = 4,
                modifier = Modifier.weight(1f),
            )
            IconButton(onClick = ::send, enabled = !busy && draft.isNotBlank()) {
                Icon(Icons.AutoMirrored.Filled.Send, contentDescription = "Send", tint = LocalPalette.current.accent)
            }
        }
    }
}

// ---------------------------------------------------------------- settings

/**
 * Alerts pushed to the phone the moment the server has them, through the
 * ntfy app, so this app does not have to ask the server every so often.
 */
@Composable
private fun InstantAlertsCard(app: App, checksInBackground: Boolean, onStopChecking: () -> Unit, onPushOff: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    // Null until the server has said where alerts are pushed.
    var saved by remember { mutableStateOf<String?>(null) }
    var draft by rememberSaveable { mutableStateOf(newNtfyAddress()) }
    var note by remember { mutableStateOf<Pair<String, Boolean>?>(null) }
    var busy by remember { mutableStateOf(false) }

    LaunchedEffect(Unit) {
        val result = app.attempt { app.api.pushSettings() }
        result.onSuccess { saved = it.ntfyUrl }
        result.onFailure { note = result.problem() to false }
    }

    fun run(done: String?, action: suspend () -> Unit) {
        busy = true
        scope.launch {
            val result = app.attempt { action() }
            busy = false
            note = if (result.isSuccess) done?.let { it to true } else result.problem() to false
        }
    }

    Card {
        SectionTitle("Instant alerts")
        note?.let { (text, good) -> Banner(text, good = good) }
        val address = saved
        when {
            address == null -> Muted("Asking your server\u2026")
            address.isEmpty() -> {
                Muted(
                    "Have alerts pushed to this phone the moment your server sees a price drop, instead of the app asking every so often. " +
                        "This uses ntfy, a free notification app. Alerts then pass through the ntfy server in the address below, and anyone " +
                        "who knows the topic name could read them, so it is a long random one.",
                )
                OutlinedTextField(
                    value = draft,
                    onValueChange = { draft = it.trim() },
                    label = { Text("ntfy topic address") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                    modifier = Modifier.fillMaxWidth(),
                )
                Button(enabled = !busy, onClick = { run(null) { saved = app.api.setNtfy(draft).ntfyUrl } }) { Text("Turn on") }
            }
            else -> {
                Text("Your server pushes alerts to", fontWeight = FontWeight.Medium)
                Muted(address)
                Text("1. Subscribe in the ntfy app", fontWeight = FontWeight.Medium)
                Muted("Opens ntfy on this topic. If ntfy is not installed yet, its store page opens: install it, then press this again.")
                Button(onClick = {
                    val link = ntfyAppLink(address)
                    val opened = link != null && runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(link))) }.isSuccess
                    if (!opened) {
                        val store = runCatching {
                            context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("market://details?id=io.heckel.ntfy")))
                        }
                        note = if (store.isSuccess) {
                            "Install ntfy, then come back and press \u201cSubscribe\u201d again." to true
                        } else {
                            "Install the ntfy app (from Google Play or F-Droid), then subscribe to the address above in it." to false
                        }
                    }
                }) { Text("Subscribe in ntfy") }
                Text("2. Try it", fontWeight = FontWeight.Medium)
                OutlinedButton(enabled = !busy, onClick = {
                    run("Sent. It should appear as a notification from ntfy within a few seconds.") { app.api.testPush() }
                }) { Text("Send a test alert") }
                Text("3. Let ntfy do the waiting", fontWeight = FontWeight.Medium)
                if (checksInBackground) {
                    Muted("Once the test arrived, this app no longer needs to ask your server by itself, and each alert would otherwise show twice.")
                    OutlinedButton(onClick = onStopChecking) { Text("Stop checking in the background") }
                } else {
                    Muted("This app does not check in the background. ntfy brings the alerts; \u201cOpen in app\u201d on one shows the product here.")
                }
                TextButton(enabled = !busy, onClick = {
                    run("Your server no longer pushes alerts through ntfy.") {
                        saved = app.api.setNtfy("").ntfyUrl
                        draft = address
                        onPushOff()
                    }
                }) { Text("Turn off") }
            }
        }
    }
}

/** This app's version and the server's, each with one button to update it. */
@Composable
private fun UpdatesCard(app: App) {
    val scope = rememberCoroutineScope()
    val updater = remember { Updater(app) }
    val step by Updater.step.collectAsStateWithLifecycle()
    var looking by remember { mutableStateOf(false) }
    // Null until looked for; a release when a newer one exists.
    var looked by remember { mutableStateOf(false) }
    var release by remember { mutableStateOf<AppRelease?>(null) }
    var server by remember { mutableStateOf<ServerUpdate?>(null) }
    var note by remember { mutableStateOf<Pair<String, Boolean>?>(null) }
    var serverStarted by remember { mutableStateOf(false) }

    suspend fun look() {
        looking = true
        note = null
        try {
            release = updater.newer()
            looked = true
        } catch (e: Exception) {
            note = (e.message ?: "Could not look for updates.") to false
        }
        app.attempt { app.api.serverUpdate() }.onSuccess { server = it }
        looking = false
    }

    LaunchedEffect(Unit) { look() }

    Card {
        SectionTitle("Updates")
        note?.let { (text, good) -> Banner(text, good = good) }

        Text("This app: version ${updater.installedVersion}", fontWeight = FontWeight.Medium)
        val found = release
        when (val now = step) {
            is UpdateStep.Downloading -> Muted("Downloading\u2026 ${now.percent}%")
            UpdateStep.Installing -> Muted("Installing. Android may ask you to confirm; the app then restarts.")
            is UpdateStep.Failed -> Banner(now.message, good = false)
            UpdateStep.Idle -> when {
                found != null -> Muted("Version ${found.version} is available.")
                looked -> Muted("This is the latest version.")
                else -> {}
            }
        }
        if (found != null && step !is UpdateStep.Downloading && step != UpdateStep.Installing) {
            Button(onClick = { scope.launch { updater.install(found) } }) { Text("Update to ${found.version}") }
        }

        server?.let { s ->
            Text("Your server: version ${s.version}", fontWeight = FontWeight.Medium)
            when {
                serverStarted -> Muted("The server is updating and restarts by itself. It is away for about a minute.")
                s.newer != null && s.canInstall -> {
                    Muted("Version ${s.newer} is available.")
                    Button(onClick = {
                        scope.launch {
                            val asked = app.attempt { app.api.updateServer() }
                            if (asked.isSuccess) serverStarted = true else note = asked.problem() to false
                        }
                    }) { Text("Update the server to ${s.newer}") }
                }
                s.newer != null -> Muted("Version ${s.newer} is available. On the server, run: sudo amepricetrack-update")
                else -> {}
            }
            s.lastProblem?.let { Muted("The server's last update did not go through: $it") }
        }

        OutlinedButton(enabled = !looking, onClick = { scope.launch { look() } }) {
            Text(if (looking) "Looking\u2026" else "Look for updates")
        }
    }
}

private fun themeName(id: String) = when (id) {
    "midnight" -> "Midnight Blue"
    "light" -> "Light"
    "system" -> "Match my device"
    else -> "OLED Black & Gold"
}

@Composable
fun SettingsScreen(app: App, onOpenPhone: () -> Unit) {
    val session by app.session.state.collectAsStateWithLifecycle()
    var canNotify by remember { mutableStateOf(AlertWorker.canNotify(app)) }
    var message by remember { mutableStateOf<String?>(null) }
    var alertEvery by remember { mutableStateOf(app.session.alertEveryMinutes) }
    // Whether the phone reports prices by itself; read again whenever this screen comes back.
    var autoReport by remember { mutableStateOf(app.session.autoReport) }
    LifecycleResumeEffect(Unit) {
        autoReport = app.session.autoReport
        onPauseOrDispose {}
    }
    val scope = rememberCoroutineScope()
    val askPermission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        canNotify = AlertWorker.canNotify(app)
        if (!granted) message = "Notifications are off for this app. You can allow them in Android's settings for AmePriceTrack."
    }

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Settings", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
        message?.let { Banner(it, good = false) }

        Card {
            SectionTitle("Account")
            Text("Signed in as ${session.username.orEmpty()}")
            Muted(session.serverUrl.orEmpty())
            OutlinedButton(onClick = {
                scope.launch {
                    // Tell the server to forget this device; sign out here either way.
                    app.attempt { app.api.logout() }
                    AlertWorker.cancel(app)
                    ReportWorker.cancel(app)
                    app.session.signOut()
                    // The home-screen widget must not keep showing this person's list.
                    WidgetStore.clear(app)
                    runCatching { DealsWidget.redraw(app) }
                }
            }) { Text("Sign out") }
        }

        Card {
            SectionTitle("Notifications")
            if (canNotify) {
                Text("Check for new alerts", fontWeight = FontWeight.Medium)
                val choices = listOf(15 to "15 min", 30 to "30 min", 60 to "Hourly", 0 to "Off")
                SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                    choices.forEachIndexed { index, (minutes, label) ->
                        SegmentedButton(
                            selected = alertEvery == minutes,
                            onClick = {
                                alertEvery = minutes
                                app.session.alertEveryMinutes = minutes
                                AlertWorker.schedule(app, minutes)
                            },
                            shape = SegmentedButtonDefaults.itemShape(index, choices.size),
                            icon = {},
                        ) { Text(label, maxLines = 1) }
                    }
                }
                Muted(
                    if (alertEvery == 0) {
                        "The app does not check in the background. Alerts still show when you open it, and your server can push them through ntfy."
                    } else {
                        "Each check is one small request, made while the phone can reach your server. Your server looks at each price " +
                            "every few hours, so checking more often than half an hour gains little."
                    },
                )
            } else {
                Muted("Allow notifications to hear about price drops without opening the app.")
                Button(onClick = {
                    if (Build.VERSION.SDK_INT >= 33) {
                        askPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
                    } else {
                        message = "Notifications are off for this app. You can allow them in Android's settings for AmePriceTrack."
                    }
                }) { Text("Allow notifications") }
            }
        }

        InstantAlertsCard(
            app = app,
            checksInBackground = canNotify && alertEvery != 0,
            onStopChecking = {
                alertEvery = 0
                app.session.alertEveryMinutes = 0
                AlertWorker.schedule(app, 0)
            },
            onPushOff = {
                // Without pushed alerts the app has to ask again, or alerts would only show when it is opened.
                if (alertEvery == 0) {
                    alertEvery = 30
                    app.session.alertEveryMinutes = 30
                    AlertWorker.schedule(app, 30)
                }
            },
        )

        Card {
            SectionTitle("Prices from this phone")
            Muted(
                "A fallback for what your server cannot read itself. Signing in at a store, or choosing your local store, is better " +
                    "done in the server's own browser from the website's Stores page; then the phone has nothing to do.",
            )
            Button(onClick = onOpenPhone) { Text("Open") }
            Muted(
                when (autoReport) {
                    "off" -> "The phone does not report by itself. Your server does the reading; this is for what it cannot do."
                    "daily" -> "The phone also reports by itself once a day. Open to change how, or which stores."
                    else -> "The phone also reports by itself twice a day. Open to change how, or which stores."
                },
            )
        }

        Card {
            SectionTitle("Appearance")
            Muted("This phone can look like the website or have a theme of its own. Other devices are not affected.")
            val website = themeName(session.theme) + if (session.accent != null) ", with your accent colour" else ""
            val choices = listOf<Pair<String?, String>>(null to "Same as the website ($website)") +
                listOf("oled-gold", "midnight", "light", "system").map { it to themeName(it) }
            choices.forEach { (id, label) ->
                Row(
                    Modifier.fillMaxWidth().clickable { app.session.setDeviceTheme(id) },
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    RadioButton(selected = session.deviceTheme == id, onClick = { app.session.setDeviceTheme(id) })
                    Text(label)
                }
            }
            if (session.deviceTheme == null) {
                OutlinedButton(onClick = {
                    scope.launch {
                        val result = app.attempt { app.api.me() }
                        result.onSuccess { app.session.setLook(it.theme, it.accent) }
                        result.onFailure { message = result.problem() }
                    }
                }) { Text("Update from the website") }
            }
        }

        Card {
            SectionTitle("Home screen widget")
            Muted(
                "\u201cBest prices\u201d shows what has reached its target, what is at its lowest and the biggest drops, " +
                    "without opening the app. It refreshes about once an hour while the phone can reach your server.",
            )
            OutlinedButton(onClick = {
                scope.launch {
                    val asked = runCatching { GlanceAppWidgetManager(app).requestPinGlanceAppWidget(DealsWidgetReceiver::class.java) }
                    if (asked.getOrNull() != true) {
                        message = "This home screen does not take widgets from apps. Add it by hand: press and hold the home screen, then choose Widgets."
                    }
                }
            }) { Text("Add to the home screen") }
        }

        UpdatesCard(app)
    }
}
