package app.amepricetrack.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance

/** The colours of one theme. The values mirror the website's stylesheet. */
data class Palette(
    val dark: Boolean,
    val bg: Color,
    val surface: Color,
    val border: Color,
    val text: Color,
    val muted: Color,
    val accent: Color,
    val accentText: Color,
    val good: Color,
    val goodSoft: Color,
    val warn: Color,
    val warnSoft: Color,
    val bad: Color,
    val badSoft: Color,
    val member: Color,
)

val OledGold = Palette(
    dark = true,
    bg = Color(0xFF000000),
    surface = Color(0xFF0A0A0A),
    border = Color(0xFF272727),
    text = Color(0xFFF3EFE4),
    muted = Color(0xFFA39D8D),
    accent = Color(0xFFE8B923),
    accentText = Color(0xFF000000),
    good = Color(0xFF5FD38D),
    goodSoft = Color(0xFF0C2A19),
    warn = Color(0xFFF0B354),
    warnSoft = Color(0xFF2E2208),
    bad = Color(0xFFFF8A80),
    badSoft = Color(0xFF3A1512),
    member = Color(0xFFC4A1FF),
)

val Midnight = Palette(
    dark = true,
    bg = Color(0xFF0F1218),
    surface = Color(0xFF181C25),
    border = Color(0xFF2C3342),
    text = Color(0xFFE8EBF1),
    muted = Color(0xFF98A2B5),
    accent = Color(0xFF5B8CFF),
    accentText = Color(0xFF0B1020),
    good = Color(0xFF4CCB8A),
    goodSoft = Color(0xFF14321F),
    warn = Color(0xFFF0B354),
    warnSoft = Color(0xFF3A2C10),
    bad = Color(0xFFFF8A80),
    badSoft = Color(0xFF3D1A17),
    member = Color(0xFFB794FF),
)

val Light = Palette(
    dark = false,
    bg = Color(0xFFF5F6F8),
    surface = Color(0xFFFFFFFF),
    border = Color(0xFFDDE1E8),
    text = Color(0xFF171A21),
    muted = Color(0xFF5D6677),
    accent = Color(0xFF2563EB),
    accentText = Color(0xFFFFFFFF),
    good = Color(0xFF0F7B47),
    goodSoft = Color(0xFFDCF5E7),
    warn = Color(0xFF9A5B00),
    warnSoft = Color(0xFFFDF0D5),
    bad = Color(0xFFB3261E),
    badSoft = Color(0xFFFBE3E1),
    member = Color(0xFF7C3AED),
)

/** "#rrggbb" -> a colour, or null when the text is not one. */
fun parseHexColour(text: String?): Color? {
    val hex = text?.trim()?.removePrefix("#") ?: return null
    if (hex.length != 6 || !hex.all { it.isDigit() || it.lowercaseChar() in 'a'..'f' }) return null
    return Color(0xFF000000 or hex.toLong(16))
}

/**
 * The palette for a theme id from the server, with the person's own accent
 * colour when they chose one. Text on the accent is black or white,
 * whichever is easier to read, as on the website.
 */
fun paletteFor(theme: String, accent: String?, systemDark: Boolean): Palette {
    val base = when (theme) {
        "midnight" -> Midnight
        "light" -> Light
        "system" -> if (systemDark) Midnight else Light
        else -> OledGold
    }
    val custom = parseHexColour(accent) ?: return base
    return base.copy(accent = custom, accentText = if (custom.luminance() > 0.179f) Color.Black else Color.White)
}

val LocalPalette = staticCompositionLocalOf { OledGold }

@Composable
fun AppTheme(theme: String, accent: String?, content: @Composable () -> Unit) {
    val p = paletteFor(theme, accent, isSystemInDarkTheme())
    val scheme = if (p.dark) {
        darkColorScheme(
            primary = p.accent, onPrimary = p.accentText,
            secondary = p.accent, onSecondary = p.accentText,
            background = p.bg, onBackground = p.text,
            surface = p.surface, onSurface = p.text,
            surfaceVariant = p.surface, onSurfaceVariant = p.muted,
            surfaceContainer = p.surface, surfaceContainerHigh = p.surface, surfaceContainerHighest = p.surface,
            surfaceContainerLow = p.surface, surfaceContainerLowest = p.bg,
            outline = p.border, outlineVariant = p.border,
            error = p.bad, onError = p.bg,
            primaryContainer = p.accent, onPrimaryContainer = p.accentText,
            secondaryContainer = p.border, onSecondaryContainer = p.text,
        )
    } else {
        lightColorScheme(
            primary = p.accent, onPrimary = p.accentText,
            secondary = p.accent, onSecondary = p.accentText,
            background = p.bg, onBackground = p.text,
            surface = p.surface, onSurface = p.text,
            surfaceVariant = p.surface, onSurfaceVariant = p.muted,
            surfaceContainer = p.surface, surfaceContainerHigh = p.surface, surfaceContainerHighest = p.surface,
            surfaceContainerLow = p.surface, surfaceContainerLowest = p.bg,
            outline = p.border, outlineVariant = p.border,
            error = p.bad, onError = p.surface,
            primaryContainer = p.accent, onPrimaryContainer = p.accentText,
            secondaryContainer = p.border, onSecondaryContainer = p.text,
        )
    }
    CompositionLocalProvider(LocalPalette provides p) {
        MaterialTheme(colorScheme = scheme, content = content)
    }
}
