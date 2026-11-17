//! Actions shared by the website and the phone API.

use crate::db::{self, Audience, Product, Watch};
use crate::pricing::{self, dollars, DealInfo, PriceView};
use crate::retailers::{self, ParseError};
use crate::{ai, alerts, App};
use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

/// A failure, split by who should hear the details.
#[derive(Debug, thiserror::Error)]
pub enum Problem {
    /// Something the person can act on; the text is shown to them.
    #[error("{0}")]
    User(String),
    /// A fault in the server; logged, and the person sees a generic message.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for Problem {
    fn from(e: sqlx::Error) -> Self {
        Problem::Internal(e.into())
    }
}

pub fn user_problem(message: impl Into<String>) -> Problem {
    Problem::User(message.into())
}

/// One watched product with everything needed to show it.
#[derive(Debug, Clone, Serialize)]
pub struct WatchView {
    pub product: Product,
    pub watch: Watch,
    pub price: PriceView,
    pub deal: DealInfo,
    /// The cheapest confirmed match at another store, if any was found.
    pub elsewhere: Option<crate::compare::tracking::Elsewhere>,
}

pub async fn watch_views(app: &App, user_id: i64) -> Result<Vec<WatchView>, Problem> {
    let rules = db::rules(&app.db, user_id).await?;
    let member = db::member_prices(&app.db, user_id).await?;
    let changes = pricing::group_changes(db::watched_price_changes(&app.db, user_id).await?);
    let mut elsewhere = crate::compare::tracking::best_elsewhere(&app.db, user_id).await?;
    let now = db::now();
    Ok(db::watched(&app.db, user_id)
        .await?
        .into_iter()
        .map(|(product, watch)| {
            let price = pricing::view(&product, member.get(&product.id).copied(), &rules);
            let history = changes.get(&product.id).map(Vec::as_slice).unwrap_or(&[]);
            let deal = pricing::deal_info(&product, &price, history, now);
            let elsewhere = elsewhere.remove(&product.id);
            WatchView { product, watch, price, deal, elsewhere }
        })
        .collect())
}

pub async fn watch_view(app: &App, user_id: i64, product_id: i64) -> Result<Option<WatchView>, Problem> {
    Ok(watch_views(app, user_id).await?.into_iter().find(|v| v.product.id == product_id))
}

static RE_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s<>"']+"#).unwrap());

/// Starts tracking the product behind a link. Accepts the text store apps
/// share ("Check this out! https://...") as well as a bare link.
pub async fn add_by_url(app: &App, user_id: i64, text: &str, target_cents: Option<i64>) -> Result<i64, Problem> {
    let text = text.trim();
    if text.is_empty() {
        return Err(user_problem("Paste a product link first."));
    }
    let mut link =
        RE_LINK.find(text).map(|m| m.as_str().trim_end_matches(['.', ',', ')']).to_string()).unwrap_or_else(|| text.to_string());
    if link.contains(char::is_whitespace) {
        return Err(user_problem("That does not look like a web link."));
    }

    if let Ok(parsed) = url::Url::parse(&link) {
        if retailers::is_short_link(&parsed) {
            link = app.fetcher.resolve_short_link(&link).await.map_err(|e| user_problem(e.to_string()))?;
        }
    }
    let found = retailers::identify(&link).map_err(|e| user_problem(e.to_string()))?;
    let product_id = db::upsert_product(&app.db, &found).await?;
    db::add_watch(&app.db, user_id, product_id, target_cents).await?;
    // Someone else may already track this product; the new watcher's target
    // could be met right now.
    alerts::evaluate(app, product_id).await?;
    if db::settings(&app.db, user_id).await?.auto_compare {
        crate::compare::tracking::enqueue(app, user_id, product_id, false).await?;
    }
    app.wake.notify_one();
    Ok(product_id)
}

#[derive(Debug, Serialize)]
pub struct IngestResult {
    pub product_id: i64,
    pub price_cents: Option<i64>,
    /// "public" or "member".
    pub recorded_as: &'static str,
}

/// Takes a product page that a signed-in device loaded and records the price on it.
///
/// A phone signed in at a store may see a member, employee or app price. When
/// the page's price differs from a fresh public price, it is kept as that
/// person's own price; otherwise it simply refreshes the public price.
pub async fn ingest(app: &App, user_id: i64, url: &str, html: &str, signed_in: bool) -> Result<IngestResult, Problem> {
    let found = retailers::identify(url).map_err(|e| user_problem(e.to_string()))?;
    let product_id: Option<i64> = sqlx::query_scalar("SELECT id FROM products WHERE retailer = ? AND retailer_sku = ?")
        .bind(&found.retailer)
        .bind(&found.sku)
        .fetch_optional(&app.db)
        .await?;
    let product = match product_id {
        Some(id) => db::product(&app.db, id).await?,
        None => None,
    };
    let Some(product) = product else {
        return Err(user_problem("That product is not being tracked yet. Add it first."));
    };
    if db::watch(&app.db, user_id, product.id).await?.is_none() {
        return Err(user_problem("That product is not on your list. Add it first."));
    }

    let obs = retailers::parse(&found.retailer, html).map_err(|e| match e {
        ParseError::Blocked(why) => user_problem(format!("The store showed a refusal page instead of the product ({why}).")),
        ParseError::NoPrice => user_problem("No price was found on that page."),
    })?;

    let now = db::now();
    let public_is_fresh = product.last_success_at.is_some_and(|t| now - t < 36 * 3600);
    let differs =
        obs.price_cents.is_some() && product.current_price_cents.is_some() && obs.price_cents != product.current_price_cents;
    let audience = if signed_in && public_is_fresh && differs { Audience::Member(user_id) } else { Audience::Public };

    // A public reading from the phone counts as a check, so the server waits a full interval.
    let interval = db::check_interval(&app.db, product.id, app.cfg.check_interval.as_secs() as i64).await?;
    let next = (audience == Audience::Public).then_some(interval);
    let changed = db::record_observation(&app.db, product.id, &obs, "device", audience, None, next).await?;
    if changed {
        alerts::evaluate(app, product.id).await?;
        crate::compare::tracking::price_changed(app, product.id).await?;
    }
    Ok(IngestResult {
        product_id: product.id,
        price_cents: obs.price_cents,
        recorded_as: if audience == Audience::Public { "public" } else { "member" },
    })
}

/// After a discount is added, changed or removed, the "last seen" price of
/// affected watches is reset so the change itself is not reported as a price drop.
pub async fn rebaseline(app: &App, user_id: i64, retailer: &str) -> Result<(), Problem> {
    sqlx::query(
        "UPDATE watches SET last_seen_price_cents = NULL, last_alert_price_cents = NULL
         WHERE user_id = ? AND product_id IN (SELECT id FROM products WHERE retailer = ?)",
    )
    .bind(user_id)
    .bind(retailer)
    .execute(&app.db)
    .await?;
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT product_id FROM watches WHERE user_id = ? AND product_id IN (SELECT id FROM products WHERE retailer = ?)",
    )
    .bind(user_id)
    .bind(retailer)
    .fetch_all(&app.db)
    .await?;
    for id in ids {
        alerts::evaluate(app, id).await?;
    }
    Ok(())
}

// ------------------------------------------------------------ pushed alerts

/// Checks a web address a person typed for pushing alerts to. Empty stays empty.
pub fn web_address(text: &str, what: &str) -> Result<String, Problem> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(String::new());
    }
    match url::Url::parse(text) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && u.host_str().is_some() => Ok(text.to_string()),
        _ => Err(user_problem(format!("The {what} address must start with http:// or https://"))),
    }
}

/// Sets (or, with an empty address, clears) the ntfy topic a person's alerts are pushed to.
pub async fn set_ntfy(app: &App, user_id: i64, address: &str) -> Result<String, Problem> {
    let ntfy = web_address(address, "ntfy")?;
    if !ntfy.is_empty() {
        // A topic address is the server plus one name: https://ntfy.sh/my-topic
        let topic = url::Url::parse(&ntfy).ok().map(|u| u.path().trim_matches('/').to_string()).unwrap_or_default();
        if topic.is_empty() || topic.contains('/') {
            return Err(user_problem(
                "The ntfy address needs a topic name after the server, such as https://ntfy.sh/my-topic-name",
            ));
        }
    }
    sqlx::query("UPDATE user_settings SET ntfy_url = ? WHERE user_id = ?").bind(&ntfy).bind(user_id).execute(&app.db).await?;
    Ok(ntfy)
}

/// Pushes a test alert to wherever this person's alerts go.
pub async fn send_test_push(app: &App, user_id: i64) -> Result<(), Problem> {
    let settings = db::settings(&app.db, user_id).await?;
    if settings.ntfy_url.is_empty() && settings.webhook_url.is_empty() {
        return Err(user_problem("Add an ntfy or webhook address first, then send a test."));
    }
    let link = app.cfg.public_url.clone().unwrap_or_default();
    alerts::push(
        &app.http,
        &settings.ntfy_url,
        &settings.webhook_url,
        "AmePriceTrack test alert",
        "If you can read this, alerts will reach you here.",
        &link,
        None,
    )
    .await
    .map_err(|e| user_problem(format!("The test could not be delivered: {e}")))
}

// --------------------------------------------------------------- assistant

fn ago(now: i64, then: Option<i64>) -> String {
    match then {
        None => "never".into(),
        Some(t) => {
            let secs = (now - t).max(0);
            match secs {
                s if s < 3600 => format!("{}m ago", (s / 60).max(1)),
                s if s < 2 * db::DAY => format!("{}h ago", s / 3600),
                s => format!("{}d ago", s / db::DAY),
            }
        }
    }
}

/// The instructions and data the assistant is given: the person's whole
/// watch list as a table, their discounts, and recent alerts.
pub async fn assistant_context(app: &App, user_id: i64) -> Result<String, Problem> {
    let now = db::now();
    let views = watch_views(app, user_id).await?;
    let rules = db::rules(&app.db, user_id).await?;
    let alerts = db::notifications(&app.db, user_id, 0, 10).await?;
    let today = time::OffsetDateTime::now_utc().date();

    let mut out = String::new();
    out.push_str(
        "You are the assistant built into AmePriceTrack, a self-hosted price tracker. \
         Answer the person's questions about the products they track, using only the data below.\n\
         Rules:\n\
         - Prices are in US dollars. \"Your price\" already includes the person's own discounts \
         (for example an employee discount) and any signed-in price their phone reported.\n\
         - Never invent a price, a sale or a product. If something is not in the data, say it is not tracked \
         and that they can add it by pasting its link on the Watchlist page.\n\
         - You can compare stores, point out the best deals, say whether now looks like a good time to buy \
         based on the history shown, and explain how a price was worked out. You cannot predict future prices; \
         say so if asked, then give what the history suggests.\n\
         - You cannot change anything (add products, set targets); explain where to do it instead.\n\
         - Be brief and specific. Lead with the answer. Use plain text or simple Markdown lists.\n\n",
    );
    out.push_str(&format!("Today is {today} (UTC).\n\n"));

    if views.is_empty() {
        out.push_str("The person is not tracking any products yet.\n");
    } else {
        out.push_str(&format!("TRACKED PRODUCTS ({}):\n", views.len()));
        for v in views.iter().take(200) {
            let p = &v.product;
            let money = |c: Option<i64>| c.map(dollars).unwrap_or_else(|| "unknown".into());
            let mut line = format!(
                "- [{}] {} | store: {} | listed now: {}",
                p.id,
                p.display_title(),
                retailers::display_name(&p.retailer),
                money(p.current_price_cents)
            );
            if v.price.is_personal() {
                line.push_str(&format!(" | your price: {}", money(v.price.your_cents)));
                if let (Some(name), Some(pct)) = (&v.price.discount_name, v.price.discount_percent) {
                    line.push_str(&format!(" (with {name}, {pct}% off)"));
                }
                if let Some(m) = v.price.member_cents {
                    line.push_str(&format!(" | signed-in price: {}", dollars(m)));
                }
            }
            if let Some(list) = p.list_price_cents {
                line.push_str(&format!(" | regular/list price: {}", dollars(list)));
            }
            if let Some(avg) = v.deal.usual_cents {
                line.push_str(&format!(" | 90-day average: {}", dollars(avg)));
            }
            if let Some(low) = p.lowest_price_cents {
                line.push_str(&format!(" | lowest recorded: {} ({})", dollars(low), ago(now, p.lowest_price_at)));
            }
            if let Some(pct) = v.deal.percent_below.filter(|p| p.abs() >= 0.5) {
                let versus = v.deal.compared_to.unwrap_or("usual");
                line.push_str(&format!(" | {:.0}% {} {versus} price", pct.abs(), if pct >= 0.0 { "below" } else { "above" }));
            }
            if let Some(target) = v.watch.target_price_cents {
                line.push_str(&format!(" | target: {}", dollars(target)));
            }
            match p.in_stock {
                Some(true) => line.push_str(" | in stock"),
                Some(false) => line.push_str(" | OUT OF STOCK"),
                None => {}
            }
            if let Some(brand) = &p.brand {
                line.push_str(&format!(" | brand: {brand}"));
            }
            if let Some(shipping) = v.price.shipping_cents.filter(|s| *s > 0) {
                line.push_str(&format!(" | delivery adds {}", dollars(shipping)));
            } else if v.price.shipping_extra {
                line.push_str(" | delivery is charged on top");
            }
            if p.third_party == Some(true) {
                line.push_str(&format!(" | sold by marketplace seller {}", p.seller.as_deref().unwrap_or("(unnamed)")));
            }
            if let Some(other) = &v.elsewhere {
                line.push_str(&format!(
                    " | cheapest other store with the same product (delivery included, same amount): {} at {}",
                    dollars(other.cents),
                    other.store_name
                ));
            }
            line.push_str(&format!(" | tracked {} | checked {}", ago(now, Some(p.created_at)), ago(now, p.last_success_at)));
            if p.current_price_cents.is_none() {
                match &p.last_error {
                    Some(e) => line.push_str(&format!(" | could not read price: {e}")),
                    None => line.push_str(" | first check still pending"),
                }
            }
            out.push_str(&line);
            out.push('\n');
        }
        if views.len() > 200 {
            out.push_str(&format!("(and {} more not shown)\n", views.len() - 200));
        }
    }

    if !rules.is_empty() {
        out.push_str("\nPERSONAL DISCOUNTS:\n");
        for r in &rules {
            out.push_str(&format!(
                "- {} at {}: {}% off{}{}{}\n",
                r.name,
                retailers::display_name(&r.retailer),
                r.percent,
                if r.brands.trim().is_empty() { " everything".to_string() } else { format!(" brands: {}", r.brands) },
                if r.exclude_keywords.trim().is_empty() { String::new() } else { format!(", except: {}", r.exclude_keywords) },
                if r.enabled { "" } else { " (turned off)" },
            ));
        }
    }

    if !alerts.is_empty() {
        out.push_str("\nRECENT ALERTS:\n");
        for n in &alerts {
            out.push_str(&format!("- {}: {} ({})\n", ago(now, Some(n.ts)), n.title, n.body.lines().next().unwrap_or("")));
        }
    }
    Ok(out)
}

/// Asks the connected AI a question about the person's tracked products.
pub async fn ask(app: &App, user_id: i64, message: &str) -> Result<String, Problem> {
    let message = message.trim();
    if message.is_empty() {
        return Err(user_problem("Type a question first."));
    }
    if message.chars().count() > 4000 {
        return Err(user_problem("That question is too long. Keep it under 4,000 characters."));
    }
    let settings = db::settings(&app.db, user_id).await?;
    let cfg = ai::AiConfig::from_settings(&settings, true).map_err(|e| user_problem(e.to_string()))?;
    let system = assistant_context(app, user_id).await?;

    // The last ten exchanges give the model the thread of the conversation.
    let mut turns: Vec<ai::Turn> = db::chat_history(&app.db, user_id, 20)
        .await?
        .into_iter()
        .map(|m| (m.role, m.content))
        .skip_while(|(role, _)| role != "user")
        .collect();
    turns.push(("user".into(), message.to_string()));

    let reply = ai::complete(&app.http, &cfg, &system, &turns).await.map_err(|e| user_problem(e.to_string()))?;
    db::add_chat_message(&app.db, user_id, "user", message).await?;
    db::add_chat_message(&app.db, user_id, "assistant", &reply).await?;
    Ok(reply)
}

#[cfg(test)]
#[path = "tests_service.rs"]
mod tests;
