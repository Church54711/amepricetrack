//! Remembering what was found at other stores, and the background worker
//! that does the searching.

use super::advisor::Advisor;
use super::matching::{amount_text, Facts, PackDiff, POSSIBLE, SAME};
use super::{facts_of, find_at_store, stores, StoreResult};
use crate::db::{self, Audience, Db, Product};
use crate::pricing::{self, dollars};
use crate::retailers::{self, ProductRef};
use crate::{ai, alerts, App};
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Comparison {
    pub id: i64,
    pub user_id: i64,
    pub source_product_id: i64,
    pub retailer: String,
    /// pending, found, possible, none, failed or rejected.
    pub status: String,
    pub product_id: Option<i64>,
    pub score: Option<i64>,
    pub reason: Option<String>,
    pub confirmed: bool,
    pub last_notified_cents: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Who decided: "rules" (barcode, model number or name) or "ai".
    pub judged_by: String,
    /// How many the tracked product holds, when the other listing holds a different amount.
    pub source_qty: Option<f64>,
    /// How many the other store's listing holds.
    pub other_qty: Option<f64>,
    /// What is counted ("pack", "sheets").
    pub qty_unit: Option<String>,
}

impl Comparison {
    /// The amounts, when the other store sells the same item in a different one.
    pub fn pack(&self) -> Option<PackDiff> {
        match (self.source_qty, self.other_qty, &self.qty_unit) {
            (Some(source_qty), Some(other_qty), Some(unit)) if source_qty > 0.0 && other_qty > 0.0 => {
                Some(PackDiff { source_qty, other_qty, unit: unit.clone() })
            }
            _ => None,
        }
    }

    /// A match the AI made that the person has not confirmed themselves.
    pub fn ai_judged(&self) -> bool {
        self.judged_by == "ai" && !self.confirmed
    }
}

/// One other store's answer for a tracked product, ready to show.
#[derive(Debug, Clone, Serialize)]
pub struct StoreRow {
    pub comparison: Comparison,
    pub store_name: String,
    /// The product found there, for 'found' and 'possible'.
    pub product: Option<Product>,
    /// What this person would pay there for the listing, after their discounts at that store.
    pub your_cents: Option<i64>,
    /// What delivery adds there; 0 when free. None when the store does not say.
    pub shipping_cents: Option<i64>,
    /// Delivery is charged there, but the page does not say how much.
    pub shipping_extra: bool,
    /// The figure to compare with the tracked product: the listing plus
    /// delivery, worked out for the same amount as the tracked product.
    pub equivalent_cents: Option<i64>,
    /// "500 sheets": how much the other listing holds, when that differs.
    pub their_amount: Option<String>,
    /// "5000 sheets": how much the tracked product holds, when that differs.
    pub your_amount: Option<String>,
    /// The offer comes from a marketplace seller, not the store itself.
    pub marketplace: bool,
}

/// One other store's offer as this person would pay it.
fn store_row(comparison: Comparison, store_name: String, product: Option<Product>, rules: &[db::DiscountRule]) -> StoreRow {
    let view = product.as_ref().map(|p| pricing::view(p, None, rules)).unwrap_or_default();
    let pack = comparison.pack();
    let equivalent_cents = view.total_cents().map(|total| pack.as_ref().map_or(total, |p| p.equivalent(total)));
    StoreRow {
        store_name,
        your_cents: view.your_cents,
        shipping_cents: view.shipping_cents,
        shipping_extra: view.shipping_extra,
        equivalent_cents,
        their_amount: pack.as_ref().map(|p| amount_text(p.other_qty, &p.unit)),
        your_amount: pack.as_ref().map(|p| amount_text(p.source_qty, &p.unit)),
        marketplace: product.as_ref().is_some_and(|p| p.third_party == Some(true)),
        product,
        comparison,
    }
}

/// The cheapest confirmed match at another store.
#[derive(Debug, Clone, Serialize)]
pub struct Elsewhere {
    pub store_name: String,
    pub product_id: i64,
    pub url: String,
    pub cents: i64,
}

/// Queues a search of every other store for a tracked product.
///
/// With `again`, stores already searched are searched once more; matches the
/// person confirmed or rejected themselves are left alone.
pub async fn enqueue(app: &App, user_id: i64, source_product_id: i64, again: bool) -> Result<()> {
    let Some(source) = db::product(&app.db, source_product_id).await? else {
        return Ok(());
    };
    let now = db::now();
    for store in stores(&app.db).await?.iter().filter(|s| s.retailer != source.retailer) {
        sqlx::query(
            "INSERT INTO comparisons (user_id, source_product_id, retailer, status, created_at, updated_at)
             VALUES (?, ?, ?, 'pending', ?, ?) ON CONFLICT (user_id, source_product_id, retailer) DO NOTHING",
        )
        .bind(user_id)
        .bind(source_product_id)
        .bind(&store.retailer)
        .bind(now)
        .bind(now)
        .execute(&app.db)
        .await?;
    }
    if again {
        sqlx::query(
            "UPDATE comparisons SET status = 'pending', updated_at = ?
             WHERE user_id = ? AND source_product_id = ? AND confirmed = 0 AND status != 'rejected'",
        )
        .bind(now)
        .bind(user_id)
        .bind(source_product_id)
        .execute(&app.db)
        .await?;
    }
    app.compare_wake.notify_one();
    Ok(())
}

/// The person's verdict on a suggested match.
pub async fn decide(app: &App, user_id: i64, comparison_id: i64, same: bool) -> Result<Option<i64>> {
    let row: Option<Comparison> = sqlx::query_as("SELECT * FROM comparisons WHERE id = ? AND user_id = ?")
        .bind(comparison_id)
        .bind(user_id)
        .fetch_optional(&app.db)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if same {
        sqlx::query(
            "UPDATE comparisons SET status = 'found', confirmed = 1, updated_at = ? WHERE id = ? AND product_id IS NOT NULL",
        )
        .bind(db::now())
        .bind(row.id)
        .execute(&app.db)
        .await?;
        check_cheaper(app, user_id, row.source_product_id).await?;
    } else {
        sqlx::query("UPDATE comparisons SET status = 'rejected', confirmed = 0, product_id = NULL, updated_at = ? WHERE id = ?")
            .bind(db::now())
            .bind(row.id)
            .execute(&app.db)
            .await?;
        db::delete_unused_products(&app.db).await?;
    }
    Ok(Some(row.source_product_id))
}

/// Every other store's row for one tracked product, in the stores' usual order.
pub async fn for_product(db: &Db, user_id: i64, source_product_id: i64) -> Result<Vec<StoreRow>> {
    let rows: Vec<Comparison> = sqlx::query_as("SELECT * FROM comparisons WHERE user_id = ? AND source_product_id = ?")
        .bind(user_id)
        .bind(source_product_id)
        .fetch_all(db)
        .await?;
    let rules = db::rules(db, user_id).await?;
    let mut out = Vec::new();
    for store in stores(db).await? {
        let Some(comparison) = rows.iter().find(|r| r.retailer == store.retailer).cloned() else {
            continue;
        };
        let product = match comparison.product_id {
            Some(id) => db::product(db, id).await?,
            None => None,
        };
        out.push(store_row(comparison, store.name, product, &rules));
    }
    Ok(out)
}

/// True when an offer should be left out of comparisons for this person.
fn left_out(row: &StoreRow, ignore_marketplace: bool) -> bool {
    row.product.as_ref().is_none_or(|p| p.in_stock == Some(false)) || (ignore_marketplace && row.marketplace)
}

/// For each product a person tracks: the cheapest in-stock match elsewhere,
/// delivery included, for the same amount as the tracked product.
pub async fn best_elsewhere(db: &Db, user_id: i64) -> Result<HashMap<i64, Elsewhere>> {
    let rows: Vec<Comparison> =
        sqlx::query_as("SELECT * FROM comparisons WHERE user_id = ? AND status = 'found' AND product_id IS NOT NULL")
            .bind(user_id)
            .fetch_all(db)
            .await?;
    let rules = db::rules(db, user_id).await?;
    let ignore_marketplace = db::settings(db, user_id).await?.ignore_marketplace;
    let mut best: HashMap<i64, Elsewhere> = HashMap::new();
    for comparison in rows {
        let (source_id, Some(product_id)) = (comparison.source_product_id, comparison.product_id) else { continue };
        let Some(product) = db::product(db, product_id).await? else { continue };
        let store_name = retailers::display_name(&product.retailer);
        let row = store_row(comparison, store_name, Some(product), &rules);
        let (Some(cents), Some(product)) = (row.equivalent_cents, &row.product) else { continue };
        if left_out(&row, ignore_marketplace) {
            continue;
        }
        if best.get(&source_id).is_none_or(|b| cents < b.cents) {
            best.insert(source_id, Elsewhere { store_name: row.store_name.clone(), product_id, url: product.url.clone(), cents });
        }
    }
    Ok(best)
}

/// Tells a person when a matched store beats the store they track.
/// Announced once per price; again only if it gets cheaper still.
pub async fn check_cheaper(app: &App, user_id: i64, source_product_id: i64) -> Result<()> {
    let Some(source) = db::product(&app.db, source_product_id).await? else {
        return Ok(());
    };
    if db::watch(&app.db, user_id, source_product_id).await?.is_none() {
        return Ok(());
    }
    let rules = db::rules(&app.db, user_id).await?;
    let member = db::member_prices(&app.db, user_id).await?.get(&source_product_id).copied();
    // Both sides are compared with delivery included.
    let Some(source_cents) = pricing::view(&source, member, &rules).total_cents() else {
        return Ok(());
    };
    let ignore_marketplace = db::settings(&app.db, user_id).await?.ignore_marketplace;
    let source_store = retailers::display_name(&source.retailer);
    for row in for_product(&app.db, user_id, source_product_id).await? {
        let (Some(product), Some(cents)) = (&row.product, row.equivalent_cents) else { continue };
        if row.comparison.status != "found" {
            continue;
        }
        let cheaper = cents < source_cents && !left_out(&row, ignore_marketplace);
        let already_told = row.comparison.last_notified_cents.is_some_and(|told| cents >= told);
        if cheaper && !already_told {
            let title = match &row.your_amount {
                Some(amount) => {
                    format!("{}: {} for {amount}, {} at {source_store}", row.store_name, dollars(cents), dollars(source_cents))
                }
                None => format!("{}: {} here, {} at {source_store}", row.store_name, dollars(cents), dollars(source_cents)),
            };
            let mut body = format!(
                "{}\nThe same product is {} cheaper at {}.",
                source.display_title(),
                dollars(source_cents - cents),
                row.store_name
            );
            // What the comparison rests on, so the person can check it.
            if let (Some(theirs), Some(yours), Some(listed)) = (&row.their_amount, &row.your_amount, row.your_cents) {
                body.push_str(&format!(
                    " It is sold there as {theirs} for {}; the price above is for {yours}, the amount you track.",
                    dollars(listed)
                ));
            }
            if let Some(shipping) = row.shipping_cents.filter(|s| *s > 0) {
                body.push_str(&format!(" That includes {} for delivery.", dollars(shipping)));
            } else if row.shipping_extra {
                body.push_str(" Delivery is charged on top there, and the page does not say how much.");
            }
            if row.marketplace {
                let seller = product.seller.as_deref().unwrap_or("another seller");
                body.push_str(&format!(" Sold by {seller}, a marketplace seller, not by {} itself.", row.store_name));
            }
            // A match only the AI vouches for is announced as such.
            if row.comparison.ai_judged() {
                body.push_str(" Your AI judged these to be the same product; check the listing before buying.");
            }
            alerts::notify(app, user_id, Some(source_product_id), "cheaper", &title, &body, Some(cents), &product.url).await?;
        }
        // Remember the announced price; forget it once this store is no longer cheaper.
        let remembered =
            if cheaper { Some(row.comparison.last_notified_cents.map_or(cents, |told| told.min(cents))) } else { None };
        sqlx::query("UPDATE comparisons SET last_notified_cents = ? WHERE id = ?")
            .bind(remembered)
            .bind(row.comparison.id)
            .execute(&app.db)
            .await?;
    }
    Ok(())
}

/// Called when any product's price changes: re-checks "cheaper elsewhere"
/// for everyone who tracks it or has it as a match.
pub async fn price_changed(app: &App, product_id: i64) -> Result<()> {
    let pairs: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT DISTINCT user_id, source_product_id FROM comparisons
         WHERE status = 'found' AND (source_product_id = ?1 OR product_id = ?1)",
    )
    .bind(product_id)
    .fetch_all(&app.db)
    .await?;
    for (user_id, source_id) in pairs {
        check_cheaper(app, user_id, source_id).await?;
    }
    Ok(())
}

fn facts(product: &Product) -> Facts {
    Facts {
        details: product.details.clone(),
        ..facts_of(
            product.title.as_deref(),
            product.brand.as_deref(),
            product.model.as_deref(),
            product.gtin.as_deref(),
            product.current_price_cents,
        )
    }
}

/// The background worker: searches one store at a time for queued products.
pub async fn run(app: App) {
    loop {
        // Only products whose own page has been read: the search needs their name.
        let next: Result<Option<Comparison>, sqlx::Error> = sqlx::query_as(
            "SELECT c.* FROM comparisons c JOIN products p ON p.id = c.source_product_id
             WHERE c.status = 'pending' AND p.title IS NOT NULL ORDER BY c.updated_at, c.id LIMIT 1",
        )
        .fetch_optional(&app.db)
        .await;
        match next {
            Ok(Some(row)) => {
                if let Err(e) = search_one(&app, &row).await {
                    tracing::error!("comparison {} failed: {e:#}", row.id);
                    let _ = finish(&app.db, row.id, "failed", None, None, Some("the server hit a problem"), "rules").await;
                }
            }
            Ok(None) => {
                tokio::select! {
                    _ = app.compare_wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                }
            }
            Err(e) => {
                tracing::error!("could not load comparisons: {e:#}");
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        }
    }
}

/// Records that the other store's listing holds a different amount of the same item.
async fn set_pack(db: &Db, id: i64, pack: &PackDiff) -> Result<()> {
    sqlx::query("UPDATE comparisons SET source_qty = ?, other_qty = ?, qty_unit = ? WHERE id = ?")
        .bind(pack.source_qty)
        .bind(pack.other_qty)
        .bind(&pack.unit)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

async fn finish(
    db: &Db,
    id: i64,
    status: &str,
    product_id: Option<i64>,
    score: Option<i64>,
    reason: Option<&str>,
    judged_by: &str,
) -> Result<()> {
    // A new answer starts without a pack difference; `set_pack` adds one.
    sqlx::query(
        "UPDATE comparisons SET status = ?, product_id = ?, score = ?, reason = ?, judged_by = ?, updated_at = ?,
            source_qty = NULL, other_qty = NULL, qty_unit = NULL WHERE id = ?",
    )
    .bind(status)
    .bind(product_id)
    .bind(score)
    .bind(reason)
    .bind(judged_by)
    .bind(db::now())
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

async fn search_one(app: &App, row: &Comparison) -> Result<()> {
    let Some(source) = db::product(&app.db, row.source_product_id).await? else {
        return Ok(());
    };
    let all = stores(&app.db).await?;
    let Some(store) = all.iter().find(|s| s.retailer == row.retailer) else {
        return finish(&app.db, row.id, "failed", None, None, Some("this store cannot be searched"), "rules").await;
    };
    let source_facts = facts(&source);

    // Free answer first: a product already tracked at that store with the same barcode.
    if let Some(gtin) = source.gtin.as_deref().filter(|g| g.len() >= 8) {
        let known: Option<i64> = sqlx::query_scalar("SELECT id FROM products WHERE retailer = ? AND gtin = ? LIMIT 1")
            .bind(&row.retailer)
            .bind(gtin)
            .fetch_optional(&app.db)
            .await?;
        if let Some(product_id) = known {
            finish(&app.db, row.id, "found", Some(product_id), Some(100), Some("Same barcode"), "rules").await?;
            return check_cheaper(app, row.user_id, row.source_product_id).await;
        }
    }

    // The person's AI service, when one is connected, helps pick and judge.
    let settings = db::settings(&app.db, row.user_id).await?;
    let advisor = ai::AiConfig::for_matching(&settings).ok().map(|cfg| Advisor::new(&app.http, cfg));

    tracing::info!(product = source.id, store = store.retailer, ai = advisor.is_some(), "looking for the same product");
    match find_at_store(&app.fetcher, &source_facts, store, advisor.as_ref()).await {
        StoreResult::Found(found) => {
            let reference = ProductRef {
                retailer: store.retailer.clone(),
                sku: found.candidate.sku.clone(),
                url: found.candidate.url.clone(),
            };
            let product_id = db::upsert_product(&app.db, &reference).await?;
            let interval = app.cfg.check_interval.as_secs() as i64;
            db::record_observation(&app.db, product_id, &found.obs, "server", Audience::Public, Some(found.mode), Some(interval))
                .await?;
            debug_assert!(found.score >= POSSIBLE);
            let status = if found.score >= SAME { "found" } else { "possible" };
            let judged_by = if found.by_ai { "ai" } else { "rules" };
            finish(&app.db, row.id, status, Some(product_id), Some(found.score as i64), Some(&found.reason), judged_by).await?;
            if let Some(pack) = &found.pack {
                set_pack(&app.db, row.id, pack).await?;
            }
            check_cheaper(app, row.user_id, row.source_product_id).await
        }
        StoreResult::NotFound(note) => {
            let judged_by = if note.is_some() { "ai" } else { "rules" };
            finish(&app.db, row.id, "none", None, None, note.as_deref(), judged_by).await?;
            // Pages opened along the way are kept only if someone uses them.
            db::delete_unused_products(&app.db).await
        }
        StoreResult::Failed(why) => finish(&app.db, row.id, "failed", None, None, Some(&why), "rules").await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retailers::Observation;
    use crate::test_app;

    async fn tracked(app: &App, user: i64, url: &str, title: &str, cents: i64) -> i64 {
        let id = crate::service::add_by_url(app, user, url, None).await.unwrap();
        let obs = Observation { title: Some(title.into()), price_cents: Some(cents), in_stock: Some(true), ..Default::default() };
        db::record_observation(&app.db, id, &obs, "server", Audience::Public, None, Some(60)).await.unwrap();
        id
    }

    /// Stores a match the way the worker does after a successful search.
    async fn matched(app: &App, user: i64, source: i64, store: &str, sku: &str, cents: i64, status: &str) -> i64 {
        matched_by(app, user, source, store, sku, cents, status, "rules").await
    }

    #[allow(clippy::too_many_arguments)]
    async fn matched_by(
        app: &App,
        user: i64,
        source: i64,
        store: &str,
        sku: &str,
        cents: i64,
        status: &str,
        judged_by: &str,
    ) -> i64 {
        let reference = ProductRef { retailer: store.into(), sku: sku.into(), url: format!("https://{store}.example/{sku}") };
        let product_id = db::upsert_product(&app.db, &reference).await.unwrap();
        let obs =
            Observation { title: Some("Chair".into()), price_cents: Some(cents), in_stock: Some(true), ..Default::default() };
        db::record_observation(&app.db, product_id, &obs, "server", Audience::Public, None, Some(60)).await.unwrap();
        let row: i64 =
            sqlx::query_scalar("SELECT id FROM comparisons WHERE user_id = ? AND source_product_id = ? AND retailer = ?")
                .bind(user)
                .bind(source)
                .bind(store)
                .fetch_one(&app.db)
                .await
                .unwrap();
        finish(&app.db, row, status, Some(product_id), Some(95), Some("Same barcode"), judged_by).await.unwrap();
        check_cheaper(app, user, source).await.unwrap();
        product_id
    }

    async fn alert_titles(app: &App, user: i64) -> Vec<String> {
        let mut titles: Vec<String> =
            db::notifications(&app.db, user, 0, 50).await.unwrap().into_iter().map(|n| n.title).collect();
        titles.reverse();
        titles
    }

    #[tokio::test]
    async fn adding_a_product_queues_every_other_store() {
        let app = test_app().await;
        let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        let id = tracked(&app, user, "https://www.staples.com/chair/product_555", "Chair", 10000).await;
        let rows = for_product(&app.db, user, id).await.unwrap();
        let stores: Vec<&str> = rows.iter().map(|r| r.comparison.retailer.as_str()).collect();
        assert_eq!(
            stores,
            vec!["bestbuy", "walmart", "target", "amazon", "kohls", "harborfreight", "hottopic"],
            "not Staples itself"
        );
        assert!(rows.iter().all(|r| r.comparison.status == "pending"));

        // Turning the setting off stops new products being queued.
        sqlx::query("UPDATE user_settings SET auto_compare = 0 WHERE user_id = ?").bind(user).execute(&app.db).await.unwrap();
        let other = tracked(&app, user, "https://www.staples.com/desk/product_777", "Desk", 5000).await;
        assert!(for_product(&app.db, user, other).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cheaper_store_is_announced_once_and_shown_as_best() {
        let app = test_app().await;
        let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        let id = tracked(&app, user, "https://www.staples.com/chair/product_555", "Chair", 10000).await;

        matched(&app, user, id, "target", "A1", 11000, "found").await; // dearer: nothing to say
        let walmart = matched(&app, user, id, "walmart", "W1", 9000, "found").await;
        matched(&app, user, id, "amazon", "B1", 5000, "possible").await; // unconfirmed: not announced
        assert_eq!(alert_titles(&app, user).await, vec!["Walmart: $90.00 here, $100.00 at Staples"]);

        let best = best_elsewhere(&app.db, user).await.unwrap();
        assert_eq!((best[&id].store_name.as_str(), best[&id].cents), ("Walmart", 9000));

        // Same price again: silent. Lower: announced. The matched product keeps being price-checked.
        check_cheaper(&app, user, id).await.unwrap();
        assert_eq!(alert_titles(&app, user).await.len(), 1);
        let obs = Observation { price_cents: Some(8000), ..Default::default() };
        db::record_observation(&app.db, walmart, &obs, "server", Audience::Public, None, Some(60)).await.unwrap();
        price_changed(&app, walmart).await.unwrap();
        assert_eq!(alert_titles(&app, user).await.last().unwrap(), "Walmart: $80.00 here, $100.00 at Staples");
        sqlx::query("UPDATE products SET next_check_at = 0").execute(&app.db).await.unwrap();
        let due: Vec<i64> = db::due_products(&app.db, 50).await.unwrap().into_iter().map(|p| p.id).collect();
        assert!(due.contains(&walmart), "matched products are kept up to date");
    }

    #[tokio::test]
    async fn personal_discount_counts_when_comparing_stores() {
        let app = test_app().await;
        let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        let id = tracked(&app, user, "https://www.staples.com/chair/product_555", "Chair", 10000).await;
        db::add_rule(&app.db, user, "staples", "Associate discount", 10.0, "", "").await.unwrap();
        // $95 at Walmart beats the listed $100 but not the $90 this person pays at Staples.
        matched(&app, user, id, "walmart", "W1", 9500, "found").await;
        assert!(alert_titles(&app, user).await.is_empty());
    }

    #[tokio::test]
    async fn confirming_and_rejecting_a_possible_match() {
        let app = test_app().await;
        let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        let id = tracked(&app, user, "https://www.staples.com/chair/product_555", "Chair", 10000).await;
        let amazon = matched(&app, user, id, "amazon", "B1", 7000, "possible").await;
        let kohls = matched(&app, user, id, "kohls", "K1", 6000, "possible").await;
        let row = |store: &'static str| {
            let app = app.clone();
            async move { for_product(&app.db, user, id).await.unwrap().into_iter().find(|r| r.comparison.retailer == store).unwrap() }
        };

        assert_eq!(decide(&app, user, row("amazon").await.comparison.id, true).await.unwrap(), Some(id));
        assert_eq!(row("amazon").await.comparison.status, "found");
        assert_eq!(alert_titles(&app, user).await, vec!["Amazon: $70.00 here, $100.00 at Staples"]);

        decide(&app, user, row("kohls").await.comparison.id, false).await.unwrap();
        assert_eq!(row("kohls").await.comparison.status, "rejected");
        assert!(db::product(&app.db, kohls).await.unwrap().is_none(), "the rejected product is no longer kept");
        assert!(db::product(&app.db, amazon).await.unwrap().is_some());

        // "Search again" leaves the person's own decisions alone.
        enqueue(&app, user, id, true).await.unwrap();
        assert_eq!(row("amazon").await.comparison.status, "found");
        assert_eq!(row("kohls").await.comparison.status, "rejected");
        assert_eq!(row("target").await.comparison.status, "pending");

        // Someone else cannot decide for this person.
        let other = db::create_user(&app.db, "alex", "h", false).await.unwrap();
        assert_eq!(decide(&app, other, row("amazon").await.comparison.id, false).await.unwrap(), None);

        // Stopping tracking removes the matches and their products.
        db::remove_watch(&app.db, user, id).await.unwrap();
        assert!(db::product(&app.db, amazon).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_match_made_by_the_ai_says_so_until_the_person_confirms_it() {
        let app = test_app().await;
        let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        let id = tracked(&app, user, "https://www.staples.com/chair/product_555", "Chair", 10000).await;
        matched_by(&app, user, id, "walmart", "W1", 9000, "found", "ai").await;
        let row = for_product(&app.db, user, id).await.unwrap().into_iter().find(|r| r.comparison.retailer == "walmart").unwrap();
        assert!(row.comparison.ai_judged());
        let alert = db::notifications(&app.db, user, 0, 50).await.unwrap().remove(0);
        assert!(alert.body.contains("Your AI judged these to be the same product"), "{}", alert.body);

        decide(&app, user, row.comparison.id, true).await.unwrap();
        let row = for_product(&app.db, user, id).await.unwrap().into_iter().find(|r| r.comparison.retailer == "walmart").unwrap();
        assert!(!row.comparison.ai_judged(), "confirmed by the person");
    }

    #[tokio::test]
    async fn another_pack_size_is_compared_for_the_same_amount() {
        let app = test_app().await;
        let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        // A carton of paper at Staples for $60; the same paper by the ream at Walmart for $5.
        let id = tracked(&app, user, "https://www.staples.com/paper/product_555", "Paper, 5000 Sheets", 6000).await;
        let walmart = matched(&app, user, id, "walmart", "W1", 500, "found").await;
        let row_id: i64 =
            sqlx::query_scalar("SELECT id FROM comparisons WHERE product_id = ?").bind(walmart).fetch_one(&app.db).await.unwrap();
        // Without the amounts it looks twelve times cheaper, which was announced.
        assert_eq!(alert_titles(&app, user).await, vec!["Walmart: $5.00 here, $60.00 at Staples"]);

        set_pack(&app.db, row_id, &PackDiff { source_qty: 5000.0, other_qty: 500.0, unit: "sheets".into() }).await.unwrap();
        sqlx::query("UPDATE comparisons SET last_notified_cents = NULL").execute(&app.db).await.unwrap();
        check_cheaper(&app, user, id).await.unwrap();
        let alert = db::notifications(&app.db, user, 0, 50).await.unwrap().remove(0);
        assert_eq!(alert.title, "Walmart: $50.00 for 5000 sheets, $60.00 at Staples");
        assert!(alert.body.contains("sold there as 500 sheets for $5.00"), "{}", alert.body);
        let best = best_elsewhere(&app.db, user).await.unwrap();
        assert_eq!(best[&id].cents, 5000);

        // At $7 a ream it is dearer for the same amount: no longer the best, nothing announced.
        let obs = Observation { price_cents: Some(700), ..Default::default() };
        db::record_observation(&app.db, walmart, &obs, "server", Audience::Public, None, Some(60)).await.unwrap();
        let before = alert_titles(&app, user).await.len();
        price_changed(&app, walmart).await.unwrap();
        assert_eq!(alert_titles(&app, user).await.len(), before);
        assert_eq!(best_elsewhere(&app.db, user).await.unwrap()[&id].cents, 7000);
    }

    #[tokio::test]
    async fn delivery_and_marketplace_sellers_count_when_comparing() {
        let app = test_app().await;
        let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        let id = tracked(&app, user, "https://www.staples.com/chair/product_555", "Chair", 10000).await;

        // $90 at Amazon from another seller, plus $15 delivery: dearer than $100 in all.
        let amazon = matched(&app, user, id, "amazon", "B1", 9000, "found").await;
        let offer = Observation {
            price_cents: Some(9000),
            seller: Some("Chairs R Us".into()),
            third_party: Some(true),
            store_ships: Some(false),
            ships_free: Some(false),
            shipping_cents: Some(1500),
            ..Default::default()
        };
        sqlx::query("DELETE FROM notifications").execute(&app.db).await.unwrap();
        sqlx::query("UPDATE comparisons SET last_notified_cents = NULL").execute(&app.db).await.unwrap();
        db::record_observation(&app.db, amazon, &offer, "server", Audience::Public, None, Some(60)).await.unwrap();
        price_changed(&app, amazon).await.unwrap();
        assert!(alert_titles(&app, user).await.is_empty(), "$90 + $15 delivery is not cheaper than $100");
        assert!(best_elsewhere(&app.db, user).await.unwrap().get(&id).is_some_and(|b| b.cents == 10500));

        // It drops to $70: cheaper even with delivery, and the alert says who sells it.
        let offer = Observation { price_cents: Some(7000), ..offer };
        db::record_observation(&app.db, amazon, &offer, "server", Audience::Public, None, Some(60)).await.unwrap();
        price_changed(&app, amazon).await.unwrap();
        let alert = db::notifications(&app.db, user, 0, 50).await.unwrap().remove(0);
        assert_eq!(alert.title, "Amazon: $85.00 here, $100.00 at Staples");
        assert!(alert.body.contains("includes $15.00 for delivery"), "{}", alert.body);
        assert!(alert.body.contains("Sold by Chairs R Us, a marketplace seller"), "{}", alert.body);

        // A membership does not cover what the seller ships itself.
        db::add_terms(&app.db, user, "amazon", "Prime", 0.0, "", "", true).await.unwrap();
        assert_eq!(best_elsewhere(&app.db, user).await.unwrap()[&id].cents, 8500);
        // Once Amazon ships it, the membership makes delivery free.
        sqlx::query("UPDATE products SET store_ships = 1 WHERE id = ?").bind(amazon).execute(&app.db).await.unwrap();
        assert_eq!(best_elsewhere(&app.db, user).await.unwrap()[&id].cents, 7000);

        // Choosing to leave marketplace sellers out removes the offer from comparisons.
        sqlx::query("UPDATE user_settings SET ignore_marketplace = 1 WHERE user_id = ?")
            .bind(user)
            .execute(&app.db)
            .await
            .unwrap();
        assert!(!best_elsewhere(&app.db, user).await.unwrap().contains_key(&id));
    }

    #[tokio::test]
    async fn an_added_store_is_searched_and_named_like_the_others() {
        let app = test_app().await;
        let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        db::save_custom_store(&app.db, "boxlunch.com", "BoxLunch", "https://www.boxlunch.com/search?q={q}", user).await.unwrap();
        let id = tracked(&app, user, "https://www.staples.com/chair/product_555", "Chair", 10000).await;
        let rows = for_product(&app.db, user, id).await.unwrap();
        let last = rows.last().unwrap();
        assert_eq!((last.comparison.retailer.as_str(), last.store_name.as_str()), ("boxlunch.com", "BoxLunch"));

        // Removing the store removes what was found there.
        let store = db::custom_stores(&app.db).await.unwrap().remove(0);
        db::remove_custom_store(&app.db, store.id).await.unwrap();
        assert!(for_product(&app.db, user, id).await.unwrap().iter().all(|r| r.comparison.retailer != "boxlunch.com"));
    }
}
