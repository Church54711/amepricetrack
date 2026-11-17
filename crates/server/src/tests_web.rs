//! The website and API, used the way a browser and the phone app use them.

use crate::test_app;
use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn site() -> Router {
    super::routes().merge(crate::api::routes()).with_state(test_app().await)
}

/// The site together with the app behind it, for tests that prepare data directly.
async fn site_and_app() -> (Router, crate::App) {
    let app = test_app().await;
    (super::routes().merge(crate::api::routes()).with_state(app.clone()), app)
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
