//! The website and API, used the way a browser and the phone app use them.

use crate::test_app;
use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn site() -> Router {
    super::site(test_app().await)
}

/// The site together with the app behind it, for tests that prepare data directly.
async fn site_and_app() -> (Router, crate::App) {
    let app = test_app().await;
    (super::site(app.clone()), app)
}

async fn send(site: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let response = site.clone().oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (parts.status, parts.headers, String::from_utf8_lossy(&bytes).into_owned())
}

fn get(path: &str, cookie: &str) -> Request<Body> {
    Request::get(path).header(header::COOKIE, cookie).body(Body::empty()).unwrap()
}

fn get_api(path: &str, token: &str) -> Request<Body> {
    Request::get(path).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap()
}

fn post_form(path: &str, cookie: &str, body: &str) -> Request<Body> {
    Request::post(path)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn post_json(path: &str, token: &str, body: Value) -> Request<Body> {
    Request::post(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn location(headers: &HeaderMap) -> &str {
    headers[header::LOCATION].to_str().unwrap()
}

fn cookie_from(headers: &HeaderMap) -> String {
    headers[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string()
}

/// Creates the first account and returns its session cookie.
async fn set_up(site: &Router) -> String {
    let (status, headers, _) = send(site, post_form("/setup", "", "username=sam&password=longenough")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    cookie_from(&headers)
}

const ADD_CHAIR: &str = "url=https%3A%2F%2Fwww.staples.com%2Fchair%2Fproduct_555&target=45";

#[tokio::test]
async fn new_server_walks_through_setup_then_requires_sign_in() {
    let site = site().await;
    let (status, headers, _) = send(&site, get("/", "")).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/setup"));

    let (status, _, body) = send(&site, post_form("/setup", "", "username=sam&password=short")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("at least 8 characters"));

    let cookie = set_up(&site).await;
    let (status, headers, body) = send(&site, get("/", &cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Nothing tracked yet"));
    assert!(headers["content-security-policy"].to_str().unwrap().contains("script-src 'self'"));

    // Setup is closed once an account exists, and pages need the cookie.
    let (_, headers, _) = send(&site, post_form("/setup", "", "username=eve&password=longenough")).await;
    assert_eq!(location(&headers), "/login");
    let (_, headers, _) = send(&site, get("/settings", "")).await;
    assert_eq!(location(&headers), "/login");

    let (status, _, body) = send(&site, post_form("/login", "", "username=sam&password=wrongwrong")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("not right"));
    let (status, _, _) = send(&site, post_form("/login", "", "username=SAM&password=longenough")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn every_page_renders_for_a_signed_in_person() {
    let site = site().await;
    let cookie = set_up(&site).await;
    let (status, headers, _) = send(&site, post_form("/watches", &cookie, ADD_CHAIR)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/products/1?msg="));
    send(&site, post_form("/discounts/preset/staples", &cookie, "")).await;

    for (path, expected) in [
        ("/", "product_555"),
        ("/?store=staples", "Checking"),
        ("/products/1", "Alert me when"),
        ("/deals", "No deals right now"),
        ("/alerts", "No alerts yet"),
        ("/discounts", "Associate discount, Staples brands"),
        ("/assistant", "No AI service is connected yet"),
        ("/settings", "Phone app"),
        ("/static/app.css", "--accent"),
        ("/static/app.js", "use strict"),
        ("/favicon.svg", "<svg"),
    ] {
        let (status, _, body) = send(&site, get(path, &cookie)).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(body.contains(expected), "{path} should mention {expected:?}");
    }
    let (status, _, _) = send(&site, get("/products/999", &cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A link that is not a product page is refused with the reason.
    let (status, headers, _) =
        send(&site, post_form("/watches", &cookie, "url=https%3A%2F%2Fwww.walmart.com%2Fsearch%3Fq%3Dchair")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/?err="));
}

#[tokio::test]
async fn theme_defaults_to_oled_gold_and_can_be_changed() {
    let site = site().await;
    // Before sign-in, and for a new account, the default theme is used.
    let (_, _, body) = send(&site, get("/setup", "")).await;
    assert!(body.contains(r#"data-theme="oled-gold""#));
    let cookie = set_up(&site).await;
    let (_, _, body) = send(&site, get("/", &cookie)).await;
    assert!(body.contains(r#"data-theme="oled-gold""#));
    assert!(!body.contains("/theme.css"), "no custom accent yet");

    let form = "theme=light&use_accent=on&accent=%23FF0080";
    let (status, _, _) = send(&site, post_form("/settings/appearance", &cookie, form)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, _, body) = send(&site, get("/", &cookie)).await;
    assert!(body.contains(r#"data-theme="light""#));
    assert!(body.contains(r#"href="/theme.css?c=ff0080""#));
    let (_, _, css) = send(&site, get("/theme.css?c=ff0080", "")).await;
    assert_eq!(css.trim(), "html:root[data-theme] { --accent: #ff0080; --accent-text: #000000; }");
    let (_, _, css) = send(&site, get("/theme.css?c=001a66", "")).await;
    assert!(css.contains("--accent-text: #ffffff"), "white text on a dark accent");
    let (_, _, css) = send(&site, get("/theme.css?c=red;}body{display:none", "")).await;
    assert_eq!(css, "", "anything that is not a plain colour is ignored");

    // Unknown themes fall back to the default; unticking the box drops the accent.
    send(&site, post_form("/settings/appearance", &cookie, "theme=nonsense&accent=%23ff0080")).await;
    let (_, _, body) = send(&site, get("/", &cookie)).await;
    assert!(body.contains(r#"data-theme="oled-gold""#));
    assert!(!body.contains("/theme.css"));
}

#[tokio::test]
async fn another_person_cannot_see_or_change_my_products() {
    let site = site().await;
    let admin = set_up(&site).await;
    send(&site, post_form("/watches", &admin, ADD_CHAIR)).await;
    send(&site, post_form("/settings/users", &admin, "username=alex&password=longenough")).await;
    let (_, headers, _) = send(&site, post_form("/login", "", "username=alex&password=longenough")).await;
    let alex = cookie_from(&headers);

    let (status, _, _) = send(&site, get("/products/1", &alex)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(&site, post_form("/products/1/watch", &alex, "target=1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    send(&site, post_form("/products/1/remove", &alex, "")).await;
    let (status, _, _) = send(&site, get("/products/1", &admin)).await;
    assert_eq!(status, StatusCode::OK, "the owner's product is untouched");

    // Only an administrator may add people.
    let (status, _, _) = send(&site, post_form("/settings/users", &alex, "username=eve&password=longenough")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn phone_api_signs_in_tracks_and_reports() {
    let site = site().await;
    set_up(&site).await;

    let (status, _, body) = send(&site, get("/api/v1/watches", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("error"));
    let wrong = json!({ "username": "sam", "password": "nope-nope" });
    let (status, _, _) = send(&site, post_json("/api/v1/login", "", wrong)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let login = json!({ "username": "sam", "password": "longenough", "device_name": "Pixel" });
    let (status, _, body) = send(&site, post_json("/api/v1/login", "", login)).await;
    assert_eq!(status, StatusCode::OK);
    let token = serde_json::from_str::<Value>(&body).unwrap()["token"].as_str().unwrap().to_string();

    let add = json!({ "url": "https://www.walmart.com/ip/AirPods-Max-2-Blue/19864722287", "target_price_cents": 40000 });
    let (status, _, body) = send(&site, post_json("/api/v1/watches", &token, add)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let added: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(added["store_name"], "Walmart");
    assert_eq!(added["watch"]["target_price_cents"], 40000);

    let page = std::fs::read_to_string(format!("{}/tests/fixtures/walmart.html", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let report = json!({ "url": "https://www.walmart.com/ip/19864722287", "html": page, "signed_in": false });
    let (status, _, body) = send(&site, post_json("/api/v1/ingest", &token, report)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"price_cents\":44900"), "{body}");

    let (_, _, body) = send(&site, get_api("/api/v1/products/1", &token)).await;
    let product: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(product["price"]["your_cents"], 44900);
    assert_eq!(product["history"].as_array().unwrap().len(), 1);
    let (_, _, body) = send(&site, get_api("/api/v1/collect", &token)).await;
    assert!(body.contains("\"optional\":[{"), "{body}");
    let (status, _, _) = send(&site, get_api("/api/v1/products/77", &token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Signing out makes the token useless.
    send(&site, post_json("/api/v1/logout", &token, json!({}))).await;
    let (status, _, _) = send(&site, get_api("/api/v1/watches", &token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn stores_page_adds_and_removes_a_store_and_checks_the_zip_code() {
    let site = site().await;
    let cookie = set_up(&site).await;
    let (status, _, body) = send(&site, get("/stores", &cookie)).await;
    assert_eq!(status, StatusCode::OK);
    for expected in ["Hot Topic", "Harbor Freight", "Your area", "Add a store", "the server's default"] {
        assert!(body.contains(expected), "{expected}");
    }

    // The search address is given as a results page for the test word.
    let form = "site=www.boxlunch.example&name=BoxLunch&search=https%3A%2F%2Fwww.boxlunch.example%2Fsearch%3Fq%3Dpricetrack";
    let (status, headers, _) = send(&site, post_form("/stores/add", &cookie, form)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/stores?msg=BoxLunch+added"), "{}", location(&headers));
    let (_, _, body) = send(&site, get("/stores", &cookie)).await;
    assert!(body.contains("https://www.boxlunch.example/search?q={q}"));

    // A product tracked afterwards is looked up there too.
    send(&site, post_form("/watches", &cookie, ADD_CHAIR)).await;
    let (_, _, body) = send(&site, get("/products/1", &cookie)).await;
    assert!(body.contains("BoxLunch"), "the added store is in the Other stores card");

    // A built-in store or an address on the local network is refused with a reason.
    let (_, headers, _) = send(&site, post_form("/stores/add", &cookie, "site=walmart.com")).await;
    assert!(location(&headers).contains("err=Walmart+is+already+supported"), "{}", location(&headers));
    assert!(
        location(&headers).starts_with("/stores?err=") && location(&headers).ends_with("#add"),
        "the message comes before the page position"
    );
    let (_, headers, _) = send(&site, post_form("/stores/add", &cookie, "site=http%3A%2F%2F10.0.0.5")).await;
    assert!(location(&headers).contains("err=Only+public+store+websites"), "{}", location(&headers));

    let (_, headers, _) = send(&site, post_form("/stores/1/remove", &cookie, "")).await;
    assert!(location(&headers).starts_with("/stores?msg=BoxLunch+removed"));
    let (_, _, body) = send(&site, get("/products/1", &cookie)).await;
    assert!(!body.contains("BoxLunch"));

    // A ZIP code has to look like one; an empty one clears the location.
    let (_, headers, _) = send(&site, post_form("/stores/location", &cookie, "zip=123+Main+St")).await;
    assert!(location(&headers).contains("err=Type+a+five-digit+US+ZIP+code"), "{}", location(&headers));
    let (_, headers, _) = send(&site, post_form("/stores/location", &cookie, "zip=")).await;
    assert!(location(&headers).starts_with("/stores?msg=Location+cleared"), "{}", location(&headers));
}

#[tokio::test]
async fn matching_model_is_saved_with_the_ai_settings() {
    let site = site().await;
    let cookie = set_up(&site).await;
    let form = "provider=claude&api_key=sk-test&base_url=&model=claude-opus-5-5&match_model=claude-haiku-5-5";
    let (_, headers, _) = send(&site, post_form("/settings/ai", &cookie, form)).await;
    assert!(location(&headers).starts_with("/settings?msg=AI+settings+saved"), "{}", location(&headers));
    let (_, _, body) = send(&site, get("/settings", &cookie)).await;
    assert!(body.contains(r#"name="match_model" value="claude-haiku-5-5""#), "the matching model is shown back");
    assert!(body.contains("a good choice here: claude-haiku-5-5"));
    assert!(!body.contains("sk-test"), "the key is never shown back");
}

#[tokio::test]
async fn a_name_or_barcode_typed_instead_of_a_link_goes_to_find() {
    let site = site().await;
    let cookie = set_up(&site).await;

    // Words in the add box are searched for, not treated as a broken link.
    let (status, headers, _) = send(&site, post_form("/watches", &cookie, "url=hammermill+copy+paper")).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/find?q=hammermill+copy+paper"));
    let (status, _, body) = send(&site, get("/find?q=hammermill+copy+paper", &cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Find a product") && body.contains("Stores searched for"), "{body}");
    for store in ["Staples", "Walmart", "Hot Topic"] {
        assert!(body.contains(&format!("<h2>{store}</h2>")), "{store} is listed");
    }
    assert!(body.contains(r#"data-store="walmart""#) && body.contains("find-waiting"));

    // A scanned barcode is recognised and written the way stores list it.
    let (_, _, body) = send(&site, get("/find?q=0195951039305", &cookie)).await;
    assert!(body.contains("Stores searched for the barcode") && body.contains(r#"data-query="195951039305""#), "{body}");

    // Too short to search for: back to the watchlist with the reason.
    let (status, headers, _) = send(&site, get("/find?q=ab", &cookie)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/?err="));
    let (status, _, body) = send(&site, get("/find/store?q=copy+paper&store=nowhere", &cookie)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("cannot be searched"));

    // The phone app starts a find the same way.
    let login = json!({ "username": "sam", "password": "longenough" });
    let (_, _, body) = send(&site, post_json("/api/v1/login", "", login)).await;
    let token = serde_json::from_str::<Value>(&body).unwrap()["token"].as_str().unwrap().to_string();
    let (status, _, body) = send(&site, get_api("/api/v1/find?q=0195951039305", &token)).await;
    let start: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!((start["query"].as_str(), start["barcode"].as_bool()), (Some("195951039305"), Some(true)));
    assert_eq!(start["stores"][0]["id"], "staples");
    assert_eq!(start["tracked"], json!([]));
}

#[tokio::test]
async fn price_match_card_shows_both_sides_and_what_to_check() {
    use crate::db::{self, Audience};
    use crate::retailers::{Observation, ProductRef};

    let (site, app) = site_and_app().await;
    let cookie = set_up(&site).await;
    send(&site, post_form("/watches", &cookie, ADD_CHAIR)).await;
    let chair =
        Observation { title: Some("Office Chair".into()), price_cents: Some(10000), in_stock: Some(true), ..Default::default() };
    db::record_observation(&app.db, 1, &chair, "server", Audience::Public, None, Some(60)).await.unwrap();

    // The same chair at Walmart for $90, from a marketplace seller.
    let reference = ProductRef { retailer: "walmart".into(), sku: "777".into(), url: "https://www.walmart.com/ip/777".into() };
    let other = db::upsert_product(&app.db, &reference).await.unwrap();
    let offer = Observation {
        title: Some("Office Chair, Black".into()),
        price_cents: Some(9000),
        in_stock: Some(true),
        seller: Some("Seats Direct".into()),
        third_party: Some(true),
        ..Default::default()
    };
    db::record_observation(&app.db, other, &offer, "server", Audience::Public, None, Some(60)).await.unwrap();
    sqlx::query(
        "UPDATE comparisons SET status = 'found', product_id = ?, score = 100, reason = 'Same barcode'
         WHERE source_product_id = 1 AND retailer = 'walmart'",
    )
    .bind(other)
    .execute(&app.db)
    .await
    .unwrap();
    let id: i64 = sqlx::query_scalar("SELECT id FROM comparisons WHERE retailer = 'walmart'").fetch_one(&app.db).await.unwrap();

    // The product page offers the card for the store that is cheaper.
    let (_, _, body) = send(&site, get("/products/1", &cookie)).await;
    assert!(body.contains(&format!(r#"href="/comparisons/{id}/match""#)), "{body}");
    assert!(body.contains("Marketplace seller: Seats Direct"));

    let (status, _, body) = send(&site, get(&format!("/comparisons/{id}/match"), &cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Show this at Staples when asking them to match the price at Walmart."), "{body}");
    assert!(body.contains("$10.00 lower at Walmart"));
    assert!(body.contains("Sold by Seats Direct, a marketplace seller"));
    assert!(body.contains("Matched by: Same barcode"));
    assert!(body.contains(r#"href="https://www.walmart.com/ip/777""#));

    // "Check again" queues both products for an immediate check.
    sqlx::query("UPDATE products SET next_check_at = 9999999999").execute(&app.db).await.unwrap();
    let (status, headers, _) = send(&site, post_form(&format!("/comparisons/{id}/refresh"), &cookie, "")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with(&format!("/comparisons/{id}/match?msg=")));
    let due: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM products WHERE next_check_at = 0").fetch_one(&app.db).await.unwrap();
    assert_eq!(due, 2);

    // A store that has not been searched, or someone else's comparison, has no card.
    let (status, _, _) = send(&site, get("/comparisons/9999/match", &cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let pending: i64 =
        sqlx::query_scalar("SELECT id FROM comparisons WHERE retailer = 'target'").fetch_one(&app.db).await.unwrap();
    let (status, _, _) = send(&site, get(&format!("/comparisons/{pending}/match"), &cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_membership_with_free_delivery_needs_no_percent() {
    let site = site().await;
    let cookie = set_up(&site).await;
    let (_, headers, _) =
        send(&site, post_form("/discounts", &cookie, "retailer=amazon&name=Prime&percent=&free_shipping=on")).await;
    assert!(location(&headers).starts_with("/discounts?msg="), "{}", location(&headers));
    let (_, _, body) = send(&site, get("/discounts", &cookie)).await;
    assert!(body.contains(r#"value="Prime""#) && body.contains("Free delivery on what Amazon ships itself"), "{body}");

    // Neither a percent nor free delivery is nothing to add.
    let (_, headers, _) = send(&site, post_form("/discounts", &cookie, "retailer=amazon&name=Nothing&percent=")).await;
    assert!(location(&headers).starts_with("/discounts?err="));

    // The setting that leaves marketplace sellers out is saved.
    send(&site, post_form("/settings/compare", &cookie, "auto_compare=on&ignore_marketplace=on")).await;
    let (_, _, body) = send(&site, get("/settings", &cookie)).await;
    assert!(body.contains(r#"name="ignore_marketplace" checked"#), "the box stays ticked");
}

#[tokio::test]
async fn what_was_set_up_in_the_servers_browser_is_saved_and_shown() {
    let (site, app) = site_and_app().await;
    let cookie = set_up(&site).await;

    // This test server has no browser, so the page says so instead of opening one.
    let (status, headers, _) = send(&site, get("/stores/browser?store=walmart", &cookie)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).contains("no+browser+set+up"), "{}", location(&headers));
    let (status, _, _) = send(&site, get("/stores/browser/screen", &cookie)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Saying "I am signed in, read it with the browser" is remembered per store.
    let (_, headers, _) =
        send(&site, post_form("/stores/browser/done", &cookie, "store=walmart&signed_in=on&read_with_browser=on")).await;
    assert!(location(&headers).starts_with("/stores?msg="), "{}", location(&headers));
    let setup = crate::stores::browser_setup(&app.db, "walmart").await.unwrap();
    assert_eq!((setup.signed_in_by.as_deref(), setup.read_with_browser), (Some("sam"), true));
    let (_, _, body) = send(&site, get("/stores", &cookie)).await;
    assert!(body.contains("Signed in by sam"), "{body}");
    assert!(body.contains("for everyone on this server"));

    // Unticking both forgets it.
    send(&site, post_form("/stores/browser/done", &cookie, "store=walmart")).await;
    assert_eq!(crate::stores::browser_setup(&app.db, "walmart").await.unwrap(), crate::stores::BrowserSetup::default());
    let (_, _, body) = send(&site, get("/stores", &cookie)).await;
    assert!(!body.contains("Signed in by sam"));

    // Someone who is not the administrator cannot use the browser or change this.
    send(&site, post_form("/settings/users", &cookie, "username=alex&password=longenough")).await;
    let (_, headers, _) = send(&site, post_form("/login", "", "username=alex&password=longenough")).await;
    let other = cookie_from(&headers);
    let (_, headers, _) = send(&site, post_form("/stores/browser/done", &other, "store=walmart&signed_in=on")).await;
    assert!(location(&headers).starts_with("/stores?err="), "{}", location(&headers));
    assert_eq!(crate::stores::browser_setup(&app.db, "walmart").await.unwrap().signed_in_by, None);
    let (status, _, _) = send(&site, get("/stores/browser/screen", &other)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_page_sent_compressed_by_the_phone_is_read() {
    use std::io::Write;

    let site = site().await;
    set_up(&site).await;
    let login = json!({ "username": "sam", "password": "longenough" });
    let (_, _, body) = send(&site, post_json("/api/v1/login", "", login)).await;
    let token = serde_json::from_str::<Value>(&body).unwrap()["token"].as_str().unwrap().to_string();
    let add = json!({ "url": "https://www.staples.com/chair/product_555" });
    send(&site, post_json("/api/v1/watches", &token, add)).await;

    let page = r#"<html><head><script type="application/ld+json">
        {"@type":"Product","name":"Office Chair","offers":{"price":"89.99","availability":"https://schema.org/InStock"}}
        </script></head><body></body></html>"#;
    let report = json!({ "url": "https://www.staples.com/chair/product_555", "html": page, "signed_in": false }).to_string();
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(report.as_bytes()).unwrap();
    let request = Request::post("/api/v1/ingest")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_ENCODING, "gzip")
        .body(Body::from(gzip.finish().unwrap()))
        .unwrap();
    let (status, _, body) = send(&site, request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["price_cents"], 8999);
}

#[tokio::test]
async fn a_product_can_be_watched_closely() {
    use crate::db;
    const FOUR_HOURS: i64 = 4 * 3600;

    let (site, app) = site_and_app().await;
    let cookie = set_up(&site).await;
    send(&site, post_form("/watches", &cookie, ADD_CHAIR)).await;
    assert_eq!(db::check_interval(&app.db, 1, FOUR_HOURS).await.unwrap(), FOUR_HOURS, "the usual pace to begin with");
    let (_, _, body) = send(&site, get("/products/1", &cookie)).await;
    assert!(body.contains("At the usual pace (about every 4 hours)"), "the choice is offered");

    // The same product found at another store is checked as often as the tracked one.
    let elsewhere = crate::retailers::identify("https://www.walmart.com/ip/123456").unwrap();
    let match_id = db::upsert_product(&app.db, &elsewhere).await.unwrap();
    sqlx::query("UPDATE comparisons SET status = 'found', product_id = ? WHERE source_product_id = 1 AND retailer = 'walmart'")
        .bind(match_id)
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query("UPDATE products SET next_check_at = ?").bind(db::now() + FOUR_HOURS).execute(&app.db).await.unwrap();

    let (_, headers, _) = send(&site, post_form("/products/1/watch", &cookie, "target=45&any_drop=on&check_minutes=60")).await;
    assert!(location(&headers).starts_with("/products/1?msg="), "{}", location(&headers));
    assert_eq!(db::check_interval(&app.db, 1, FOUR_HOURS).await.unwrap(), 3600);
    assert_eq!(db::check_interval(&app.db, match_id, FOUR_HOURS).await.unwrap(), 3600);
    // Both are due within the hour now, not in four.
    let latest: i64 = sqlx::query_scalar("SELECT MAX(next_check_at) FROM products").fetch_one(&app.db).await.unwrap();
    assert!(latest <= db::now() + 3600, "next check in {} seconds", latest - db::now());
    let (_, _, body) = send(&site, get("/products/1", &cookie)).await;
    assert!(body.contains(r#"<option value="60" selected>Every hour"#), "the choice is shown as saved");
    let (_, _, body) = send(&site, get("/", &cookie)).await;
    assert!(body.contains("Watched closely: every hour"), "the watchlist says so");

    // The phone app sets it the same way; nonsense is refused, and leaving it out goes back to the usual pace.
    let login = json!({ "username": "sam", "password": "longenough" });
    let (_, _, body) = send(&site, post_json("/api/v1/login", "", login)).await;
    let token = serde_json::from_str::<Value>(&body).unwrap()["token"].as_str().unwrap().to_string();
    let (status, _, body) = send(&site, post_json("/api/v1/products/1/watch", &token, json!({ "check_minutes": 5 }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _, body) = send(&site, post_json("/api/v1/products/1/watch", &token, json!({ "check_minutes": 30 }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["watch"]["check_minutes"], 30);
    assert_eq!(db::check_interval(&app.db, match_id, FOUR_HOURS).await.unwrap(), 1800);
    send(&site, post_json("/api/v1/products/1/watch", &token, json!({}))).await;
    assert_eq!(db::check_interval(&app.db, 1, FOUR_HOURS).await.unwrap(), FOUR_HOURS);
    let (_, _, body) = send(&site, get_api("/api/v1/me", &token)).await;
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["check_interval_minutes"], 240);
}

#[tokio::test]
async fn the_phone_app_can_set_up_pushed_alerts() {
    let site = site().await;
    let cookie = set_up(&site).await;
    let login = json!({ "username": "sam", "password": "longenough" });
    let (_, _, body) = send(&site, post_json("/api/v1/login", "", login)).await;
    let token = serde_json::from_str::<Value>(&body).unwrap()["token"].as_str().unwrap().to_string();

    let (_, _, body) = send(&site, get_api("/api/v1/push", &token)).await;
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), json!({ "ntfy_url": "", "webhook": false }));
    let (status, _, body) = send(&site, post_json("/api/v1/push/test", &token, json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("Add an ntfy or webhook address first"), "{body}");

    // An address needs to be a web address with a topic name.
    for bad in ["ntfy.sh/my-topic", "https://ntfy.sh/", "https://ntfy.sh/a/b"] {
        let (status, _, body) = send(&site, post_json("/api/v1/push", &token, json!({ "ntfy_url": bad }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    // Nothing listens on this port, so the test alert fails at once without leaving this machine.
    let topic = "http://127.0.0.1:9/apt-made-up-topic";
    let (status, _, body) = send(&site, post_json("/api/v1/push", &token, json!({ "ntfy_url": topic }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, _, body) = send(&site, get_api("/api/v1/push", &token)).await;
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["ntfy_url"], topic);
    let (_, _, body) = send(&site, get("/settings", &cookie)).await;
    assert!(body.contains(topic), "the website shows the same address");
    let (status, _, body) = send(&site, post_json("/api/v1/push/test", &token, json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("could not be delivered"), "{body}");

    let (_, _, body) = send(&site, post_json("/api/v1/push", &token, json!({ "ntfy_url": "" }))).await;
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["ntfy_url"], "");
}

#[tokio::test]
async fn only_the_administrator_can_ask_for_a_backup() {
    let site = site().await;
    let admin = set_up(&site).await;
    let (_, _, body) = send(&site, get("/settings", &admin)).await;
    assert!(body.contains("Back up now") && body.contains("Backups"));
    send(&site, post_form("/settings/users", &admin, "username=alex&password=longenough")).await;
    let (_, headers, _) = send(&site, post_form("/login", "", "username=alex&password=longenough")).await;
    let other = cookie_from(&headers);
    let (_, _, body) = send(&site, get("/settings", &other)).await;
    assert!(!body.contains("Back up now"));
    let (_, headers, _) = send(&site, post_form("/settings/backup", &other, "")).await;
    assert!(location(&headers).contains("err=Only+the+administrator"), "{}", location(&headers));
}

#[tokio::test]
async fn a_form_posted_by_another_website_is_refused() {
    let site = site().await;
    let cookie = set_up(&site).await;
    let post_from = |origin: &'static str, host: &'static str| {
        Request::post("/settings/users")
            .header(header::COOKIE, &cookie)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::HOST, host)
            .header(header::ORIGIN, origin)
            .body(Body::from("username=intruder&password=longenough"))
            .unwrap()
    };
    // Another service on the same machine, a page that hides where it is from, another site altogether.
    for elsewhere in ["http://192.168.1.20:8080", "null", "https://evil.example", "https://192.168.1.20:8787"] {
        let (status, _, body) = send(&site, post_from(elsewhere, "192.168.1.20:8787")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{elsewhere}");
        assert!(body.contains("came from another website"), "{elsewhere}");
    }
    let (_, _, body) = send(&site, get("/settings", &cookie)).await;
    assert!(!body.contains("intruder"), "no account was created");

    // The site's own form, at the address it was opened with, works as before.
    let (status, headers, _) = send(&site, post_from("http://192.168.1.20:8787", "192.168.1.20:8787")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/settings?msg="), "{}", location(&headers));
    // Behind a proxy that takes HTTPS, the proxy says which scheme the visitor used.
    let proxied = Request::post("/settings/users")
        .header(header::COOKIE, &cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::HOST, "prices.example")
        .header("x-forwarded-proto", "https")
        .header(header::ORIGIN, "https://prices.example")
        .body(Body::from("username=second&password=longenough"))
        .unwrap();
    let (status, _, _) = send(&site, proxied).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    // Pages tell browsers to name this site as the origin of its own forms.
    let (_, headers, _) = send(&site, get("/settings", &cookie)).await;
    assert_eq!(headers["referrer-policy"], "same-origin");
}

#[tokio::test]
async fn only_one_first_account_can_be_created() {
    let app = test_app().await;
    let first = crate::db::create_first_user(&app.db, "sam", "h").await.unwrap();
    let second = crate::db::create_first_user(&app.db, "eve", "h").await.unwrap();
    assert!(first.is_some() && second.is_none());
    let users = crate::db::list_users(&app.db).await.unwrap();
    assert_eq!(users.iter().map(|u| (u.username.as_str(), u.is_admin)).collect::<Vec<_>>(), vec![("sam", true)]);
    // The first account gets its settings row like any other.
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_settings").fetch_one(&app.db).await.unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn someone_who_is_not_the_administrator_cannot_point_the_server_at_the_home_network() {
    let site = site().await;
    let admin = set_up(&site).await;
    send(&site, post_form("/settings/users", &admin, "username=alex&password=longenough")).await;
    let login = json!({ "username": "alex", "password": "longenough" });
    let (_, _, body) = send(&site, post_json("/api/v1/login", "", login)).await;
    let token = serde_json::from_str::<Value>(&body).unwrap()["token"].as_str().unwrap().to_string();
    let (_, headers, _) = send(&site, post_form("/login", "", "username=alex&password=longenough")).await;
    let member = cookie_from(&headers);

    // A numbered private address is refused when it is saved, from the app and from the website.
    let local = json!({ "ntfy_url": "http://192.168.1.5:8080/prices" });
    let (status, _, body) = send(&site, post_json("/api/v1/push", &token, local)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("Only the administrator"), "{body}");
    let form = "ntfy_url=&webhook_url=http%3A%2F%2F10.0.0.7%2Fhook";
    let (_, headers, _) = send(&site, post_form("/settings/alerts", &member, form)).await;
    assert!(location(&headers).contains("err=That+address+is+on+the+home+network"), "{}", location(&headers));
    // So is a local AI service, when asked for its models.
    let form = "provider=ollama&api_key=&base_url=http%3A%2F%2F192.168.1.9%3A11434%2Fv1&model=&match_model=";
    let (_, _, body) = send(&site, post_form("/settings/ai/models", &member, form)).await;
    assert!(body.contains("Only the administrator"), "{body}");

    // The administrator may: the address is saved (nothing is sent to it here).
    let form = "ntfy_url=http%3A%2F%2F192.168.1.5%3A8080%2Fprices&webhook_url=";
    let (_, headers, _) = send(&site, post_form("/settings/alerts", &admin, form)).await;
    assert!(location(&headers).starts_with("/settings?msg="), "{}", location(&headers));
}

#[tokio::test]
async fn a_buy_by_day_makes_checks_closer_and_sends_reminders_once() {
    use crate::{alerts, db, service};
    const FOUR_HOURS: i64 = 4 * 3600;

    let (site, app) = site_and_app().await;
    let cookie = set_up(&site).await;
    send(&site, post_form("/watches", &cookie, ADD_CHAIR)).await;
    let day = |ahead: i64| service::day_text(db::now() + ahead * db::DAY);

    // A day that has passed is refused; one five days off is kept and shown.
    let (_, headers, _) = send(&site, post_form("/products/1/watch", &cookie, &format!("any_drop=on&buy_by={}", day(-3)))).await;
    assert!(location(&headers).contains("err="), "{}", location(&headers));
    let (_, headers, _) = send(&site, post_form("/products/1/watch", &cookie, &format!("any_drop=on&buy_by={}", day(5)))).await;
    assert!(location(&headers).contains("msg="), "{}", location(&headers));
    let (_, _, body) = send(&site, get("/products/1", &cookie)).await;
    assert!(body.contains(&format!(r#"name="buy_by" value="{}""#, day(5))), "the day is shown as saved");
    let (_, _, body) = send(&site, get("/", &cookie)).await;
    assert!(body.contains("Buy by "), "the watchlist says so");
    assert_eq!(db::check_interval(&app.db, 1, FOUR_HOURS).await.unwrap(), 3600, "hourly in the last week");

    // The phone app: leaving the day out keeps it, a new one replaces it, an empty one removes it.
    let login = json!({ "username": "sam", "password": "longenough" });
    let (_, _, body) = send(&site, post_json("/api/v1/login", "", login)).await;
    let token = serde_json::from_str::<Value>(&body).unwrap()["token"].as_str().unwrap().to_string();
    let (_, _, body) = send(&site, post_json("/api/v1/products/1/watch", &token, json!({ "note": "gift" }))).await;
    let kept = serde_json::from_str::<Value>(&body).unwrap()["watch"]["buy_by"].as_i64().unwrap();
    assert_eq!(service::day_text(kept), day(5));
    let (status, _, body) = send(&site, post_json("/api/v1/products/1/watch", &token, json!({ "buy_by": day(1) }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(db::check_interval(&app.db, 1, FOUR_HOURS).await.unwrap(), 1800, "every half hour in the last two days");
    let (status, _, _) = send(&site, post_json("/api/v1/products/1/watch", &token, json!({ "buy_by": "next friday" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Saved with the day already tomorrow, no reminder is owed for "tomorrow".
    alerts::buy_by_reminders(&app).await.unwrap();
    assert_eq!(db::unread_count(&app.db, 1).await.unwrap(), 0);
    // A day set a while ago, now tomorrow: one reminder, and only one.
    sqlx::query("UPDATE watches SET buy_by_told = 1").execute(&app.db).await.unwrap();
    alerts::buy_by_reminders(&app).await.unwrap();
    alerts::buy_by_reminders(&app).await.unwrap();
    let alerts = db::notifications(&app.db, 1, 0, 10).await.unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].kind, "buy_by");
    assert!(alerts[0].title.ends_with("that is tomorrow"), "{}", alerts[0].title);

    let (_, _, body) = send(&site, post_json("/api/v1/products/1/watch", &token, json!({ "buy_by": "" }))).await;
    assert!(serde_json::from_str::<Value>(&body).unwrap()["watch"]["buy_by"].is_null());
    assert_eq!(db::check_interval(&app.db, 1, FOUR_HOURS).await.unwrap(), FOUR_HOURS);
}

#[tokio::test]
async fn price_history_can_be_downloaded_and_older_prices_added() {
    use crate::db;
    use crate::retailers::Observation;

    let (site, app) = site_and_app().await;
    let cookie = set_up(&site).await;
    send(&site, post_form("/watches", &cookie, ADD_CHAIR)).await;
    let seen = Observation { price_cents: Some(5000), in_stock: Some(true), ..Default::default() };
    db::record_observation(&app.db, 1, &seen, "server", db::Audience::Public, None, None).await.unwrap();

    let (status, headers, body) = send(&site, get("/products/1/history.csv", &cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/csv"));
    assert!(headers[header::CONTENT_DISPOSITION].to_str().unwrap().contains(".csv"));
    assert!(body.starts_with("time,price,list_price,in_stock,kind,source\n"), "{body}");
    assert!(body.contains(",50.00,,yes,listed,server"), "{body}");

    // Two older prices; one "from the future" and one from after tracking began are left out.
    let pasted = "prices=2020-01-05%2C+61.00%0A2020-02-01%2C+39.99%0A2090-01-01%2C+1.00";
    let (_, headers, _) = send(&site, post_form("/products/1/history", &cookie, pasted)).await;
    assert!(location(&headers).contains("msg=Added+2+past+prices"), "{}", location(&headers));
    let product = db::product(&app.db, 1).await.unwrap().unwrap();
    assert_eq!(product.lowest_price_cents, Some(3999), "the lowest price on record is now an older one");
    assert_eq!(product.current_price_cents, Some(5000), "today's price is untouched");
    let (_, _, body) = send(&site, get("/products/1", &cookie)).await;
    assert!(body.contains("?range=365"), "a long history can be narrowed");
    assert!(body.contains("Added later"));
    let (_, _, narrowed) = send(&site, get("/products/1?range=30", &cookie)).await;
    assert!(narrowed.contains("<svg"), "the last month still draws");

    // Adding again replaces what was added before.
    let (_, headers, _) = send(&site, post_form("/products/1/history", &cookie, "prices=2021-06-01%2C+45.00")).await;
    assert!(location(&headers).contains("msg=Added+1+past+price."), "{}", location(&headers));
    let imported: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM price_points WHERE source = 'import'").fetch_one(&app.db).await.unwrap();
    assert_eq!(imported, 1);
    assert_eq!(db::product(&app.db, 1).await.unwrap().unwrap().lowest_price_cents, Some(4500));
    let (_, headers, _) = send(&site, post_form("/products/1/history", &cookie, "prices=nonsense")).await;
    assert!(location(&headers).contains("err="));

    // Someone else who tracks the same product cannot rewrite the history everyone sees.
    send(&site, post_form("/settings/users", &cookie, "username=pat&password=longenough")).await;
    let (_, headers, _) = send(&site, post_form("/login", "", "username=pat&password=longenough")).await;
    let pat = cookie_from(&headers);
    send(&site, post_form("/watches", &pat, ADD_CHAIR)).await;
    let (_, headers, _) = send(&site, post_form("/products/1/history", &pat, "prices=2019-01-01%2C+0.01")).await;
    assert!(location(&headers).contains("err=") && location(&headers).contains("administrator"), "{}", location(&headers));
    assert_eq!(db::product(&app.db, 1).await.unwrap().unwrap().lowest_price_cents, Some(4500));

    // The phone API serves the same file.
    let login = json!({ "username": "sam", "password": "longenough" });
    let (_, _, body) = send(&site, post_json("/api/v1/login", "", login)).await;
    let token = serde_json::from_str::<Value>(&body).unwrap()["token"].as_str().unwrap().to_string();
    let (status, _, body) = send(&site, get_api("/api/v1/products/1/history.csv", &token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.lines().count(), 3, "{body}");
}

#[tokio::test]
async fn the_server_says_its_version_and_only_updates_where_it_can() {
    let site = site().await;
    let cookie = set_up(&site).await;
    let (_, _, body) = send(&site, get("/settings", &cookie)).await;
    assert!(body.contains(env!("CARGO_PKG_VERSION")));
    assert!(body.contains("Looking for new versions is turned off"), "this test server does not look");

    // This server was not installed as a service, so it cannot replace itself.
    let (_, headers, _) = send(&site, post_form("/settings/update", &cookie, "")).await;
    assert!(location(&headers).contains("err=") && location(&headers).contains("amepricetrack-update"), "{}", location(&headers));

    let login = json!({ "username": "sam", "password": "longenough" });
    let (_, _, body) = send(&site, post_json("/api/v1/login", "", login)).await;
    let token = serde_json::from_str::<Value>(&body).unwrap()["token"].as_str().unwrap().to_string();
    let (status, _, body) = send(&site, get_api("/api/v1/update", &token)).await;
    assert_eq!(status, StatusCode::OK);
    let state: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(state["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(state["can_install"], false);
    assert!(state["newer"].is_null());
    let (status, _, _) = send(&site, post_json("/api/v1/update", &token, json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Someone who is not the administrator is not told about versions and cannot ask for one.
    send(&site, post_form("/settings/users", &cookie, "username=pat&password=longenough")).await;
    let login = json!({ "username": "pat", "password": "longenough" });
    let (_, _, body) = send(&site, post_json("/api/v1/login", "", login)).await;
    let pat = serde_json::from_str::<Value>(&body).unwrap()["token"].as_str().unwrap().to_string();
    let (status, _, _) = send(&site, post_json("/api/v1/update", &pat, json!({}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
