//! The background loop that re-checks watched products.

use crate::db::{self, Audience, Product};
use crate::fetch::FetchError;
use crate::{alerts, App};
use std::time::{Duration, Instant};

const HOUR: i64 = 3600;

pub async fn run(app: App) {
    let mut last_cleanup = Instant::now() - Duration::from_secs(3600);
    loop {
        match db::due_products(&app.db, 10).await {
            Ok(due) => {
                for product in due {
                    check_one(&app, &product).await;
                }
            }
            Err(e) => tracing::error!("could not load products to check: {e:#}"),
        }

        if let Some(browser) = app.fetcher.browser() {
            browser.shutdown_if_idle(Duration::from_secs(300)).await;
        }
        if last_cleanup.elapsed() > Duration::from_secs(3600) {
            last_cleanup = Instant::now();
            let expired = sqlx::query("DELETE FROM sessions WHERE expires_at IS NOT NULL AND expires_at < ?")
                .bind(db::now())
                .execute(&app.db)
                .await;
            if let Err(e) = expired {
                tracing::warn!("could not remove expired sign-ins: {e:#}");
            }
            // Tell people when a whole store has stopped being readable.
            if let Err(e) = crate::health::check(&app).await {
                tracing::warn!("could not check the stores' health: {e:#}");
            }
            // Once a day, a copy of the database.
            if let Err(e) = crate::backup::daily(&app).await {
                tracing::warn!("could not look after the backups: {e:#}");
            }
        }

        // Sleep until something is due, or until someone adds a product or presses "check now".
        tokio::select! {
            _ = app.wake.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(30)) => {}
        }
    }
}

/// How long to wait before the next try after `fails` failures in a row.
pub fn retry_delay(error: &FetchError, fails: i64, interval: i64) -> i64 {
    match error {
        // Nothing will change until someone installs a browser or the phone reports it.
        FetchError::NeedsBrowser => interval,
        FetchError::Gone => 24 * HOUR,
        _ => (600 * 2i64.pow(fails.clamp(0, 6) as u32)).min(interval),
    }
}

pub async fn check_one(app: &App, product: &Product) {
    let usual = app.cfg.check_interval.as_secs() as i64;
    // Sooner for a product someone asked to have watched closely.
    let interval = db::check_interval(&app.db, product.id, usual).await.unwrap_or(usual);
    let fetched = app.fetcher.fetch(&product.retailer, &product.retailer_sku, &product.url, product.remembered_mode()).await;
    match fetched {
        Ok(found) => {
            // The page also shows whether a sign-in made in the server's browser still holds.
            if let Some(html) = found.html.as_deref() {
                if let Err(e) = crate::stores::note_reading(app, &product.retailer, found.mode, html).await {
                    tracing::warn!(store = %product.retailer, "could not check the store's sign-in: {e}");
                }
            }
            // Spread checks out a little so they do not all land on the same minute.
            let next = (interval as f64 * rand::random_range(0.9..1.1)) as i64;
            let recorded =
                db::record_observation(&app.db, product.id, &found.obs, "server", Audience::Public, Some(found.mode), Some(next))
                    .await;
            // A product read for the first time can now be looked up at other stores.
            if product.title.is_none() {
                app.compare_wake.notify_one();
            }
            match recorded {
                Ok(true) => {
                    tracing::info!(
                        product = product.id,
                        store = %product.retailer,
                        price = ?found.obs.price_cents,
                        "price or stock changed"
                    );
                    if let Err(e) = alerts::evaluate(app, product.id).await {
                        tracing::error!("could not evaluate alerts for product {}: {e:#}", product.id);
                    }
                    if let Err(e) = crate::compare::tracking::price_changed(app, product.id).await {
                        tracing::error!("could not compare stores for product {}: {e:#}", product.id);
                    }
                }
                Ok(false) => {}
                Err(e) => tracing::error!("could not save the price of product {}: {e:#}", product.id),
            }
        }
        Err(error) => {
            let delay = retry_delay(&error, product.fail_count, interval);
            tracing::warn!(product = product.id, store = %product.retailer, "check failed: {error}");
            if let Err(e) = db::record_failure(&app.db, product.id, &error.to_string(), delay).await {
                tracing::error!("could not record the failed check of product {}: {e:#}", product.id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_back_off_up_to_the_normal_interval() {
        let interval = 4 * HOUR;
        let blocked = FetchError::Blocked("HTTP 403".into());
        assert_eq!(retry_delay(&blocked, 0, interval), 600);
        assert_eq!(retry_delay(&blocked, 2, interval), 2400);
        assert_eq!(retry_delay(&blocked, 20, interval), interval);
        assert_eq!(retry_delay(&FetchError::NeedsBrowser, 0, interval), interval);
        assert_eq!(retry_delay(&FetchError::Gone, 0, interval), 24 * HOUR);
    }
}
