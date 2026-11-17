@file:OptIn(ExperimentalMaterial3Api::class)

package app.amepricetrack.ui

import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Notifications
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.ShoppingCart
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import app.amepricetrack.App
import app.amepricetrack.Incoming
import app.amepricetrack.data.SessionState
import app.amepricetrack.data.normaliseServerAddress
import app.amepricetrack.work.AlertWorker
import app.amepricetrack.work.ReportWorker
import kotlinx.coroutines.launch

private const val TAB_WATCHLIST = "watchlist"
private const val TAB_ALERTS = "alerts"
private const val TAB_ASSISTANT = "assistant"
private const val TAB_SETTINGS = "settings"


/** The whole app: the sign-in screen, or the four tabs with detail screens on top. */
@Composable
fun AppRoot(app: App, session: SessionState, incoming: Incoming?, onIncomingHandled: () -> Unit) {
    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        if (!session.signedIn) {
            LoginScreen(app, session)
        } else {
            SignedIn(app, incoming, onIncomingHandled)
        }
    }
}

@Composable
private fun SignedIn(app: App, incoming: Incoming?, onIncomingHandled: () -> Unit) {
    var tab by rememberSaveable { mutableStateOf(TAB_WATCHLIST) }
    // Detail screens opened on top of the tabs: "product:12", "phone", "find:copy paper" or "match:12:34".
    var stack by rememberSaveable { mutableStateOf(listOf<String>()) }
    var unread by remember { mutableIntStateOf(0) }
    // Raised to make the watchlist load again after something was added elsewhere.
    var watchlistVersion by remember { mutableIntStateOf(0) }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()

    fun open(screen: String) {
        stack = stack + screen
    }

    BackHandler(enabled = stack.isNotEmpty()) { stack = stack.dropLast(1) }

    // The look chosen on the website, and how many alerts are waiting.
    LaunchedEffect(Unit) {
        app.attempt { app.api.me() }.onSuccess {
            app.session.setLook(it.theme, it.accent)
            app.session.checkIntervalMinutes = it.checkIntervalMinutes
            unread = it.unreadAlerts
        }
        AlertWorker.schedule(app, app.session.alertEveryMinutes)
        // Only does anything when the person turned automatic reports on.
        ReportWorker.schedule(app, app.session)
    }

    LaunchedEffect(incoming) {
        when (incoming) {
            is Incoming.OpenProduct -> {
                open("product:${incoming.id}")
                onIncomingHandled()
            }
            is Incoming.SharedText -> {
                val text = incoming.text
                // Marking it handled restarts this effect, so the adding runs on its own.
                onIncomingHandled()
                scope.launch {
                    launch { snackbar.showSnackbar("Adding it to your watchlist…") }
                    val added = app.attempt { app.api.addWatch(text) }
                    added.onSuccess {
                        watchlistVersion++
                        open("product:${it.product.id}")
                    }
                    added.onFailure { snackbar.showSnackbar(added.problem()) }
                }
            }
            null -> {}
        }
    }

    val top = stack.lastOrNull()
    if (top != null) {
        val back = { stack = stack.dropLast(1) }
        when {
            top.startsWith("product:") -> ProductScreen(
                app = app,
                productId = top.removePrefix("product:").toLong(),
                onBack = back,
                onRemoved = {
                    watchlistVersion++
                    back()
                },
                onPriceMatch = { comparison -> open("match:${top.removePrefix("product:")}:$comparison") },
            )
            top == "phone" -> PhoneScreen(app, onBack = back)
            top.startsWith("find:") -> FindScreen(
                app = app,
                initial = top.removePrefix("find:"),
                onBack = back,
                onOpenProduct = { open("product:$it") },
                onAdded = {
                    watchlistVersion++
                    // The new product takes the place of the search.
                    stack = stack.dropLast(1) + "product:$it"
                },
            )
            top.startsWith("match:") -> {
                val (product, comparison) = top.removePrefix("match:").split(":").map { it.toLong() }
                PriceMatchScreen(app, productId = product, comparisonId = comparison, onBack = back)
            }
        }
        return
    }

    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        bottomBar = {
            NavigationBar(containerColor = LocalPalette.current.surface) {
                val colours = NavigationBarItemDefaults.colors(
                    selectedIconColor = LocalPalette.current.accentText,
                    selectedTextColor = LocalPalette.current.accent,
                    indicatorColor = LocalPalette.current.accent,
                    unselectedIconColor = LocalPalette.current.muted,
                    unselectedTextColor = LocalPalette.current.muted,
                )
                NavigationBarItem(
                    selected = tab == TAB_WATCHLIST,
                    onClick = { tab = TAB_WATCHLIST },
                    icon = { Icon(Icons.Filled.ShoppingCart, contentDescription = null) },
                    label = { Text("Watchlist") },
                    colors = colours,
                )
                NavigationBarItem(
                    selected = tab == TAB_ALERTS,
                    onClick = { tab = TAB_ALERTS },
                    icon = {
                        BadgedBox(badge = { if (unread > 0) Badge { Text(minOf(unread, 99).toString()) } }) {
                            Icon(Icons.Filled.Notifications, contentDescription = null)
                        }
                    },
                    label = { Text("Alerts") },
                    colors = colours,
                )
                NavigationBarItem(
                    selected = tab == TAB_ASSISTANT,
                    onClick = { tab = TAB_ASSISTANT },
                    icon = { Icon(Icons.AutoMirrored.Filled.Send, contentDescription = null) },
                    label = { Text("Assistant") },
                    colors = colours,
                )
                NavigationBarItem(
                    selected = tab == TAB_SETTINGS,
                    onClick = { tab = TAB_SETTINGS },
                    icon = { Icon(Icons.Filled.Settings, contentDescription = null) },
                    label = { Text("Settings") },
                    colors = colours,
                )
            }
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize()) {
            when (tab) {
                TAB_WATCHLIST -> WatchlistScreen(
                    app = app,
                    version = watchlistVersion,
                    onOpen = { open("product:$it") },
                    onFind = { open("find:$it") },
                    onMessage = { scope.launch { snackbar.showSnackbar(it) } },
                )
                TAB_ALERTS -> AlertsScreen(app, onOpen = { open("product:$it") }, onUnread = { unread = it })
                TAB_ASSISTANT -> AssistantScreen(app)
                else -> SettingsScreen(app, onOpenPhone = { open("phone") })
            }
        }
    }
}

@Composable
private fun LoginScreen(app: App, session: SessionState) {
    var server by rememberSaveable { mutableStateOf(session.serverUrl.orEmpty()) }
    var username by rememberSaveable { mutableStateOf(session.username.orEmpty()) }
    var password by rememberSaveable { mutableStateOf("") }
    var problem by rememberSaveable { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    fun signIn() {
        val address = normaliseServerAddress(server)
        if (address == null) {
            problem = "Type the server's address, for example 192.168.1.20:8787"
            return
        }
        if (username.isBlank() || password.isEmpty()) {
            problem = "Type your username and password."
            return
        }
        busy = true
        problem = null
        scope.launch {
            val device = "${Build.MANUFACTURER} ${Build.MODEL}".trim()
            val result = app.attempt { app.api.login(address, username.trim(), password, device) }
            busy = false
            result.onSuccess { app.session.signIn(address, it, username.trim()) }
            result.onFailure { problem = result.problem() }
        }
    }

    Column(
        Modifier.fillMaxSize().safeDrawingPadding().imePadding().verticalScroll(rememberScrollState()).padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Text("AmePriceTrack", style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.Bold, color = LocalPalette.current.accent)
        Muted("Sign in to your own AmePriceTrack server with the same username and password as on its website.")
        problem?.let { Banner(it, good = false) }
        OutlinedTextField(
            value = server,
            onValueChange = { server = it },
            label = { Text("Server address") },
            placeholder = { Text("192.168.1.20:8787") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = username,
            onValueChange = { username = it },
            label = { Text("Username") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = password,
            onValueChange = { password = it },
            label = { Text("Password") },
            singleLine = true,
            visualTransformation = PasswordVisualTransformation(),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
            modifier = Modifier.fillMaxWidth(),
        )
        Button(onClick = ::signIn, enabled = !busy, modifier = Modifier.fillMaxWidth()) {
            Text(if (busy) "Signing in…" else "Sign in")
        }
        Muted("The address is the one you open the website with. The phone has to be able to reach it, for example on the same Wi-Fi.")
    }
}
