//! Database access: opening the SQLite file, row types, and the queries the
//! rest of the server uses.

use crate::retailers::{FetchMode, Observation, ProductRef};
use anyhow::Result;
use serde::Serialize;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::Path;

pub type Db = SqlitePool;

pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

pub const DAY: i64 = 86_400;

/// A signed-in price older than this is no longer trusted.
pub const MEMBER_PRICE_MAX_AGE: i64 = 7 * DAY;

pub async fn open(data_dir: &Path) -> Result<Db> {
    std::fs::create_dir_all(data_dir)?;
    let options = SqliteConnectOptions::new()
        .filename(data_dir.join("amepricetrack.db"))
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(10));
    let pool = SqlitePoolOptions::new().max_connections(8).connect_with(options).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

#[cfg(test)]
pub async fn open_in_memory() -> Db {
    let options = SqliteConnectOptions::new().in_memory(true).foreign_keys(true);
    // One connection, so every query sees the same in-memory database.
    let pool = SqlitePoolOptions::new().max_connections(1).connect_with(options).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

// ------------------------------------------------------------------ rows

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub is_admin: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Product {
    pub id: i64,
    pub retailer: String,
    pub retailer_sku: String,
    pub url: String,
    pub title: Option<String>,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub gtin: Option<String>,
    pub image_url: Option<String>,
    pub current_price_cents: Option<i64>,
    pub list_price_cents: Option<i64>,
    pub in_stock: Option<bool>,
    pub promo_text: Option<String>,
    pub lowest_price_cents: Option<i64>,
    pub lowest_price_at: Option<i64>,
    pub fetch_mode: Option<String>,
    pub created_at: i64,
    pub last_checked_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub next_check_at: i64,
    pub fail_count: i64,
    pub last_error: Option<String>,
    /// Colour, size and a line of description from the product page.
    pub details: Option<String>,
}

impl Product {
    pub fn display_title(&self) -> String {
        self.title.clone().unwrap_or_else(|| self.url.clone())
    }
    pub fn remembered_mode(&self) -> Option<FetchMode> {
        self.fetch_mode.as_deref().and_then(FetchMode::from_str)
    }
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Watch {
    pub id: i64,
    pub user_id: i64,
    pub product_id: i64,
    pub target_price_cents: Option<i64>,
    pub drop_percent: Option<f64>,
    pub notify_any_drop: bool,
    pub notify_back_in_stock: bool,
    pub note: Option<String>,
    pub created_at: i64,
    pub last_seen_price_cents: Option<i64>,
    pub last_seen_in_stock: Option<bool>,
    pub last_alert_price_cents: Option<i64>,
    pub last_alert_at: Option<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct DiscountRule {
    pub id: i64,
    pub user_id: i64,
    pub retailer: String,
    pub name: String,
    pub percent: f64,
    pub brands: String,
    pub exclude_keywords: String,
    pub enabled: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct PricePoint {
    pub ts: i64,
    pub price_cents: i64,
    pub list_price_cents: Option<i64>,
    pub in_stock: Option<bool>,
    pub source: String,
    /// True when this price was seen while signed in at the store.
    pub member: bool,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Notification {
    pub id: i64,
    pub user_id: i64,
    pub product_id: Option<i64>,
    pub ts: i64,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub price_cents: Option<i64>,
    pub read_at: Option<i64>,
}

#[derive(Debug, Clone, Default, sqlx::FromRow)]
pub struct Settings {
    #[allow(dead_code)]
    pub user_id: i64,
    pub ntfy_url: String,
    pub webhook_url: String,
    pub ai_provider: String,
    pub ai_base_url: String,
    pub ai_api_key: String,
    pub ai_model: String,
    /// Theme id; see `ui::THEMES`.
    pub theme: String,
    /// Custom accent colour as #rrggbb, or empty for the theme's own.
    pub accent: String,
    /// Look newly added products up at the other stores automatically.
    pub auto_compare: bool,
    /// AI model for matching products across stores; empty to use `ai_model`.
    pub ai_match_model: String,
}

/// A store someone added to this server.
#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct CustomStore {
    pub id: i64,
    pub retailer: String,
    pub name: String,
    pub search_url: String,
    pub added_by: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SessionRow {
    pub id: i64,
    pub label: Option<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct ChatMessage {
    pub id: i64,
    pub ts: i64,
    pub role: String,
    pub content: String,
}

// ------------------------------------------------------------------ users

pub async fn user_count(db: &Db) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(db).await?)
}

pub async fn user_by_name(db: &Db, username: &str) -> Result<Option<User>> {
    Ok(sqlx::query_as("SELECT * FROM users WHERE username = ?").bind(username).fetch_optional(db).await?)
}

pub async fn list_users(db: &Db) -> Result<Vec<User>> {
    Ok(sqlx::query_as("SELECT * FROM users ORDER BY id").fetch_all(db).await?)
}

pub async fn create_user(db: &Db, username: &str, password_hash: &str, is_admin: bool) -> Result<i64> {
    let id = sqlx::query("INSERT INTO users (username, password_hash, is_admin, created_at) VALUES (?, ?, ?, ?)")
        .bind(username)
        .bind(password_hash)
        .bind(is_admin)
        .bind(now())
        .execute(db)
        .await?
        .last_insert_rowid();
    sqlx::query("INSERT INTO user_settings (user_id) VALUES (?)").bind(id).execute(db).await?;
    Ok(id)
}

pub async fn settings(db: &Db, user_id: i64) -> Result<Settings> {
    let row: Option<Settings> =
        sqlx::query_as("SELECT * FROM user_settings WHERE user_id = ?").bind(user_id).fetch_optional(db).await?;
    Ok(row.unwrap_or(Settings { user_id, ..Default::default() }))
}

// --------------------------------------------------------------- products

/// Finds or creates the shared product row for a store item.
pub async fn upsert_product(db: &Db, p: &ProductRef) -> Result<i64> {
    let ts = now();
    sqlx::query(
        "INSERT INTO products (retailer, retailer_sku, url, created_at, next_check_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (retailer, retailer_sku) DO NOTHING",
    )
    .bind(&p.retailer)
    .bind(&p.sku)
    .bind(&p.url)
    .bind(ts)
    .bind(ts)
    .execute(db)
    .await?;
    Ok(sqlx::query_scalar("SELECT id FROM products WHERE retailer = ? AND retailer_sku = ?")
        .bind(&p.retailer)
        .bind(&p.sku)
        .fetch_one(db)
        .await?)
}

pub async fn product(db: &Db, id: i64) -> Result<Option<Product>> {
    Ok(sqlx::query_as("SELECT * FROM products WHERE id = ?").bind(id).fetch_optional(db).await?)
}

/// Products whose next check is due, oldest first: everything someone tracks,
pub async fn due_products(db: &Db, limit: i64) -> Result<Vec<Product>> {
    Ok(sqlx::query_as(
        "SELECT * FROM products
         WHERE next_check_at <= ?
           AND (id IN (SELECT product_id FROM watches)
                -- ...and the matches found at other stores for products someone tracks.
                OR id IN (SELECT c.product_id FROM comparisons c
                          JOIN watches w ON w.product_id = c.source_product_id AND w.user_id = c.user_id
                          WHERE c.status IN ('found', 'possible') AND c.product_id IS NOT NULL))
         ORDER BY next_check_at LIMIT ?",
    )
    .bind(now())
    .bind(limit)
    .fetch_all(db)
    .await?)
}

pub async fn check_soon(db: &Db, product_id: i64) -> Result<()> {
    sqlx::query("UPDATE products SET next_check_at = 0 WHERE id = ?").bind(product_id).execute(db).await?;
    Ok(())
}

pub async fn record_failure(db: &Db, product_id: i64, error: &str, retry_in: i64) -> Result<()> {
    let ts = now();
    sqlx::query(
        "UPDATE products SET last_checked_at = ?, next_check_at = ?, fail_count = fail_count + 1, last_error = ?
         WHERE id = ?",
    )
    .bind(ts)
    .bind(ts + retry_in)
    .bind(error)
    .bind(product_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Who a reading belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Audience {
    /// The price anyone sees.
    Public,
    /// A price seen while this user was signed in at the store.
    Member(i64),
}

/// Stores one reading of a product. Returns true when the public price or
/// stock status changed (which is when alerts need re-evaluating).
///
/// `next_check_in` is given by the scheduler; readings from a phone leave the
/// schedule alone.
pub async fn record_observation(
    db: &Db,
    product_id: i64,
    obs: &Observation,
    source: &str,
    audience: Audience,
    mode: Option<FetchMode>,
    next_check_in: Option<i64>,
) -> Result<bool> {
    let ts = now();
    let Some(before) = product(db, product_id).await? else {
        return Ok(false);
    };

    // Descriptive details: keep what we have when the new reading lacks a field.
    sqlx::query(
        "UPDATE products SET
            title = COALESCE(?, title), brand = COALESCE(?, brand), model = COALESCE(?, model),
            gtin = COALESCE(?, gtin), image_url = COALESCE(?, image_url), details = COALESCE(?, details)
         WHERE id = ?",
    )
    .bind(&obs.title)
    .bind(&obs.brand)
    .bind(&obs.model)
    .bind(&obs.gtin)
    .bind(&obs.image_url)
    .bind(&obs.details)
    .bind(product_id)
    .execute(db)
    .await?;

    // Chains that price per store say which store they chose for the server.
    if let (Some(store), "server") = (&obs.local_store, source) {
        let key = format!("{}_store", before.retailer);
        let value = serde_json::to_string(store)?;
        if server_setting(db, &key).await?.as_deref() != Some(value.as_str()) {
            set_server_setting(db, &key, &value).await?;
        }
    }

    if let Audience::Member(user_id) = audience {
        if let Some(price) = obs.price_cents {
            let last: Option<i64> = sqlx::query_scalar(
                "SELECT price_cents FROM price_points WHERE product_id = ? AND user_id = ? ORDER BY ts DESC, id DESC LIMIT 1",
            )
            .bind(product_id)
            .bind(user_id)
            .fetch_optional(db)
            .await?;
            if last == Some(price) {
                // Same as before: just refresh its age so it stays trusted.
                sqlx::query(
                    "UPDATE price_points SET ts = ? WHERE id = (SELECT id FROM price_points
                     WHERE product_id = ? AND user_id = ? ORDER BY ts DESC, id DESC LIMIT 1)",
                )
                .bind(ts)
                .bind(product_id)
                .bind(user_id)
                .execute(db)
                .await?;
            } else {
                insert_point(db, product_id, Some(user_id), ts, price, obs, source).await?;
            }
        }
        return Ok(true);
    }

    let stock_changed = obs.in_stock.is_some() && obs.in_stock != before.in_stock;
    let price_changed = obs.price_cents.is_some() && obs.price_cents != before.current_price_cents;

    if let Some(price) = obs.price_cents {
        if price_changed || stock_changed {
            insert_point(db, product_id, None, ts, price, obs, source).await?;
        }
        let new_low = before.lowest_price_cents.is_none_or(|low| price < low);
        sqlx::query(
            "UPDATE products SET current_price_cents = ?, list_price_cents = ?, promo_text = ?,
                in_stock = COALESCE(?, in_stock),
                lowest_price_cents = CASE WHEN ? THEN ? ELSE lowest_price_cents END,
                lowest_price_at = CASE WHEN ? THEN ? ELSE lowest_price_at END
             WHERE id = ?",
        )
        .bind(price)
        .bind(obs.list_price_cents)
        .bind(&obs.promo_text)
        .bind(obs.in_stock)
        .bind(new_low)
        .bind(price)
        .bind(new_low)
        .bind(ts)
        .bind(product_id)
        .execute(db)
        .await?;
    } else if stock_changed {
        // Out of stock with no price shown: keep the last known price.
        sqlx::query("UPDATE products SET in_stock = ? WHERE id = ?").bind(obs.in_stock).bind(product_id).execute(db).await?;
    }

    sqlx::query(
        "UPDATE products SET last_checked_at = ?, last_success_at = ?, fail_count = 0, last_error = NULL,
            fetch_mode = COALESCE(?, fetch_mode), next_check_at = COALESCE(?, next_check_at)
         WHERE id = ?",
    )
    .bind(ts)
    .bind(ts)
    .bind(mode.map(FetchMode::as_str))
    .bind(next_check_in.map(|s| ts + s))
    .bind(product_id)
    .execute(db)
    .await?;

    Ok(price_changed || stock_changed)
}

async fn insert_point(
    db: &Db,
    product_id: i64,
    user_id: Option<i64>,
    ts: i64,
    price: i64,
    obs: &Observation,
    source: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO price_points (product_id, user_id, ts, price_cents, list_price_cents, in_stock, source)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(product_id)
    .bind(user_id)
    .bind(ts)
    .bind(price)
    .bind(obs.list_price_cents)
    .bind(obs.in_stock)
    .bind(source)
    .execute(db)
    .await?;
    Ok(())
}

/// Price history for one product: the public prices plus this user's signed-in prices.
pub async fn history(db: &Db, product_id: i64, user_id: i64) -> Result<Vec<PricePoint>> {
    Ok(sqlx::query_as(
        "SELECT ts, price_cents, list_price_cents, in_stock, source, (user_id IS NOT NULL) AS member
         FROM price_points WHERE product_id = ? AND (user_id IS NULL OR user_id = ?) ORDER BY ts, id",
    )
    .bind(product_id)
    .bind(user_id)
    .fetch_all(db)
    .await?)
}

/// This user's recent signed-in price per product: product id -> cents.
pub async fn member_prices(db: &Db, user_id: i64) -> Result<std::collections::HashMap<i64, i64>> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT p.product_id, p.price_cents FROM price_points p
         WHERE p.user_id = ? AND p.ts >= ?
           AND p.id = (SELECT id FROM price_points q WHERE q.product_id = p.product_id AND q.user_id = p.user_id
                       ORDER BY q.ts DESC, q.id DESC LIMIT 1)",
    )
    .bind(user_id)
    .bind(now() - MEMBER_PRICE_MAX_AGE)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().collect())
}

// ---------------------------------------------------------------- watches

pub async fn watch(db: &Db, user_id: i64, product_id: i64) -> Result<Option<Watch>> {
    Ok(sqlx::query_as("SELECT * FROM watches WHERE user_id = ? AND product_id = ?")
        .bind(user_id)
        .bind(product_id)
        .fetch_optional(db)
        .await?)
}

pub async fn watches_for_product(db: &Db, product_id: i64) -> Result<Vec<Watch>> {
    Ok(sqlx::query_as("SELECT * FROM watches WHERE product_id = ?").bind(product_id).fetch_all(db).await?)
}

/// Starts watching a product. Watching it again keeps the existing settings
/// unless a new target price is given.
pub async fn add_watch(db: &Db, user_id: i64, product_id: i64, target_price_cents: Option<i64>) -> Result<i64> {
    sqlx::query(
        "INSERT INTO watches (user_id, product_id, target_price_cents, created_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (user_id, product_id) DO UPDATE SET
            target_price_cents = COALESCE(excluded.target_price_cents, target_price_cents)",
    )
    .bind(user_id)
    .bind(product_id)
    .bind(target_price_cents)
    .bind(now())
    .execute(db)
    .await?;
    Ok(sqlx::query_scalar("SELECT id FROM watches WHERE user_id = ? AND product_id = ?")
        .bind(user_id)
        .bind(product_id)
        .fetch_one(db)
        .await?)
}

pub struct WatchEdit {
    pub target_price_cents: Option<i64>,
    pub drop_percent: Option<f64>,
    pub notify_any_drop: bool,
    pub notify_back_in_stock: bool,
    pub note: Option<String>,
}

pub async fn update_watch(db: &Db, user_id: i64, product_id: i64, e: &WatchEdit) -> Result<bool> {
    let done = sqlx::query(
        "UPDATE watches SET target_price_cents = ?, drop_percent = ?, notify_any_drop = ?,
            notify_back_in_stock = ?, note = ?, last_alert_price_cents = NULL
         WHERE user_id = ? AND product_id = ?",
    )
    .bind(e.target_price_cents)
    .bind(e.drop_percent)
    .bind(e.notify_any_drop)
    .bind(e.notify_back_in_stock)
    .bind(&e.note)
    .bind(user_id)
    .bind(product_id)
    .execute(db)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Stops watching. A product nobody needs any more is removed with its history.
pub async fn remove_watch(db: &Db, user_id: i64, product_id: i64) -> Result<bool> {
    let done = sqlx::query("DELETE FROM watches WHERE user_id = ? AND product_id = ?")
        .bind(user_id)
        .bind(product_id)
        .execute(db)
        .await?;
    // What was found at other stores for it goes too.
    sqlx::query("DELETE FROM comparisons WHERE user_id = ? AND source_product_id = ?")
        .bind(user_id)
        .bind(product_id)
        .execute(db)
        .await?;
    delete_unused_products(db).await?;
    Ok(done.rows_affected() > 0)
}

/// Removes products nobody tracks and nobody has as a match at another store.
pub async fn delete_unused_products(db: &Db) -> Result<()> {
    sqlx::query(
        "DELETE FROM products WHERE id NOT IN (SELECT product_id FROM watches)
           AND id NOT IN (SELECT product_id FROM comparisons WHERE product_id IS NOT NULL)",
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Everything a user watches, newest first.
pub async fn watched(db: &Db, user_id: i64) -> Result<Vec<(Product, Watch)>> {
    let products: Vec<Product> = sqlx::query_as(
        "SELECT p.* FROM products p JOIN watches w ON w.product_id = p.id WHERE w.user_id = ? ORDER BY w.created_at DESC, w.id DESC",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    let watches: Vec<Watch> = sqlx::query_as("SELECT * FROM watches WHERE user_id = ?").bind(user_id).fetch_all(db).await?;
    let mut by_product: std::collections::HashMap<i64, Watch> = watches.into_iter().map(|w| (w.product_id, w)).collect();
    Ok(products.into_iter().filter_map(|p| by_product.remove(&p.id).map(|w| (p, w))).collect())
}

/// Public price changes for everything a user watches, oldest first, used
/// for averages. (product id, time, cents)
pub async fn watched_price_changes(db: &Db, user_id: i64) -> Result<Vec<(i64, i64, i64)>> {
    Ok(sqlx::query_as(
        "SELECT product_id, ts, price_cents FROM price_points
         WHERE user_id IS NULL AND product_id IN (SELECT product_id FROM watches WHERE user_id = ?)
         ORDER BY product_id, ts, id",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?)
}

// ------------------------------------------------- added stores, server settings

pub async fn custom_stores(db: &Db) -> Result<Vec<CustomStore>> {
    Ok(sqlx::query_as("SELECT * FROM stores ORDER BY name COLLATE NOCASE, id").fetch_all(db).await?)
}

/// Adds a store, or updates its name and search address when the site is already listed.
pub async fn save_custom_store(db: &Db, retailer: &str, name: &str, search_url: &str, added_by: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO stores (retailer, name, search_url, added_by, created_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (retailer) DO UPDATE SET name = excluded.name, search_url = excluded.search_url",
    )
    .bind(retailer)
    .bind(name)
    .bind(search_url)
    .bind(added_by)
    .bind(now())
    .execute(db)
    .await?;
    Ok(())
}

/// Removes an added store and what was found there for tracked products.
/// Products people track at that store directly are kept.
pub async fn remove_custom_store(db: &Db, id: i64) -> Result<Option<CustomStore>> {
    let row: Option<CustomStore> = sqlx::query_as("SELECT * FROM stores WHERE id = ?").bind(id).fetch_optional(db).await?;
    if let Some(store) = &row {
        sqlx::query("DELETE FROM comparisons WHERE retailer = ?").bind(&store.retailer).execute(db).await?;
        sqlx::query("DELETE FROM stores WHERE id = ?").bind(id).execute(db).await?;
        delete_unused_products(db).await?;
    }
    Ok(row)
}

pub async fn server_setting(db: &Db, key: &str) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT value FROM server_settings WHERE key = ?").bind(key).fetch_optional(db).await?)
}

/// Stores a server-wide setting; an empty value removes it.
pub async fn set_server_setting(db: &Db, key: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        sqlx::query("DELETE FROM server_settings WHERE key = ?").bind(key).execute(db).await?;
    } else {
        sqlx::query(
            "INSERT INTO server_settings (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(db)
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------- discount rules

pub async fn rules(db: &Db, user_id: i64) -> Result<Vec<DiscountRule>> {
    Ok(sqlx::query_as("SELECT * FROM discount_rules WHERE user_id = ? ORDER BY retailer, percent DESC, id")
        .bind(user_id)
        .fetch_all(db)
        .await?)
}

pub async fn add_rule(
    db: &Db,
    user_id: i64,
    retailer: &str,
    name: &str,
    percent: f64,
    brands: &str,
    exclude_keywords: &str,
) -> Result<i64> {
    Ok(sqlx::query(
        "INSERT INTO discount_rules (user_id, retailer, name, percent, brands, exclude_keywords, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(retailer)
    .bind(name)
    .bind(percent)
    .bind(brands)
    .bind(exclude_keywords)
    .bind(now())
    .execute(db)
    .await?
    .last_insert_rowid())
}

// ---------------------------------------------------------- notifications

pub async fn add_notification(
    db: &Db,
    user_id: i64,
    product_id: Option<i64>,
    kind: &str,
    title: &str,
    body: &str,
    price_cents: Option<i64>,
) -> Result<i64> {
    Ok(sqlx::query(
        "INSERT INTO notifications (user_id, product_id, ts, kind, title, body, price_cents) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(product_id)
    .bind(now())
    .bind(kind)
    .bind(title)
    .bind(body)
    .bind(price_cents)
    .execute(db)
    .await?
    .last_insert_rowid())
}

pub async fn notifications(db: &Db, user_id: i64, after_id: i64, limit: i64) -> Result<Vec<Notification>> {
    Ok(sqlx::query_as("SELECT * FROM notifications WHERE user_id = ? AND id > ? ORDER BY id DESC LIMIT ?")
        .bind(user_id)
        .bind(after_id)
        .bind(limit)
        .fetch_all(db)
        .await?)
}

pub async fn unread_count(db: &Db, user_id: i64) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE user_id = ? AND read_at IS NULL")
        .bind(user_id)
        .fetch_one(db)
        .await?)
}

pub async fn mark_all_read(db: &Db, user_id: i64) -> Result<()> {
    sqlx::query("UPDATE notifications SET read_at = ? WHERE user_id = ? AND read_at IS NULL")
        .bind(now())
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

// ------------------------------------------------------------------- chat

pub async fn chat_history(db: &Db, user_id: i64, limit: i64) -> Result<Vec<ChatMessage>> {
    let mut rows: Vec<ChatMessage> =
        sqlx::query_as("SELECT id, ts, role, content FROM chat_messages WHERE user_id = ? ORDER BY id DESC LIMIT ?")
            .bind(user_id)
            .bind(limit)
            .fetch_all(db)
            .await?;
    rows.reverse();
    Ok(rows)
}

pub async fn add_chat_message(db: &Db, user_id: i64, role: &str, content: &str) -> Result<i64> {
    Ok(sqlx::query("INSERT INTO chat_messages (user_id, ts, role, content) VALUES (?, ?, ?, ?)")
        .bind(user_id)
        .bind(now())
        .bind(role)
        .bind(content)
        .execute(db)
        .await?
        .last_insert_rowid())
}
