//! JSON API for the phone app (and scripts). Every call except sign-in needs
//! `Authorization: Bearer <token>`.

use crate::auth::{self, CurrentUser};
use crate::db::{self, WatchEdit};
use crate::retailers::{self, FetchMode};
use crate::service::{self, Problem};
use crate::{alerts, App};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/login", post(login))
        .route("/api/v1/logout", post(logout))
        .route("/api/v1/me", get(me))
        .route("/api/v1/watches", get(list_watches).post(add_watch))
        .route("/api/v1/products/{id}", get(product))
        .route("/api/v1/products/{id}/watch", post(update_watch).delete(remove_watch))
        .route("/api/v1/products/{id}/check", post(check_now))
        .route("/api/v1/products/{id}/compare", post(compare_now))
        .route("/api/v1/comparisons/{id}", post(decide_comparison))
        .route("/api/v1/comparisons/{id}/refresh", post(refresh_comparison))
        .route("/api/v1/find", get(find))
        .route("/api/v1/notifications", get(notifications))
        .route("/api/v1/notifications/read", post(mark_read))
        .route("/api/v1/collect", get(collect))
        .route("/api/v1/ingest", post(ingest))
        .route("/api/v1/chat", post(chat))
}

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl<E: Into<Problem>> From<E> for ApiError {
    fn from(e: E) -> Self {
        match e.into() {
            Problem::User(message) => ApiError(StatusCode::BAD_REQUEST, message),
            Problem::Internal(e) => {
                tracing::error!("api request failed: {e:#}");
                ApiError(StatusCode::INTERNAL_SERVER_ERROR, "The server hit a problem. The details are in its log.".into())
            }
        }
    }
}

fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "That product is not on your list.".into())
}

type ApiResult = Result<Json<Value>, ApiError>;

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
    /// Shown on the Settings page so the device can be recognised and revoked.
    #[serde(default)]
    device_name: String,
}

async fn login(State(app): State<App>, Json(body): Json<LoginBody>) -> ApiResult {
    let user = db::user_by_name(&app.db, body.username.trim()).await?;
    let ok = match &user {
        Some(u) => auth::verify_password(body.password, u.password_hash.clone()).await,
        None => false,
    };
    let Some(user) = user.filter(|_| ok) else {
        tokio::time::sleep(Duration::from_millis(700)).await;
        return Err(ApiError(StatusCode::UNAUTHORIZED, "That username or password is not right.".into()));
    };
    let label = body.device_name.trim();
    let label: String = if label.is_empty() { "Phone".into() } else { label.chars().take(60).collect() };
    let token = auth::create_session(&app.db, user.id, "api", Some(&label)).await?;
    Ok(Json(json!({ "token": token, "username": user.username })))
}

async fn logout(State(app): State<App>, CurrentUser(_user): CurrentUser, headers: axum::http::HeaderMap) -> ApiResult {
    if let Some(token) = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) {
        auth::delete_session(&app.db, token.trim()).await?;
    }
    Ok(Json(json!({ "ok": true })))
}

async fn me(State(app): State<App>, CurrentUser(user): CurrentUser) -> ApiResult {
    let settings = db::settings(&app.db, user.id).await?;
    let mut stores: Vec<Value> =
        retailers::RETAILERS.iter().map(|r| json!({ "id": r.id, "name": r.name, "domain": r.domain })).collect();
    // Stores people added use their site name as the id.
    stores.extend(
        db::custom_stores(&app.db)
            .await?
            .iter()
            .map(|s| json!({ "id": s.retailer, "name": s.name, "domain": s.retailer, "added": true })),
    );
    Ok(Json(json!({
        "username": user.username,
        // The app uses the same look as the website.
        "theme": crate::ui::theme_id(&settings.theme),
        "accent": crate::ui::accent_colour(&settings.accent),
        "is_admin": user.is_admin,
        "unread_alerts": db::unread_count(&app.db, user.id).await?,
        "server_version": env!("CARGO_PKG_VERSION"),
        "stores": stores,
        "location": crate::stores::location(&app.db).await?,
    })))
}

fn view_json(v: &service::WatchView) -> Value {
    let mut value = serde_json::to_value(v).unwrap_or(Value::Null);
    value["store_name"] = json!(retailers::display_name(&v.product.retailer));
    value
}

async fn list_watches(State(app): State<App>, CurrentUser(user): CurrentUser) -> ApiResult {
    let views = service::watch_views(&app, user.id).await?;
    Ok(Json(json!({ "watches": views.iter().map(view_json).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct AddBody {
    /// A product link, or the text a store app shares.
    url: String,
    target_price_cents: Option<i64>,
}

async fn add_watch(State(app): State<App>, CurrentUser(user): CurrentUser, Json(body): Json<AddBody>) -> ApiResult {
    let target = body.target_price_cents.filter(|c| *c > 0);
    let id = service::add_by_url(&app, user.id, &body.url, target).await?;
    let view = service::watch_view(&app, user.id, id).await?.ok_or_else(not_found)?;
    Ok(Json(view_json(&view)))
}

async fn product(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> ApiResult {
    let view = service::watch_view(&app, user.id, id).await?.ok_or_else(not_found)?;
    let history = db::history(&app.db, id, user.id).await?;
    let stores = crate::compare::tracking::for_product(&app.db, user.id, id).await?;
    let mut value = view_json(&view);
    value["history"] = json!(history);
    // One entry per other store: status is pending, found, possible, none, failed or rejected.
    value["other_stores"] = json!(stores);
    Ok(Json(value))
}

#[derive(Deserialize)]
struct WatchBody {
    target_price_cents: Option<i64>,
    drop_percent: Option<f64>,
    #[serde(default = "yes")]
    notify_any_drop: bool,
    #[serde(default)]
    notify_back_in_stock: bool,
    note: Option<String>,
}

fn yes() -> bool {
    true
}

async fn update_watch(
    State(app): State<App>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
    Json(body): Json<WatchBody>,
) -> ApiResult {
    if body.drop_percent.is_some_and(|p| !(p > 0.0 && p <= 100.0)) {
        return Err(service::user_problem("drop_percent must be between 0 and 100.").into());
    }
    let edit = WatchEdit {
        target_price_cents: body.target_price_cents.filter(|c| *c > 0),
        drop_percent: body.drop_percent,
        notify_any_drop: body.notify_any_drop,
        notify_back_in_stock: body.notify_back_in_stock,
        note: body.note.map(|n| n.trim().chars().take(200).collect::<String>()).filter(|n| !n.is_empty()),
    };
    if !db::update_watch(&app.db, user.id, id, &edit).await? {
        return Err(not_found());
    }
    alerts::evaluate(&app, id).await?;
    let view = service::watch_view(&app, user.id, id).await?.ok_or_else(not_found)?;
    Ok(Json(view_json(&view)))
}

async fn remove_watch(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> ApiResult {
    if !db::remove_watch(&app.db, user.id, id).await? {
        return Err(not_found());
    }
    Ok(Json(json!({ "ok": true })))
}

async fn check_now(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> ApiResult {
    if db::watch(&app.db, user.id, id).await?.is_none() {
        return Err(not_found());
    }
    db::check_soon(&app.db, id).await?;
    app.wake.notify_one();
    Ok(Json(json!({ "ok": true })))
}

/// Searches the other stores for this product (again).
async fn compare_now(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> ApiResult {
    if db::watch(&app.db, user.id, id).await?.is_none() {
        return Err(not_found());
    }
    crate::compare::tracking::enqueue(&app, user.id, id, true).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct DecisionBody {
    /// True: the suggested product is the same. False: it is a different product.
    same: bool,
}

async fn decide_comparison(
    State(app): State<App>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
    Json(body): Json<DecisionBody>,
) -> ApiResult {
    match crate::compare::tracking::decide(&app, user.id, id, body.same).await? {
        Some(product_id) => Ok(Json(json!({ "ok": true, "product_id": product_id }))),
        None => Err(ApiError(StatusCode::NOT_FOUND, "That comparison does not exist.".into())),
    }
}

/// Checks both sides of a comparison again now, before asking for a price match.
async fn refresh_comparison(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> ApiResult {
    let row: Option<(i64, Option<i64>)> =
        sqlx::query_as("SELECT source_product_id, product_id FROM comparisons WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user.id)
            .fetch_optional(&app.db)
            .await
            .map_err(Problem::from)?;
    let Some((source, other)) = row else {
        return Err(ApiError(StatusCode::NOT_FOUND, "That comparison does not exist.".into()));
    };
    for product_id in [Some(source), other].into_iter().flatten() {
        db::check_soon(&app.db, product_id).await?;
    }
    app.wake.notify_one();
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct FindQuery {
    /// A product name, or a barcode number.
    q: String,
    /// Without a store: the tidied query, the stores to ask and what is
    /// already tracked. With one: that store's first results.
    store: Option<String>,
}

async fn find(State(app): State<App>, CurrentUser(user): CurrentUser, Query(q): Query<FindQuery>) -> ApiResult {
    match q.store.as_deref() {
        None => Ok(Json(crate::find::start(&app, user.id, &q.q).await?)),
        Some(store) => {
            let query = crate::find::query(&q.q)?;
            Ok(Json(json!(crate::find::at_store(&app, &query, store).await?)))
        }
    }
}

#[derive(Deserialize)]
struct AfterQuery {
    /// Only alerts newer than this id. The app remembers the highest id it has shown.
    #[serde(default)]
    after: i64,
}

async fn notifications(State(app): State<App>, CurrentUser(user): CurrentUser, Query(q): Query<AfterQuery>) -> ApiResult {
    let list = db::notifications(&app.db, user.id, q.after, 100).await?;
    Ok(Json(json!({ "notifications": list, "unread": db::unread_count(&app.db, user.id).await? })))
}

async fn mark_read(State(app): State<App>, CurrentUser(user): CurrentUser) -> ApiResult {
    db::mark_all_read(&app.db, user.id).await?;
    Ok(Json(json!({ "ok": true })))
}

/// The pages the phone should load and send back with `ingest`:
/// `needed` are products the server cannot read itself; `optional` are the
/// rest, useful when the phone is signed in at the store and may see a
/// better price.
async fn collect(State(app): State<App>, CurrentUser(user): CurrentUser) -> ApiResult {
    let has_browser = app.fetcher.has_browser();
    let now = db::now();
    let stale_after = 2 * app.cfg.check_interval.as_secs() as i64;
    let mut needed = Vec::new();
    let mut optional = Vec::new();
    for (p, _) in db::watched(&app.db, user.id).await? {
        let entry = json!({
            "product_id": p.id,
            "store": p.retailer,
            "fail_count": p.fail_count,
            "store_name": retailers::display_name(&p.retailer),
            "url": p.url,
            "title": p.display_title(),
            "last_success_at": p.last_success_at,
        });
        let server_cannot = !has_browser && retailers::default_mode(&p.retailer) == FetchMode::Browser;
        let stale = p.last_success_at.is_none_or(|t| now - t > stale_after) && p.fail_count > 0;
        if server_cannot || stale {
            needed.push(entry);
        } else {
            optional.push(entry);
        }
    }
    Ok(Json(json!({ "needed": needed, "optional": optional })))
}

#[derive(Deserialize)]
struct IngestBody {
    /// Address of the product page that was loaded.
    url: String,
    /// The page's full HTML as the browser rendered it.
    html: String,
    /// True when the device is signed in at the store, so the page may show
    /// a member, employee or app-only price.
    #[serde(default)]
    signed_in: bool,
}

async fn ingest(State(app): State<App>, CurrentUser(user): CurrentUser, Json(body): Json<IngestBody>) -> ApiResult {
    let result = service::ingest(&app, user.id, &body.url, &body.html, body.signed_in).await?;
    Ok(Json(json!(result)))
}

#[derive(Deserialize)]
struct ChatBody {
    message: String,
}

async fn chat(State(app): State<App>, CurrentUser(user): CurrentUser, Json(body): Json<ChatBody>) -> ApiResult {
    let reply = service::ask(&app, user.id, &body.message).await?;
    Ok(Json(json!({ "reply": reply })))
}
