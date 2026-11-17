//! Shared page furniture: the layout, small display pieces, and the price chart.

use crate::db::{PricePoint, User};
use crate::pricing::dollars;
use crate::retailers;
use crate::service::WatchView;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use time::macros::format_description;

pub const CSS: &str = include_str!("../static/app.css");
pub const JS: &str = include_str!("../static/app.js");
pub const ICON: &str = include_str!("../../../assets/logo-mark.svg");

/// Changes whenever the stylesheet or script changes, so a browser never
/// keeps using an old copy after the server is updated.
static ASSET_VERSION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    CSS.hash(&mut hasher);
    JS.hash(&mut hasher);
    format!("{:08x}", hasher.finish() as u32)
});

/// A one-off message shown at the top of the next page.
pub struct Flash {
    pub ok: bool,
    pub text: String,
}

pub struct Chrome<'a> {
    pub user: &'a User,
    pub unread: i64,
    pub flash: Option<Flash>,
    /// One of `THEMES`.
    pub theme: &'static str,
    /// Custom accent colour (#rrggbb), when the person chose one.
    pub accent: Option<String>,
}

pub struct Theme {
    pub id: &'static str,
    pub label: &'static str,
}

/// The colour themes. Their colours live in app.css under `data-theme`.
pub const THEMES: &[Theme] = &[
    Theme { id: "oled-gold", label: "OLED Black & Gold" },
    Theme { id: "midnight", label: "Midnight Blue" },
    Theme { id: "light", label: "Light" },
    Theme { id: "system", label: "Match my device" },
];

pub const DEFAULT_THEME: &str = "oled-gold";

/// The stored theme id, or the default when it is empty or unknown.
pub fn theme_id(stored: &str) -> &'static str {
    THEMES.iter().find(|t| t.id == stored).map(|t| t.id).unwrap_or(DEFAULT_THEME)
}

/// Accepts a colour written as #rrggbb (or rrggbb) and returns it as lowercase #rrggbb.
pub fn accent_colour(text: &str) -> Option<String> {
    let hex = text.trim().trim_start_matches('#');
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| format!("#{}", hex.to_ascii_lowercase()))
}

/// The stylesheet that applies a custom accent colour. Text placed on the
/// accent (buttons) is made black or white, whichever is easier to read.
pub fn accent_css(colour: &str) -> String {
    let channel = |i: usize| {
        let value = u8::from_str_radix(&colour[1 + i * 2..3 + i * 2], 16).unwrap_or(0) as f64 / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * channel(0) + 0.7152 * channel(1) + 0.0722 * channel(2);
    let on_accent = if luminance > 0.179 { "#000000" } else { "#ffffff" };
    // The doubled selector outranks the theme blocks in app.css.
    format!("html:root[data-theme] {{ --accent: {colour}; --accent-text: {on_accent}; }}\n")
}

fn head(title: &str, accent: Option<&str>) -> Markup {
    html! {
        head {
            meta charset="utf-8";
            meta name="viewport" content="width=device-width, initial-scale=1";
            title { (title) " · AmePriceTrack" }
            link rel="icon" href="/favicon.svg" type="image/svg+xml";
            link rel="stylesheet" href={ "/static/app.css?v=" (*ASSET_VERSION) };
            @if let Some(colour) = accent {
                link rel="stylesheet" href={ "/theme.css?c=" (colour.trim_start_matches('#')) };
            }
            script src={ "/static/app.js?v=" (*ASSET_VERSION) } defer {}
        }
    }
}

fn brand_mark() -> Markup {
    html! { span.mark aria-hidden="true" { "A" } }
}

/// A full signed-in page. `active` is the nav entry to highlight.
pub fn page(chrome: &Chrome, active: &str, title: &str, body: Markup) -> Markup {
    let nav: &[(&str, &str, &str)] = &[
        ("watchlist", "/", "Watchlist"),
        ("deals", "/deals", "Deals"),
        ("alerts", "/alerts", "Alerts"),
        ("discounts", "/discounts", "Discounts"),
        ("stores", "/stores", "Stores"),
        ("assistant", "/assistant", "Assistant"),
        ("settings", "/settings", "Settings"),
    ];
    html! {
        (DOCTYPE)
        html lang="en" data-theme=(chrome.theme) {
            (head(title, chrome.accent.as_deref()))
            body {
                header.topbar {
                    div.topbar-inner {
                        a.brand href="/" { (brand_mark()) "AmePriceTrack" }
                        nav.nav aria-label="Main" {
                            @for (id, href, label) in nav {
                                a href=(href) class=[(*id == active).then_some("active")] aria-current=[(*id == active).then_some("page")] {
                                    (label)
                                    @if *id == "alerts" && chrome.unread > 0 {
                                        span.count aria-label={ (chrome.unread) " unread" } { (chrome.unread.min(99)) }
                                    }
                                }
                            }
                        }
                        div.who {
                            span.muted.small { (chrome.user.username) }
                            form method="post" action="/logout" { button.link type="submit" { "Sign out" } }
                        }
                    }
                }
                main {
                    @if let Some(flash) = &chrome.flash {
                        div.flash.ok[flash.ok].err[!flash.ok] role="status" { (flash.text) }
                    }
                    (body)
                }
            }
        }
    }
}

/// The bare page used for signing in and first-time setup.
pub fn auth_page(title: &str, error: Option<&str>, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" data-theme=(DEFAULT_THEME) {
            (head(title, None))
            body {
                div.auth {
                    div.brand { (brand_mark()) "AmePriceTrack" }
                    div.card {
                        h1 { (title) }
                        @if let Some(error) = error {
                            div.flash.err role="alert" { (error) }
                        }
                        (body)
                    }
                }
            }
        }
    }
}

/// A plain page for "not found" and server faults.
pub fn message_page(title: &str, text: &str) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" data-theme=(DEFAULT_THEME) {
            (head(title, None))
            body {
                div.auth {
                    div.card {
                        h1 { (title) }
                        p { (text) }
                        a.btn href="/" { "Back to the watchlist" }
                    }
                }
            }
        }
    }
}

/// A timestamp. Shown in UTC, then rewritten in the viewer's time zone by app.js.
pub fn when(ts: i64, date_only: bool) -> Markup {
    let Ok(dt) = time::OffsetDateTime::from_unix_timestamp(ts) else {
        return html! { "unknown" };
    };
    let text = if date_only {
        dt.format(format_description!("[year]-[month]-[day]"))
    } else {
        dt.format(format_description!("[year]-[month]-[day] [hour]:[minute] UTC"))
    }
    .unwrap_or_default();
    html! { time data-ts=(ts) data-style=[date_only.then_some("date")] { (text) } }
}

pub fn ago(now: i64, then: i64) -> String {
    let secs = (now - then).max(0);
    match secs {
        s if s < 90 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 7200 => "1 hour ago".into(),
        s if s < 2 * 86_400 => format!("{} hours ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

pub fn store_name(id: &str) -> String {
    retailers::display_name(id)
}

pub fn thumb(image_url: Option<&str>) -> Markup {
    html! {
        @match image_url {
            Some(url) => { img.thumb src=(url) alt="" loading="lazy" referrerpolicy="no-referrer"; }
            None => { div.thumb.empty {} }
        }
    }
}

/// The right-hand price column of a watchlist row.
pub fn price_cell(v: &WatchView) -> Markup {
    let personal = v.price.is_personal();
    html! {
        @match v.price.your_cents {
            Some(cents) => {
                div.price.personal[personal] { (dollars(cents)) }
                @if personal {
                    @if let Some(public) = v.price.public_cents { div.was { (dollars(public)) } }
                    div.price-note {
                        @if let (Some(name), Some(pct)) = (&v.price.discount_name, v.price.discount_percent) {
                            (name) " (" (pct) "% off)"
                        } @else { "Signed-in price" }
                    }
                } @else if let Some(list) = v.product.list_price_cents {
                    div.was { (dollars(list)) }
                }
            }
            None => { div.muted.small { "No price yet" } }
        }
    }
}

/// Small badges describing a watched product's state.
pub fn badges(v: &WatchView) -> Markup {
    let p = &v.product;
    html! {
        @if p.in_stock == Some(false) { span.tag.bad { "Out of stock" } }
        @if let (Some(other), Some(mine)) = (&v.elsewhere, v.price.your_cents) {
            @if other.cents < mine { span.tag.warn { (dollars(other.cents)) " at " (other.store_name) } }
        }
        @if v.deal.at_lowest { span.tag.good { "Lowest recorded" } }
        @if let Some(pct) = v.deal.percent_below.filter(|pct| *pct >= 1.0) {
            span.tag.good {
                (format!("{pct:.0}%")) " below "
                @if v.deal.compared_to == Some("average") { "usual" } @else { "list price" }
            }
        }
        @if v.price.member_cents.is_some() && v.price.is_personal() { span.tag.member { "Signed-in price" } }
        @if let (Some(target), Some(now)) = (v.watch.target_price_cents, v.price.your_cents) {
            @if now <= target { span.tag.good { "At your target" } }
        }
        @if p.current_price_cents.is_none() && v.price.your_cents.is_none() {
            @if p.last_error.is_some() { span.tag.bad { "Could not read price" } }
            @else { span.tag.warn { "Checking\u{2026}" } }
        } @else if p.last_error.is_some() && p.fail_count >= 3 {
            span.tag.warn { "Price may be out of date" }
        }
    }
}

/// The stretches of time a chart can show: (what the link says, its name in the address, days).
pub const CHART_RANGES: &[(&str, &str, i64)] = &[("1 month", "30", 30), ("3 months", "90", 90), ("1 year", "365", 365)];

/// The points to draw for the time since `since`: those inside it, and the
/// price that already held when it began.
fn in_range(points: &[PricePoint], since: i64) -> Vec<PricePoint> {
    let mut shown: Vec<PricePoint> = Vec::new();
    for member in [false, true] {
        let mut held: Option<PricePoint> = None;
        for p in points.iter().filter(|p| p.member == member) {
            if p.ts < since {
                held = Some(PricePoint { ts: since, ..p.clone() });
            } else {
                shown.extend(held.take());
                shown.push(p.clone());
            }
        }
        // Nothing changed inside the range: the price that held is still the price.
        if !member {
            shown.extend(held);
        }
    }
    shown.sort_by_key(|p| p.ts);
    shown
}

/// Price history as a stepped line: a price holds until the next change.
/// `since` leaves out everything before that time.
pub fn chart(points: &[PricePoint], target_cents: Option<i64>, now: i64, since: Option<i64>) -> Markup {
    let ranged;
    let points = match since {
        Some(since) => {
            ranged = in_range(points, since);
            &ranged[..]
        }
        None => points,
    };
    let public: Vec<&PricePoint> = points.iter().filter(|p| !p.member).collect();
    let member: Vec<&PricePoint> = points.iter().filter(|p| p.member).collect();
    let Some(first) = points.iter().map(|p| p.ts).min() else {
        return html! { p.muted { "No prices recorded yet." } };
    };

    const W: f64 = 720.0;
    const H: f64 = 240.0;
    const LEFT: f64 = 12.0;
    const RIGHT: f64 = 12.0;
    const TOP: f64 = 22.0;
    const BOTTOM: f64 = 30.0;

    let prices = points.iter().map(|p| p.price_cents);
    let price_low = prices.clone().min().unwrap_or(0);
    let price_high = prices.max().unwrap_or(0);
    let (mut low, mut high) = (price_low, price_high);
    // Show the target line only when it is near the prices; a far-off target
    // would squash the real movement flat.
    let target = target_cents.filter(|t| *t as f64 >= low as f64 * 0.6 && *t as f64 <= high as f64 * 1.4);
    if let Some(t) = target {
        low = low.min(t);
        high = high.max(t);
    }
    let pad = (((high - low) as f64) * 0.12).max(high as f64 * 0.03).max(50.0);
    let (y_min, y_max) = ((low as f64 - pad).max(0.0), high as f64 + pad);

    // At least a day wide, so a brand-new product is not a single dot.
    let span = ((now - first) as f64).max(86_400.0);
    let x = |ts: i64| LEFT + ((ts - first) as f64 / span).clamp(0.0, 1.0) * (W - LEFT - RIGHT);
    let y = |cents: i64| TOP + (1.0 - (cents as f64 - y_min) / (y_max - y_min)) * (H - TOP - BOTTOM);

    let mut line = String::new();
    for (i, p) in public.iter().enumerate() {
        if i == 0 {
            line.push_str(&format!("M{:.1},{:.1}", x(p.ts), y(p.price_cents)));
        } else {
            line.push_str(&format!(" H{:.1} V{:.1}", x(p.ts), y(p.price_cents)));
        }
    }
    let area = if public.is_empty() {
        String::new()
    } else {
        line.push_str(&format!(" H{:.1}", x(now)));
        format!("{line} V{:.1} H{:.1} Z", H - BOTTOM, x(public[0].ts))
    };

    let label_fmt = format_description!("[month repr:short] [day padding:none], [year]");
    let first_label =
        time::OffsetDateTime::from_unix_timestamp(first).ok().and_then(|d| d.format(label_fmt).ok()).unwrap_or_default();
    let summary =
        format!("Price history from {} to now, ranging from {} to {}.", first_label, dollars(price_low), dollars(price_high),);

    html! {
        svg.chart viewBox={ "0 0 " (W) " " (H) } role="img" aria-label=(summary) {
            line.grid x1=(LEFT) x2=(W - RIGHT) y1=(y(price_high)) y2=(y(price_high)) {}
            line.grid x1=(LEFT) x2=(W - RIGHT) y1=(y(price_low)) y2=(y(price_low)) {}
            line.grid x1=(LEFT) x2=(W - RIGHT) y1=(H - BOTTOM) y2=(H - BOTTOM) {}
            text x=(LEFT + 2.0) y=(y(price_high) - 6.0) { (dollars(price_high)) }
            @if price_high != price_low {
                text x=(LEFT + 2.0) y=(y(price_low) + 15.0) { (dollars(price_low)) }
            }
            text x=(LEFT) y=(H - 9.0) { (first_label) }
            text x=(W - RIGHT) y=(H - 9.0) text-anchor="end" { "Now" }
            @if let Some(t) = target {
                line.target x1=(LEFT) x2=(W - RIGHT) y1=(y(t)) y2=(y(t)) {}
                text.target-label x=(W - RIGHT - 2.0) y=(y(t) - 6.0) text-anchor="end" { "Target " (dollars(t)) }
            }
            @if !area.is_empty() {
                path.area d=(area) {}
                path.line d=(line) {}
            }
            @for p in &public {
                circle.dot cx=(format!("{:.1}", x(p.ts))) cy=(format!("{:.1}", y(p.price_cents))) r="3.5" {
                    title { (dollars(p.price_cents)) }
                }
            }
            @for p in &member {
                circle.dot.member cx=(format!("{:.1}", x(p.ts))) cy=(format!("{:.1}", y(p.price_cents))) r="4.5" {
                    title { "Signed-in price " (dollars(p.price_cents)) }
                }
            }
        }
        div.legend {
            span { i {} "Listed price" }
            @if !member.is_empty() { span { i.member {} "Signed-in price" } }
            @if target.is_some() { span { i.target {} "Your target" } }
        }
    }
}

/// Raw SVG for the favicon route.
pub fn icon() -> PreEscaped<&'static str> {
    PreEscaped(ICON)
}
