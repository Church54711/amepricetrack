package app.amepricetrack.ui

import android.text.format.DateUtils
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.amepricetrack.App
import app.amepricetrack.data.ApiException

/** Something being fetched from the server. */
sealed interface Load<out T> {
    data object Loading : Load<Nothing>
    data class Failed(val message: String) : Load<Nothing>
    data class Ready<T>(val value: T) : Load<T>
}

/**
 * Runs a server call and turns a failure into words to show. When the server
 * no longer accepts this device, the person is sent back to the sign-in screen.
 */
suspend fun <T> App.attempt(call: suspend () -> T): Result<T> = try {
    Result.success(call())
} catch (e: ApiException) {
    if (e.signedOut) session.signOut()
    Result.failure(e)
}

fun Result<*>.problem(): String = exceptionOrNull()?.message ?: "Something went wrong."

/** "5 minutes ago", from a server time in seconds. */
fun ago(seconds: Long): String =
    DateUtils.getRelativeTimeSpanString(seconds * 1000, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS).toString()

@Composable
fun Card(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    val p = LocalPalette.current
    Surface(
        modifier = modifier.fillMaxWidth(),
        color = p.surface,
        contentColor = p.text,
        shape = RoundedCornerShape(12.dp),
        border = BorderStroke(1.dp, p.border),
    ) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp), content = content)
    }
}

@Composable
fun SectionTitle(text: String) {
    Text(text, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
}

@Composable
fun Muted(text: String, modifier: Modifier = Modifier) {
    Text(text, modifier = modifier, color = LocalPalette.current.muted, style = MaterialTheme.typography.bodyMedium)
}

enum class TagKind { Good, Warn, Bad, Member, Plain }

/** A small coloured label such as "Lowest price" or "Out of stock". */
@Composable
fun Tag(text: String, kind: TagKind) {
    val p = LocalPalette.current
    val (fg, bg) = when (kind) {
        TagKind.Good -> p.good to p.goodSoft
        TagKind.Warn -> p.warn to p.warnSoft
        TagKind.Bad -> p.bad to p.badSoft
        TagKind.Member -> p.member to p.border
        TagKind.Plain -> p.muted to p.border
    }
    Text(
        text,
        color = fg,
        fontSize = 12.sp,
        fontWeight = FontWeight.SemiBold,
        modifier = Modifier.background(bg, RoundedCornerShape(6.dp)).padding(horizontal = 7.dp, vertical = 2.dp),
    )
}

@Composable
fun LoadingBox() {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
}

@Composable
fun ProblemBox(message: String, onRetry: () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(message, color = LocalPalette.current.text)
        Button(onClick = onRetry) { Text("Try again") }
    }
}

@Composable
fun Banner(message: String, good: Boolean) {
    val p = LocalPalette.current
    Text(
        message,
        color = if (good) p.good else p.bad,
        modifier = Modifier
            .fillMaxWidth()
            .background(if (good) p.goodSoft else p.badSoft, RoundedCornerShape(10.dp))
            .padding(12.dp),
    )
}
