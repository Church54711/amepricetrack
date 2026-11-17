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

#[tokio::test]
async fn a_phone_reading_is_only_shared_when_nobody_else_relies_on_the_product() {
    let app = test_app().await;
    let admin = db::create_user(&app.db, "sam", "h", true).await.unwrap();
    let member = db::create_user(&app.db, "alex", "h", false).await.unwrap();
    sqlx::query("UPDATE user_settings SET auto_compare = 0").execute(&app.db).await.unwrap();
    let id = add_by_url(&app, admin, CHAIR, Some(5000)).await.unwrap();
    add_by_url(&app, member, CHAIR, None).await.unwrap();
    let page = |price: &str| {
        format!(
            r#"<html><head><title>Made up</title><script type="application/ld+json">
            {{"@type":"Product","name":"Chair of lies","offers":{{"price":"{price}","availability":"https://schema.org/InStock"}}}}
            </script></head></html>"#
        )
    };
    // While the server cannot read the product, a phone's reading is all there is, and it is shared.
    let fallback = ingest(&app, member, CHAIR, &page("95.00"), false).await.unwrap();
    assert_eq!(fallback.recorded_as, "public");

    // Once the server reads it itself, a made-up page from someone who shares the product
    // changes nothing for the other person, and does not put off the server's next check.
    let real = Observation { title: Some("Office Chair".into()), price_cents: Some(8999), ..Default::default() };
    let by_browser = Some(crate::retailers::FetchMode::Browser);
    db::record_observation(&app.db, id, &real, "server", Audience::Public, by_browser, Some(60)).await.unwrap();
    alerts::evaluate(&app, id).await.unwrap();
    let alerts_before = db::notifications(&app.db, admin, 0, 10).await.unwrap().len();
    let due = db::product(&app.db, id).await.unwrap().unwrap().next_check_at;
    let theirs = ingest(&app, member, CHAIR, &page("1.00"), false).await.unwrap();
    assert_eq!(theirs.recorded_as, "member");
    ingest(&app, member, CHAIR, &page("89.99"), false).await.unwrap();
    assert_eq!(db::product(&app.db, id).await.unwrap().unwrap().next_check_at, due, "the same price again moves nothing");
    let product = db::product(&app.db, id).await.unwrap().unwrap();
    assert_eq!((product.current_price_cents, product.title.as_deref()), (Some(8999), Some("Office Chair")));
    assert_eq!(db::notifications(&app.db, admin, 0, 10).await.unwrap().len(), alerts_before, "no false target alert");
    // The sender sees the lower of the two, which here is their own claim; nobody else does.
    assert_eq!(watch_view(&app, admin, id).await.unwrap().unwrap().price.your_cents, Some(8999));

    // A reading from the administrator's phone in between does not change that.
    assert_eq!(ingest(&app, admin, CHAIR, &page("88.00"), false).await.unwrap().recorded_as, "public");
    assert_eq!(ingest(&app, member, CHAIR, &page("2.00"), false).await.unwrap().recorded_as, "member");
    assert_eq!(db::product(&app.db, id).await.unwrap().unwrap().current_price_cents, Some(8800));

    // The server stops being able to read it: the phone's readings are shared again, one after another.
    db::record_failure(&app.db, id, "the store refused the request (robot check)", 600).await.unwrap();
    for price in ["85.00", "84.00"] {
        assert_eq!(ingest(&app, member, CHAIR, &page(price), false).await.unwrap().recorded_as, "public", "{price}");
    }
    assert_eq!(db::product(&app.db, id).await.unwrap().unwrap().current_price_cents, Some(8400));

    // The administrator's phone is taken at its word, as before.
    let ours = ingest(&app, admin, CHAIR, &page("79.99"), false).await.unwrap();
    assert_eq!(ours.recorded_as, "public");
    assert_eq!(db::product(&app.db, id).await.unwrap().unwrap().current_price_cents, Some(7999));
}

#[tokio::test]
async fn a_new_delivery_charge_counts_as_a_change_even_at_the_same_price() {
    let app = test_app().await;
    let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
    sqlx::query("UPDATE user_settings SET auto_compare = 0").execute(&app.db).await.unwrap();
    let id = add_by_url(&app, user, WALMART, None).await.unwrap();
    let free = Observation { price_cents: Some(4000), ships_free: Some(true), ..Default::default() };
    let charged =
        Observation { price_cents: Some(4000), ships_free: Some(false), shipping_cents: Some(699), ..Default::default() };
    assert!(db::record_observation(&app.db, id, &free, "server", Audience::Public, None, None).await.unwrap());
    assert!(!db::record_observation(&app.db, id, &free, "server", Audience::Public, None, None).await.unwrap(), "nothing new");
    assert!(db::record_observation(&app.db, id, &charged, "server", Audience::Public, None, None).await.unwrap());
}

#[tokio::test]
async fn saving_other_settings_does_not_announce_a_met_target_again() {
    let app = test_app().await;
    let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
    sqlx::query("UPDATE user_settings SET auto_compare = 0").execute(&app.db).await.unwrap();
    let id = add_by_url(&app, user, "https://www.staples.com/chair/product_555", None).await.unwrap();
    let price = Observation { title: Some("Chair".into()), price_cents: Some(4000), ..Default::default() };
    db::record_observation(&app.db, id, &price, "server", Audience::Public, None, None).await.unwrap();
    alerts::evaluate(&app, id).await.unwrap();
    let alerts_so_far = || async { db::notifications(&app.db, user, 0, 20).await.unwrap().len() };

    let edit = |target: i64, note: &str, pace: Option<i64>| db::WatchEdit {
        target_price_cents: Some(target),
        drop_percent: None,
        notify_any_drop: true,
        notify_back_in_stock: false,
        note: Some(note.to_string()),
        check_minutes: pace,
    };
    // A target the price already meets is announced once.
    db::update_watch(&app.db, user, id, &edit(4500, "", None)).await.unwrap();
    alerts::evaluate(&app, id).await.unwrap();
    assert_eq!(alerts_so_far().await, 1);
    // A note, or watching it closely, changes nothing about the target.
    db::update_watch(&app.db, user, id, &edit(4500, "for the office", Some(60))).await.unwrap();
    alerts::evaluate(&app, id).await.unwrap();
    assert_eq!(alerts_so_far().await, 1, "no second alert for the same target");
    // A different target that is also met is news again.
    db::update_watch(&app.db, user, id, &edit(4200, "for the office", Some(60))).await.unwrap();
    alerts::evaluate(&app, id).await.unwrap();
    assert_eq!(alerts_so_far().await, 2);
}
