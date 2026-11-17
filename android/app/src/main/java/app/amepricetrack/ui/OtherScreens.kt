@file:OptIn(ExperimentalMaterial3Api::class)

package app.amepricetrack.ui

import android.Manifest
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
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.amepricetrack.App
import app.amepricetrack.data.Alert
import app.amepricetrack.work.AlertWorker
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
    var autoReport by remember { mutableStateOf(app.session.autoReport) }
    // What the last automatic report did; read again whenever the app comes back to the front.
    var lastReport by remember { mutableStateOf<String?>(null) }
    LifecycleResumeEffect(Unit) {
        lastReport = app.session.lastReportSummary?.let { "Last time, ${ago(app.session.lastReportAt / 1000)}: $it" }
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
                }
            }) { Text("Sign out") }
        }

        Card {
            SectionTitle("Notifications")
            if (canNotify) {
                Muted("New alerts are checked about every 15 minutes while the phone can reach your server.")
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

        Card {
            SectionTitle("Prices from this phone")
            Muted(
                "Sign in at a store inside the app, and the phone can report the prices that store shows you: " +
                    "member and employee prices, your own Walmart store, and stores your server cannot read.",
            )
            Button(onClick = onOpenPhone) { Text("Open") }

            Text("Report by itself", fontWeight = FontWeight.Medium)
            val choices = listOf("off" to "Off", "daily" to "Daily", "twice" to "Twice a day")
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                choices.forEachIndexed { index, (mode, label) ->
                    SegmentedButton(
                        selected = autoReport == mode,
                        onClick = {
                            autoReport = mode
                            app.session.autoReport = mode
                            ReportWorker.schedule(app, mode)
                        },
                        shape = SegmentedButtonDefaults.itemShape(index, choices.size),
                        // No tick: the three labels need the room on a narrow phone.
                        icon = {},
                    ) { Text(label, maxLines = 1) }
                }
            }
            Muted(
                if (autoReport == "off") {
                    "The phone only reports prices when you open the screen above and start it."
                } else {
                    "In the background, the phone reads the products your server cannot read itself, and the ones at stores you " +
                        "opened in the app, and sends those prices to your server." +
                        (lastReport?.let { " $it" } ?: "")
                },
            )
        }

        Card {
            SectionTitle("Appearance")
            Text(themeName(session.theme) + if (session.accent != null) ", with your own accent colour" else "")
            Muted("The app uses the theme chosen on the website's Settings page.")
            OutlinedButton(onClick = {
                scope.launch {
                    val result = app.attempt { app.api.me() }
                    result.onSuccess { app.session.setLook(it.theme, it.accent) }
                    result.onFailure { message = result.problem() }
                }
            }) { Text("Update from the website") }
        }
    }
}
