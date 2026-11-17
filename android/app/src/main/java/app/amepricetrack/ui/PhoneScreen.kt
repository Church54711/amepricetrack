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
import kotlinx.coroutines.launch

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

    suspend fun load() {
        val result = app.attempt { app.api.collect() }
        state = result.fold({ Load.Ready(it) }, { Load.Failed(result.problem()) })
    }

    LaunchedEffect(Unit) { load() }

    fun report(entries: List<CollectEntry>) {
        running = true
        results.clear()
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
                            Muted(
                                when (app.session.autoReport) {
                                    "off" -> "Automatic reports are off. You can turn them on in Settings."
                                    else -> app.session.lastReportSummary?.let { summary ->
                                        "Last automatic report, ${ago(app.session.lastReportAt / 1000)}: $summary"
                                    } ?: "The phone also reports by itself; see Settings."
                                },
                            )
                        }

                        Card {
                            SectionTitle("Stores")
                            Muted("Open a store to sign in or choose your local store. After that, its prices here are treated as your own.")
                            entries.groupBy { it.storeName }.forEach { (store, products) ->
                                val home = Uri.parse(products.first().url).let { "${it.scheme}://${it.host}/" }
                                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
                                    Column(Modifier.weight(1f)) {
                                        Text(store, fontWeight = FontWeight.Medium)
                                        Muted(if (products.size == 1) "1 product" else "${products.size} products")
                                    }
                                    OutlinedButton(
                                        enabled = !running,
                                        onClick = {
                                            app.session.visitedStores = app.session.visitedStores + PhoneReport.hostOf(home)
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
