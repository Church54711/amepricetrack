//! Noticing when a whole store stops being readable.
//!
//! Stores change their pages and tighten what they let robots see. One
//! product failing is shown on that product; when every product at a store
//! has been failing for two days, the people who track things there are told
//! once, and told again when the store reads normally.

use crate::db::{self, Db, DAY};
use crate::{alerts, retailers, App};
use anyhow::Result;
use serde::Serialize;

/// How long every product at a store must have failed before it is announced.
pub const OUTAGE_AFTER: i64 = 2 * DAY;

/// How the products the server keeps up to date at one store are doing.
#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct StoreHealth {
    pub retailer: String,
    /// Products the server checks at this store.
    pub products: i64,
    /// How many of them failed their last check.
    pub failing: i64,
    /// The most recent successful reading of any of them, by the server or a phone.
    pub last_success_at: Option<i64>,
    /// When the oldest of them started being tracked.
    pub first_seen: i64,
    /// The most recent problem, in the store reader's words.
    pub last_error: Option<String>,
}

impl StoreHealth {
    /// Since when nothing at this store could be read, if that is the case now.
    pub fn down_since(&self) -> Option<i64> {
        (self.products > 0 && self.failing == self.products).then(|| self.last_success_at.unwrap_or(self.first_seen))
    }

    /// True once the store has been unreadable for long enough to tell people.
    pub fn unreadable(&self, now: i64) -> bool {
        self.down_since().is_some_and(|since| now - since >= OUTAGE_AFTER)
    }
}

/// One row per store that has products to check.
pub async fn overview(db: &Db) -> Result<Vec<StoreHealth>> {
    Ok(sqlx::query_as(
        "SELECT p.retailer AS retailer, COUNT(*) AS products,
                SUM(CASE WHEN p.fail_count > 0 THEN 1 ELSE 0 END) AS failing,
                MAX(p.last_success_at) AS last_success_at, MIN(p.created_at) AS first_seen,
                (SELECT q.last_error FROM products q WHERE q.retailer = p.retailer AND q.last_error IS NOT NULL
                 ORDER BY q.last_checked_at DESC LIMIT 1) AS last_error
         FROM products p
         WHERE p.id IN (SELECT product_id FROM watches)
            OR p.id IN (SELECT product_id FROM comparisons WHERE status IN ('found', 'possible') AND product_id IS NOT NULL)
         GROUP BY p.retailer",
    )
    .fetch_all(db)
    .await?)
}

fn outage_key(retailer: &str) -> String {
    format!("outage_{retailer}")
}

/// Everyone who tracks something at a store, or compares against it.
async fn people(db: &Db, retailer: &str) -> Result<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT w.user_id FROM watches w JOIN products p ON p.id = w.product_id WHERE p.retailer = ?1
         UNION
         SELECT user_id FROM comparisons WHERE retailer = ?1 AND status = 'found'",
    )
    .bind(retailer)
    .fetch_all(db)
    .await?)
}

/// Announces stores that stopped being readable, and stores that recovered.
pub async fn check(app: &App) -> Result<()> {
    let now = db::now();
    let link = app.cfg.public_url.as_ref().map(|base| format!("{base}/stores")).unwrap_or_default();
    for store in overview(&app.db).await? {
        let key = outage_key(&store.retailer);
        let announced = db::server_setting(&app.db, &key).await?.is_some();
        let name = retailers::display_name(&store.retailer);
        if store.unreadable(now) && !announced {
            db::set_server_setting(&app.db, &key, &now.to_string()).await?;
            let days = (now - store.down_since().unwrap_or(now)) / DAY;
            let title = format!("{name} has not been readable for {days} days");
            let mut body = format!(
                "None of the {} {name} products the server checks could be read, so their prices may be out of date.",
                store.products
            );
            if let Some(error) = &store.last_error {
                body.push_str(&format!(" The last problem: {error}."));
            }
            body.push_str(" The phone app can report these prices in the meantime.");
            tracing::warn!(store = store.retailer, "store unreadable: {title}");
            for user_id in people(&app.db, &store.retailer).await? {
                alerts::notify(app, user_id, None, "store", &title, &body, None, &link).await?;
            }
        } else if store.down_since().is_none() && announced {
            db::set_server_setting(&app.db, &key, "").await?;
            let title = format!("{name} is readable again");
            let body = format!("The server can read {name} prices again. They are being brought up to date.");
            for user_id in people(&app.db, &store.retailer).await? {
                alerts::notify(app, user_id, None, "store", &title, &body, None, &link).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Audience;
    use crate::retailers::Observation;
    use crate::test_app;

    async fn titles(app: &App, user: i64) -> Vec<String> {
        let mut all: Vec<String> = db::notifications(&app.db, user, 0, 50).await.unwrap().into_iter().map(|n| n.title).collect();
        all.reverse();
        all
    }

    #[tokio::test]
    async fn a_store_that_stays_unreadable_is_announced_once_and_again_on_recovery() {
        let app = test_app().await;
        let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        let other = db::create_user(&app.db, "alex", "h", false).await.unwrap();
        sqlx::query("UPDATE user_settings SET auto_compare = 0").execute(&app.db).await.unwrap();
        let chair = crate::service::add_by_url(&app, user, "https://www.staples.com/chair/product_555", None).await.unwrap();
        let desk = crate::service::add_by_url(&app, user, "https://www.staples.com/desk/product_777", None).await.unwrap();
        let phone = crate::service::add_by_url(&app, other, "https://www.walmart.com/ip/123456", None).await.unwrap();
        let price = Observation { title: Some("Thing".into()), price_cents: Some(1000), ..Default::default() };
        for id in [chair, desk, phone] {
            db::record_observation(&app.db, id, &price, "server", Audience::Public, None, Some(60)).await.unwrap();
        }

        // One of two failing: the store still works.
        db::record_failure(&app.db, chair, "the store refused the request (robot check)", 600).await.unwrap();
        check(&app).await.unwrap();
        assert!(titles(&app, user).await.is_empty());

        // Both failing, but only since today: too early to say.
        db::record_failure(&app.db, desk, "the store refused the request (robot check)", 600).await.unwrap();
        check(&app).await.unwrap();
        assert!(titles(&app, user).await.is_empty());

        // Three days without a single reading: announced, once, to the person who tracks things there.
        sqlx::query("UPDATE products SET last_success_at = last_success_at - ? WHERE retailer = 'staples'")
            .bind(3 * DAY)
            .execute(&app.db)
            .await
            .unwrap();
        check(&app).await.unwrap();
        check(&app).await.unwrap();
        assert_eq!(titles(&app, user).await, vec!["Staples has not been readable for 3 days"]);
        let alert = db::notifications(&app.db, user, 0, 50).await.unwrap().remove(0);
        assert!(alert.body.contains("None of the 2 Staples products"), "{}", alert.body);
        assert!(alert.body.contains("robot check"), "{}", alert.body);
        assert!(titles(&app, other).await.is_empty(), "Walmart is fine");

        let staples = overview(&app.db).await.unwrap().into_iter().find(|s| s.retailer == "staples").unwrap();
        assert!(staples.unreadable(db::now()));

        // A phone reports one of them: the store counts as readable again.
        db::record_observation(&app.db, chair, &price, "device", Audience::Public, None, None).await.unwrap();
        check(&app).await.unwrap();
        check(&app).await.unwrap();
        assert_eq!(titles(&app, user).await.last().unwrap(), "Staples is readable again");
        assert_eq!(titles(&app, user).await.len(), 2);
    }
}
