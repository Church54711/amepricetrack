//! Tracking, alerts, discounts and phone reports working together.

use super::*;
use crate::retailers::Observation;
use crate::test_app;

const WALMART: &str = "https://www.walmart.com/ip/AirPods-Max-2-Blue/19864722287";
const CHAIR: &str = "https://www.staples.com/chair/product_555";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn priced(cents: i64) -> Observation {
    Observation { title: Some("Chair".into()), price_cents: Some(cents), in_stock: Some(true), ..Default::default() }
}

/// Records a server reading and runs the alert check, as the scheduler does.
async fn server_reads(app: &App, product_id: i64, cents: i64) {
    let changed =
        db::record_observation(&app.db, product_id, &priced(cents), "server", Audience::Public, None, Some(60)).await.unwrap();
    if changed {
        alerts::evaluate(app, product_id).await.unwrap();
    }
}

/// Alert titles, oldest first.
async fn alert_titles(app: &App, user_id: i64) -> Vec<String> {
    let mut titles: Vec<String> =
        db::notifications(&app.db, user_id, 0, 50).await.unwrap().into_iter().map(|n| n.title).collect();
    titles.reverse();
    titles
}

#[tokio::test]
async fn shared_text_is_accepted_and_bad_links_explained() {
    let app = test_app().await;
    let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
    let shared =
        "Check out this item I found at Walmart! https://www.walmart.com/ip/AirPods-Max-2-Blue/19864722287?athbdg=L1600.";
    let id = add_by_url(&app, user, shared, Some(40000)).await.unwrap();
    let view = watch_view(&app, user, id).await.unwrap().unwrap();
    assert_eq!(view.product.url, "https://www.walmart.com/ip/19864722287");
    assert_eq!(view.watch.target_price_cents, Some(40000));

    // Adding the same product again keeps one entry.
    assert_eq!(add_by_url(&app, user, WALMART, None).await.unwrap(), id);
    assert_eq!(watch_views(&app, user).await.unwrap().len(), 1);

    for bad in ["", "hello there", "https://www.walmart.com/search?q=x", "http://192.168.0.1/x"] {
        assert!(matches!(add_by_url(&app, user, bad, None).await, Err(Problem::User(_))), "{bad:?}");
    }
}

#[tokio::test]
async fn price_changes_produce_the_right_alerts() {
    let app = test_app().await;
    let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
    let id = add_by_url(&app, user, CHAIR, Some(9000)).await.unwrap();

    server_reads(&app, id, 10000).await; // first reading: nothing to say
    server_reads(&app, id, 9500).await; // drop, still above target
    server_reads(&app, id, 9500).await; // unchanged
    server_reads(&app, id, 8800).await; // target reached
    server_reads(&app, id, 9900).await; // back up
    server_reads(&app, id, 8900).await; // reached again: announced again
    assert_eq!(
        alert_titles(&app, user).await,
        vec![
            "Staples: Price drop: $100.00 \u{2192} $95.00",
            "Staples: Hit your target: now $88.00 (target $90.00)",
            "Staples: Hit your target: now $89.00 (target $90.00)",
        ]
    );

    let product = db::product(&app.db, id).await.unwrap().unwrap();
    assert_eq!(product.current_price_cents, Some(8900));
    assert_eq!(product.lowest_price_cents, Some(8800));
    assert_eq!(db::history(&app.db, id, user).await.unwrap().len(), 5, "one row per change");
}

#[tokio::test]
async fn employee_discount_changes_your_price_without_a_false_alert() {
    let app = test_app().await;
    let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
    let other = db::create_user(&app.db, "alex", "h", false).await.unwrap();
    let id = add_by_url(&app, user, CHAIR, None).await.unwrap();
    add_by_url(&app, other, CHAIR, None).await.unwrap();
    server_reads(&app, id, 10000).await;

    db::add_rule(&app.db, user, "staples", "Associate discount", 10.0, "", "").await.unwrap();
    rebaseline(&app, user, "staples").await.unwrap();
    assert!(alert_titles(&app, user).await.is_empty(), "adding a discount is not a price drop");

    let mine = watch_view(&app, user, id).await.unwrap().unwrap();
    assert_eq!(mine.price.your_cents, Some(9000));
    let theirs = watch_view(&app, other, id).await.unwrap().unwrap();
    assert_eq!(theirs.price.your_cents, Some(10000), "discounts are personal");

    server_reads(&app, id, 9000).await;
    assert_eq!(alert_titles(&app, user).await, vec!["Staples: Price drop: $90.00 \u{2192} $81.00"]);
    assert_eq!(alert_titles(&app, other).await, vec!["Staples: Price drop: $100.00 \u{2192} $90.00"]);
}

#[tokio::test]
async fn phone_reports_public_and_signed_in_prices() {
    let app = test_app().await;
    let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
    let page = fixture("walmart.html");

    let untracked = ingest(&app, user, WALMART, &page, false).await;
    assert!(matches!(untracked, Err(Problem::User(_))), "must be on the list first");

    let id = add_by_url(&app, user, WALMART, None).await.unwrap();
    let first = ingest(&app, user, WALMART, &page, true).await.unwrap();
    assert_eq!((first.price_cents, first.recorded_as), (Some(44900), "public"));
    let product = db::product(&app.db, id).await.unwrap().unwrap();
    assert_eq!(product.current_price_cents, Some(44900));
    assert_eq!(product.title.as_deref(), Some("AirPods Max 2 - Blue"));

    // Signed in, the same page shows a lower price: kept as this person's own.
    let member_page = page.replace("\"price\": 449", "\"price\": 399");
    assert_ne!(member_page, page, "the fixture should contain the price to swap");
    let second = ingest(&app, user, WALMART, &member_page, true).await.unwrap();
    assert_eq!((second.price_cents, second.recorded_as), (Some(39900), "member"));
    let view = watch_view(&app, user, id).await.unwrap().unwrap();
    assert_eq!(view.price.public_cents, Some(44900));
    assert_eq!(view.price.member_cents, Some(39900));
    assert_eq!(view.price.your_cents, Some(39900));
    assert_eq!(alert_titles(&app, user).await, vec!["Walmart: Price drop: $449.00 \u{2192} $399.00"]);

    let refusal = "<html><head><title>Access Denied</title></head></html>";
    assert!(matches!(ingest(&app, user, WALMART, refusal, true).await, Err(Problem::User(_))));
}

#[tokio::test]
async fn assistant_is_told_about_prices_and_discounts() {
    let app = test_app().await;
    let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
    let empty = assistant_context(&app, user).await.unwrap();
    assert!(empty.contains("not tracking any products yet"));

    let id = add_by_url(&app, user, CHAIR, Some(8000)).await.unwrap();
    server_reads(&app, id, 10000).await;
    db::add_rule(&app.db, user, "staples", "Associate discount", 10.0, "", "gift card").await.unwrap();
    let context = assistant_context(&app, user).await.unwrap();
    let expected = "Chair | store: Staples | listed now: $100.00 | your price: $90.00 (with Associate discount, 10% off)";
    assert!(context.contains(expected), "{context}");
    assert!(context.contains("target: $80.00"));
    assert!(context.contains("Associate discount at Staples: 10% off everything, except: gift card"));

    // Without a connected AI service the person is told what to do.
    match ask(&app, user, "Best deal?").await {
        Err(Problem::User(message)) => assert!(message.contains("Settings"), "{message}"),
        other => panic!("expected a setup message, got {other:?}"),
    }
}
