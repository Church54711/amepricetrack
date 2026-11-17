//! Decides when a price change is worth telling someone about, and delivers
//! the alert: it is saved for the web page and the phone app, and pushed to
//! ntfy or a webhook when the person has set one up.

use crate::db::{self, Product, Watch};
use crate::pricing::{self, dollars, PriceView};
use crate::retailers;
use anyhow::Result;

#[derive(Debug, PartialEq)]
pub struct Alert {
    pub kind: &'static str,
    pub headline: String,
}

/// Compares a watch against the price its owner would pay now.
///
/// Returns the alert to send (if any). The caller stores `view.your_cents`
/// as the new "last seen" price afterwards.
pub fn decide(watch: &Watch, product: &Product, view: &PriceView) -> Option<Alert> {
    let came_back = watch.notify_back_in_stock && watch.last_seen_in_stock == Some(false) && product.in_stock == Some(true);

    let Some(now_price) = view.your_cents else {
        return came_back.then(|| Alert { kind: "stock", headline: "Back in stock".into() });
    };
    let before = watch.last_seen_price_cents;

    // 1. Target price reached. Announced once per trip below the target, and
    //    again only if it then goes lower still.
    if let Some(target) = watch.target_price_cents {
        if now_price <= target && watch.last_alert_price_cents.is_none_or(|told| now_price < told) {
            return Some(Alert {
                kind: "target",
                headline: format!("Hit your target: now {} (target {})", dollars(now_price), dollars(target)),
            });
        }
    }

    if let Some(before) = before.filter(|b| now_price < *b) {
        let percent = (before - now_price) as f64 / before as f64 * 100.0;
        // 2. A drop of at least the chosen size.
        if let Some(wanted) = watch.drop_percent.filter(|w| *w > 0.0) {
            if percent >= wanted {
                return Some(Alert {
                    kind: "drop",
                    headline: format!("Down {:.0}%: {} \u{2192} {}", percent, dollars(before), dollars(now_price)),
                });
            }
        } else if watch.notify_any_drop {
            // 3. Any drop at all (only when no minimum size was chosen).
            return Some(Alert {
                kind: "drop",
                headline: format!("Price drop: {} \u{2192} {}", dollars(before), dollars(now_price)),
            });
        }
    }

    came_back.then(|| Alert { kind: "stock", headline: format!("Back in stock at {}", dollars(now_price)) })
}

/// Re-checks every watch on a product after its price or stock changed.
pub async fn evaluate(app: &crate::App, product_id: i64) -> Result<()> {
    let Some(product) = db::product(&app.db, product_id).await? else {
        return Ok(());
    };
    for watch in db::watches_for_product(&app.db, product_id).await? {
        let rules = db::rules(&app.db, watch.user_id).await?;
        let member = db::member_prices(&app.db, watch.user_id).await?.get(&product_id).copied();
        let view = pricing::view(&product, member, &rules);
        let alert = decide(&watch, &product, &view);

        // Remember what this person has now been shown. When the price climbs
        // back above the target, forget the last target alert so the next dip
        // is announced again.
        let above_target = match (watch.target_price_cents, view.your_cents) {
            (Some(target), Some(price)) => price > target,
            _ => false,
        };
        let alert_price = match &alert {
            Some(a) if a.kind == "target" => view.your_cents,
            _ if above_target => None,
            _ => watch.last_alert_price_cents,
        };
        sqlx::query(
            "UPDATE watches SET last_seen_price_cents = COALESCE(?, last_seen_price_cents),
                last_seen_in_stock = COALESCE(?, last_seen_in_stock), last_alert_price_cents = ?,
                last_alert_at = CASE WHEN ? THEN ? ELSE last_alert_at END
             WHERE id = ?",
        )
        .bind(view.your_cents)
        .bind(product.in_stock)
        .bind(alert_price)
        .bind(alert.is_some())
        .bind(db::now())
        .bind(watch.id)
        .execute(&app.db)
        .await?;

        if let Some(alert) = alert {
            send(app, &watch, &product, &view, &alert).await?;
        }
    }
    Ok(())
}

async fn send(app: &crate::App, watch: &Watch, product: &Product, view: &PriceView, alert: &Alert) -> Result<()> {
    let store = retailers::display_name(&product.retailer);
    let title = format!("{store}: {}", alert.headline);
    let mut body = product.display_title();
    if view.is_personal() {
        if let (Some(public), Some(name)) = (view.public_cents, &view.discount_name) {
            body.push_str(&format!("\nYour price with {name}. Listed at {}.", dollars(public)));
        } else if let Some(public) = view.public_cents {
            body.push_str(&format!("\nYour signed-in price. Listed at {}.", dollars(public)));
        }
    }
    if let (Some(now_price), Some(low)) = (product.current_price_cents, product.lowest_price_cents) {
        if now_price <= low {
            body.push_str("\nLowest price recorded so far.");
        }
    }
    notify(app, watch.user_id, Some(product.id), alert.kind, &title, &body, view.your_cents, &product.url).await
}

/// Saves an alert for a person and pushes it to ntfy or their webhook.
/// `store_link` is used in the push when this server's own address is not set.
#[allow(clippy::too_many_arguments)]
pub async fn notify(
    app: &crate::App,
    user_id: i64,
    product_id: Option<i64>,
    kind: &str,
    title: &str,
    body: &str,
    price_cents: Option<i64>,
    store_link: &str,
) -> Result<()> {
    db::add_notification(&app.db, user_id, product_id, kind, title, body, price_cents).await?;
    tracing::info!(user = user_id, product = ?product_id, "alert: {title}");

    let settings = db::settings(&app.db, user_id).await?;
    let link = match (&app.cfg.public_url, product_id) {
        (Some(base), Some(id)) => format!("{base}/products/{id}"),
        _ => store_link.to_string(),
    };
    // For someone who uses the phone app, the pushed alert gets a button that opens the product there.
    let app_link = match product_id {
        Some(id) if !settings.ntfy_url.is_empty() && uses_phone_app(&app.db, user_id).await? => Some(app_link(id)),
        _ => None,
    };
    let (http, title, body) = (app.http.clone(), title.to_string(), body.to_string());
    // Pushing is best effort and must not hold up the price checker.
    tokio::spawn(async move {
        if let Err(e) = push(&http, &settings.ntfy_url, &settings.webhook_url, &title, &body, &link, app_link.as_deref()).await {
            tracing::warn!("could not push alert: {e:#}");
        }
    });
    Ok(())
}

/// The address that opens a product in the phone app.
pub fn app_link(product_id: i64) -> String {
    format!("amepricetrack://products/{product_id}")
}

/// True when this person has a phone (or anything else) signed in through the API.
async fn uses_phone_app(db: &db::Db, user_id: i64) -> Result<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sessions WHERE user_id = ? AND kind = 'api')")
        .bind(user_id)
        .fetch_one(db)
        .await?)
}

/// Sends one alert to ntfy and/or a webhook. Empty addresses are skipped.
/// `app_link` adds a button to the ntfy notification that opens the phone app.
pub async fn push(
    http: &reqwest::Client,
    ntfy_url: &str,
    webhook_url: &str,
    title: &str,
    body: &str,
    link: &str,
    app_link: Option<&str>,
) -> Result<()> {
    let mut problems = Vec::new();
    if !ntfy_url.is_empty() {
        // ntfy headers must be plain ASCII; the arrow in price drops is not.
        let ascii_title: String = title.replace('\u{2192}', "->").chars().filter(char::is_ascii).collect();
        let mut request = http.post(ntfy_url).header("Title", ascii_title).header("Tags", "moneybag");
        // An alert about the server itself may have nowhere to link to.
        if !link.is_empty() {
            request = request.header("Click", link);
        }
        if let Some(app_link) = app_link {
            request = request.header("Actions", format!("view, Open in app, {app_link}, clear=true"));
        }
        let sent = request.body(body.to_string()).send().await.and_then(|r| r.error_for_status());
        if let Err(e) = sent {
            problems.push(format!("ntfy: {}", crate::fetch::root_cause(&e)));
        }
    }
    if !webhook_url.is_empty() {
        // Discord wants its own shape; everything else gets a plain description.
        let payload = if webhook_url.contains("discord.com/api/webhooks") || webhook_url.contains("discordapp.com/api/webhooks") {
            serde_json::json!({ "content": format!("**{title}**\n{body}\n{link}") })
        } else {
            serde_json::json!({ "title": title, "body": body, "url": link, "text": format!("{title}\n{body}\n{link}") })
        };
        let sent = http.post(webhook_url).json(&payload).send().await.and_then(|r| r.error_for_status());
        if let Err(e) = sent {
            problems.push(format!("webhook: {}", crate::fetch::root_cause(&e)));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(problems.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product(price: Option<i64>, in_stock: Option<bool>) -> Product {
        Product {
            id: 1,
            retailer: "staples".into(),
            retailer_sku: "1".into(),
            url: "https://www.staples.com/x/product_1".into(),
            title: Some("Chair".into()),
            brand: None,
            model: None,
            gtin: None,
            image_url: None,
            current_price_cents: price,
            list_price_cents: None,
            in_stock,
            promo_text: None,
            lowest_price_cents: price,
            lowest_price_at: None,
            fetch_mode: None,
            created_at: 0,
            last_checked_at: None,
            last_success_at: None,
            next_check_at: 0,
            fail_count: 0,
            last_error: None,
            details: None,
            seller: None,
            third_party: None,
            store_ships: None,
            ships_free: None,
            shipping_cents: None,
        }
    }

    fn watch() -> Watch {
        Watch {
            id: 1,
            user_id: 1,
            product_id: 1,
            target_price_cents: None,
            drop_percent: None,
            notify_any_drop: true,
            notify_back_in_stock: false,
            note: None,
            created_at: 0,
            last_seen_price_cents: None,
            last_seen_in_stock: None,
            last_alert_price_cents: None,
            last_alert_at: None,
            check_minutes: None,
        }
    }

    fn check(w: &Watch, price: i64) -> Option<Alert> {
        let p = product(Some(price), Some(true));
        decide(w, &p, &pricing::view(&p, None, &[]))
    }

    #[test]
    fn first_reading_is_silent_unless_already_at_target() {
        assert_eq!(check(&watch(), 10000), None);
        let w = Watch { target_price_cents: Some(12000), ..watch() };
        assert_eq!(check(&w, 10000).unwrap().kind, "target");
    }

    #[test]
    fn any_drop_alerts_but_rises_and_no_change_do_not() {
        let w = Watch { last_seen_price_cents: Some(10000), ..watch() };
        assert_eq!(check(&w, 9000).unwrap().headline, "Price drop: $100.00 \u{2192} $90.00");
        assert_eq!(check(&w, 10000), None);
        assert_eq!(check(&w, 11000), None);
        let quiet = Watch { notify_any_drop: false, ..w };
        assert_eq!(check(&quiet, 9000), None);
    }

    #[test]
    fn minimum_drop_size_filters_small_drops() {
        let w = Watch { last_seen_price_cents: Some(10000), drop_percent: Some(15.0), ..watch() };
        assert_eq!(check(&w, 9000), None, "10% is under the 15% asked for");
        assert_eq!(check(&w, 8000).unwrap().headline, "Down 20%: $100.00 \u{2192} $80.00");
    }

    #[test]
    fn target_is_announced_once_then_only_when_lower() {
        let w = Watch { target_price_cents: Some(9000), last_seen_price_cents: Some(10000), notify_any_drop: false, ..watch() };
        assert_eq!(check(&w, 8500).unwrap().kind, "target");
        let told = Watch { last_alert_price_cents: Some(8500), last_seen_price_cents: Some(8500), ..w.clone() };
        assert_eq!(check(&told, 8500), None);
        assert_eq!(check(&told, 8000).unwrap().kind, "target");
        assert_eq!(check(&w, 9500), None, "still above the target");
    }

    #[test]
    fn target_uses_the_personal_price() {
        // Listed at $100, target $92: not reached publicly, reached with 10% off.
        let w = Watch { target_price_cents: Some(9200), ..watch() };
        let p = product(Some(10000), Some(true));
        let rule = crate::db::DiscountRule {
            id: 1,
            user_id: 1,
            retailer: "staples".into(),
            name: "Associate 10%".into(),
            percent: 10.0,
            brands: String::new(),
            exclude_keywords: String::new(),
            enabled: true,
            created_at: 0,
            free_shipping: false,
        };
        assert_eq!(decide(&w, &p, &pricing::view(&p, None, &[])), None);
        let alert = decide(&w, &p, &pricing::view(&p, None, &[rule])).unwrap();
        assert_eq!(alert.headline, "Hit your target: now $90.00 (target $92.00)");
    }

    /// Accepts one request on this machine, answers "OK", and hands back what was sent.
    async fn one_request() -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/made-up-topic", listener.local_addr().unwrap());
        let received = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut seen = Vec::new();
            let mut buffer = [0u8; 4096];
            // The headers and the short body arrive within the first reads.
            while !String::from_utf8_lossy(&seen).contains("Lowest price recorded") {
                let n = socket.read(&mut buffer).await.unwrap();
                if n == 0 {
                    break;
                }
                seen.extend_from_slice(&buffer[..n]);
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok").await.unwrap();
            String::from_utf8_lossy(&seen).to_ascii_lowercase()
        });
        (address, received)
    }

    #[tokio::test]
    async fn a_pushed_alert_can_open_the_phone_app() {
        let http = reqwest::Client::new();
        let (topic, received) = one_request().await;
        let body = "Chair\nLowest price recorded so far.";
        push(
            &http,
            &topic,
            "",
            "Staples: Price drop: $100.00 \u{2192} $90.00",
            body,
            "http://server.example/products/12",
            Some(&app_link(12)),
        )
        .await
        .unwrap();
        let sent = received.await.unwrap();
        assert!(sent.contains("title: staples: price drop: $100.00 -> $90.00"), "{sent}");
        assert!(sent.contains("click: http://server.example/products/12"), "{sent}");
        assert!(sent.contains("actions: view, open in app, amepricetrack://products/12, clear=true"), "{sent}");

        // Without the app in use there is no such button.
        let (topic, received) = one_request().await;
        push(&http, &topic, "", "Staples: Back in stock", body, "", None).await.unwrap();
        let sent = received.await.unwrap();
        assert!(!sent.contains("actions:") && !sent.contains("click:"), "{sent}");
    }

    #[test]
    fn back_in_stock() {
        let w =
            Watch { notify_back_in_stock: true, last_seen_in_stock: Some(false), last_seen_price_cents: Some(5000), ..watch() };
        let p = product(Some(5000), Some(true));
        assert_eq!(decide(&w, &p, &pricing::view(&p, None, &[])).unwrap().kind, "stock");
        let off = Watch { notify_back_in_stock: false, ..w };
        assert_eq!(decide(&off, &p, &pricing::view(&p, None, &[])), None);
    }
}
