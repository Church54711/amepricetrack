@file:OptIn(ExperimentalMaterial3Api::class)

package app.amepricetrack.ui

import android.net.Uri
import android.view.ViewGroup
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import app.amepricetrack.App
import app.amepricetrack.data.CollectEntry
import app.amepricetrack.work.PageReader
import app.amepricetrack.work.PhoneReport
import app.amepricetrack.work.ReportWorker
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Switch
import kotlinx.coroutines.launch

/** A setting with a switch, its name and what it means. */
@Composable
private fun SwitchRow(label: String, detail: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(label, fontWeight = FontWeight.Medium)
            Muted(detail)
        }
        Switch(checked = checked, onCheckedChange = onChange)
    }
}

/** The in-app browser, kept for as long as this screen is open. */
@Composable
private fun rememberStoreBrowser(): PageReader {
    val context = LocalContext.current
    val browser = remember { PageReader.create(context) }
    DisposableEffect(Unit) { onDispose { browser.close() } }
    return browser
}

@Composable
fun PhoneScreen(app: App, onBack: () -> Unit) {
    var state by remember { mutableStateOf<Load<List<CollectEntry>>>(Load.Loading) }
    // The store page the person is browsing, when they are.
    var browsing by remember { mutableStateOf<String?>(null) }
    var running by remember { mutableStateOf(false) }
    var current by remember { mutableStateOf<String?>(null) }
    val results = remember { mutableStateMapOf<Long, String>() }
    val browser = rememberStoreBrowser()
    val scope = rememberCoroutineScope()
    // The automatic-report choices, kept on this phone.
    var autoReport by remember { mutableStateOf(app.session.autoReport) }
    var wifiOnly by remember { mutableStateOf(app.session.reportOnWifiOnly) }
    var whileCharging by remember { mutableStateOf(app.session.reportWhileCharging) }
    var skipped by remember { mutableStateOf(app.session.reportSkippedStores) }
    var visited by remember { mutableStateOf(app.session.visitedStores) }

    suspend fun load() {
        val result = app.attempt { app.api.collect() }
        state = result.fold({ Load.Ready(it) }, { Load.Failed(result.problem()) })
    }

    LaunchedEffect(Unit) { load() }

    fun report(entries: List<CollectEntry>) {
        running = true
        results.clear()
        // A price is text: pictures and video are left out while reading.
        browser.lean = true
        scope.launch {
            for (entry in entries) {
                current = entry.url
                results[entry.productId] = PhoneReport.reportOne(app, browser, entry).text
            }
            current = null
            running = false
            browser.rest()
        }
    }

    BackHandler(enabled = browsing != null) {
        if (browser.view.canGoBack()) browser.view.goBack() else browsing = null
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(if (browsing != null) "Store" else "Prices from this phone") },
                navigationIcon = {
                    IconButton(onClick = { if (browsing != null) browsing = null else onBack() }, enabled = !running) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                actions = { if (browsing != null) TextButton(onClick = { browsing = null }) { Text("Done") } },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
            )
        },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize()) {
            // One browser for both jobs: full height while the person browses, a strip while pages are read.
            // It is only on screen while in use, and never draws outside its own area.
            if (browsing != null || running) {
                val size = if (browsing != null) Modifier.fillMaxWidth().weight(1f) else Modifier.fillMaxWidth().height(260.dp)
                AndroidView(
                    factory = {
                        (browser.view.parent as? ViewGroup)?.removeView(browser.view)
                        browser.view
                    },
                    modifier = size.clipToBounds(),
                )
            }
            if (browsing != null) return@Column

            when (val loaded = state) {
                Load.Loading -> LoadingBox()
                is Load.Failed -> ProblemBox(loaded.message) {
                    state = Load.Loading
                    scope.launch { load() }
                }
                is Load.Ready -> Column(
                    Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    val entries = loaded.value
                    val needed = entries.count { it.needed }
                    Muted(
                        "The phone opens each product page the way you would, and sends the page to your server so it can read the price. " +
                            "Your store passwords and sign-ins never leave this phone.",
                    )
                    if (entries.isEmpty()) {
                        Card { Muted("Nothing is being tracked yet.") }
                    } else {
                        Card {
                            SectionTitle("Report prices")
                            if (needed > 0) {
                                Muted("Your server cannot read $needed of your products itself. This phone can.")
                            }
                            Button(onClick = { report(entries) }, enabled = !running) {
                                Text(if (running) "Reading pages…" else "Report all ${entries.size} now")
                            }
                            if (needed in 1 until entries.size) {
                                OutlinedButton(onClick = { report(entries.filter { it.needed }) }, enabled = !running) {
                                    Text("Only the $needed the server cannot read")
                                }
                            }
                            Muted("Keep this screen open while it works; each page takes a few seconds.")
                        }

                        Card {
                            SectionTitle("Report by itself")
                            Muted(
                                "Your server reads prices itself, and that is the place for it. Turn this on only for what the server cannot do: " +
                                    "a store that refuses it, or one you are signed in at on this phone only.",
                            )
                            val choices = listOf("off" to "Off", "daily" to "Daily", "twice" to "Twice a day")
                            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                                choices.forEachIndexed { index, (mode, label) ->
                                    SegmentedButton(
                                        selected = autoReport == mode,
                                        onClick = {
                                            autoReport = mode
                                            app.session.autoReport = mode
                                            ReportWorker.schedule(app, app.session)
                                        },
                                        shape = SegmentedButtonDefaults.itemShape(index, choices.size),
                                        // No tick: the three labels need the room on a narrow phone.
                                        icon = {},
                                    ) { Text(label, maxLines = 1) }
                                }
                            }
                            if (autoReport != "off") {
                                SwitchRow("Only on Wi-Fi", "Where your home server usually is. Also keeps it off mobile data.", wifiOnly) {
                                    wifiOnly = it
                                    app.session.reportOnWifiOnly = it
                                    ReportWorker.schedule(app, app.session)
                                }
                                SwitchRow("Only while charging", "Costs the battery nothing, but runs less often.", whileCharging) {
                                    whileCharging = it
                                    app.session.reportWhileCharging = it
                                    ReportWorker.schedule(app, app.session)
                                }
                                Muted(
                                    "It never runs on a low battery, reads at most ${PhoneReport.MAX_AUTOMATIC} pages a time without pictures or video, " +
                                        "and skips anything read recently. Choose the stores below." +
                                        (app.session.lastReportSummary?.let { " Last time, ${ago(app.session.lastReportAt / 1000)}: $it" } ?: ""),
                                )
                            }
                        }

                        Card {
                            SectionTitle("Stores")
                            Muted(
                                "Open a store to sign in or choose your local store on this phone. After that, its prices here are treated as your own." +
                                    if (autoReport != "off") " The switch decides whether a store is part of the automatic reports." else "",
                            )
                            val plan = PhoneReport.Plan(visited, skipped, emptyMap(), 0)
                            entries.groupBy { it.storeName }.forEach { (store, products) ->
                                val home = Uri.parse(products.first().url).let { "${it.scheme}://${it.host}/" }
                                val host = PhoneReport.hostOf(home)
                                val part = PhoneReport.partOf(products, plan)
                                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                                    Column(Modifier.weight(1f)) {
                                        Text(store, fontWeight = FontWeight.Medium)
                                        val count = if (products.size == 1) "1 product" else "${products.size} products"
                                        Muted(
                                            count + when (part) {
                                                PhoneReport.Part.ServerCannot -> ". Your server cannot read it."
                                                PhoneReport.Part.OpenedHere -> ". You opened it on this phone."
                                                PhoneReport.Part.NotNeeded -> ". Your server reads it; nothing for the phone to do."
                                                PhoneReport.Part.TurnedOff -> ". Left out of automatic reports."
                                            },
                                        )
                                    }
                                    // Only stores the phone would report have something to switch off.
                                    if (autoReport != "off" && part != PhoneReport.Part.NotNeeded) {
                                        Switch(
                                            checked = part != PhoneReport.Part.TurnedOff,
                                            onCheckedChange = { on ->
                                                skipped = if (on) skipped - host else skipped + host
                                                app.session.reportSkippedStores = skipped
                                            },
                                        )
                                    }
                                    OutlinedButton(
                                        enabled = !running,
                                        onClick = {
                                            visited = visited + host
                                            app.session.visitedStores = visited
                                            // A person browsing wants the whole page, pictures included.
                                            browser.lean = false
                                            browser.view.loadUrl(home)
                                            browsing = home
                                        },
                                    ) { Text("Open") }
                                }
                            }
                        }

                        if (results.isNotEmpty() || running) {
                            Card {
                                SectionTitle("Results")
                                entries.forEach { entry ->
                                    val outcome = results[entry.productId] ?: if (current == entry.url) "Reading…" else null
                                    if (outcome != null) {
                                        Column {
                                            Text(entry.title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                            Muted("${entry.storeName}: $outcome")
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
