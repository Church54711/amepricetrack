//! The website: every page and form.

use crate::auth::{self, CurrentUser};
use crate::db::{self, User, WatchEdit};
use crate::pricing::{dollars, parse_dollars};
use crate::retailers::{self, FetchMode};
use crate::service::{self, Problem, WatchView};
use crate::ui::{self, Chrome, Flash};
use crate::{ai, alerts, App};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use maud::{html, Markup};
use serde::Deserialize;
use std::time::Duration;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/", get(watchlist))
        .route("/setup", get(setup_form).post(setup_submit))
        .route("/login", get(login_form).post(login_submit))
        .route("/logout", post(logout))
        .route("/watches", post(add_watch))
        .route("/products/{id}", get(product_page))
        .route("/products/{id}/watch", post(update_watch))
        .route("/products/{id}/history.csv", get(history_csv))
        .route("/products/{id}/history", post(import_history))
        .route("/products/{id}/history/keepa", post(import_keepa))
        .route("/settings/keepa", post(save_keepa))
        .route("/products/{id}/check", post(check_now))
        .route("/products/{id}/remove", post(remove_watch))
        .route("/products/{id}/compare", post(compare_now))
        .route("/comparisons/{id}/same", post(comparison_same))
        .route("/comparisons/{id}/different", post(comparison_different))
        .route("/comparisons/{id}/match", get(price_match_page))
        .route("/comparisons/{id}/refresh", post(price_match_refresh))
        .route("/find", get(find_page))
        .route("/find/store", get(find_store))
        .route("/settings/compare", post(save_compare_settings))
        .route("/stores", get(stores_page))
        .route("/stores/add", post(add_store))
        .route("/stores/location", post(save_location))
        .route("/stores/{id}/remove", post(remove_store))
        .route("/stores/browser", get(browser_page))
        .route("/stores/browser/screen", get(browser_screen))
        .route("/stores/browser/act", post(browser_act))
        .route("/stores/browser/done", post(browser_done))
        .route("/deals", get(deals))
        .route("/alerts", get(alerts_page))
        .route("/discounts", get(discounts_page).post(add_rule))
        .route("/discounts/preset/staples", post(staples_preset))
        .route("/discounts/{id}", post(update_rule))
        .route("/assistant", get(assistant_page))
        .route("/assistant/send", post(assistant_send))
        .route("/assistant/clear", post(assistant_clear))
        .route("/settings", get(settings_page))
        .route("/settings/appearance", post(save_appearance))
        .route("/theme.css", get(theme_css))
        .route("/settings/alerts", post(save_alert_settings))
        .route("/settings/alerts/test", post(test_alert))
        .route("/settings/ai", post(save_ai))
        .route("/settings/ai/models", post(ai_models))
        .route("/settings/password", post(change_password))
        .route("/settings/tokens", post(create_token))
        .route("/settings/tokens/{id}/revoke", post(revoke_token))
        .route("/settings/users", post(add_user))
        .route("/settings/users/{id}/remove", post(remove_user))
        .route("/settings/backup", post(backup_now))
        .route("/settings/update", post(update_now))
        .route("/settings/update/check", post(update_check))
        .route("/static/app.css", get(|| async { asset("text/css; charset=utf-8", ui::CSS) }))
        .route("/static/app.js", get(|| async { asset("text/javascript; charset=utf-8", ui::JS) }))
        .route("/favicon.svg", get(|| async { asset("image/svg+xml", ui::icon().0) }))
        .fallback(not_found)
        .layer(axum::middleware::map_response(security_headers))
}

/// The whole site: the pages and the phone API, with the checks every request goes through.
pub fn site(app: App) -> Router {
    routes()
        .merge(crate::api::routes())
        .layer(axum::middleware::from_fn_with_state(app.clone(), same_origin_writes))
        .with_state(app)
}

/// Refuses a change that another website asked the browser to make.
///
/// The sign-in cookie is sent with any request to this server's address,
/// including a form posted by a page from another site on the same network
/// name. Browsers say where such a request came from; one that did not come
/// from this site's own pages is turned away. Requests that name no origin
/// (the phone app, scripts) are not a browser acting for another site.
async fn same_origin_writes(State(app): State<App>, request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let changes_something = !matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    let (origin, host, forwarded_host, forwarded_scheme) = {
        let text = |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        (text("origin"), text("host"), text("x-forwarded-host"), text("x-forwarded-proto"))
    };
    if let Some(origin) = origin.filter(|_| changes_something) {
        // This server speaks plain HTTP; a proxy in front says so when the visitor used HTTPS.
        // A page in a browser cannot set these headers on a request to another site, so the
        // only senders that could forge them carry nobody's sign-in cookie.
        let scheme = forwarded_scheme.as_deref().map(|s| s.split(',').next().unwrap_or(s).trim()).unwrap_or("http");
        let is = |authority: &Option<String>| authority.as_deref().is_some_and(|a| origin == format!("{scheme}://{a}"));
        let ours = is(&host) || is(&forwarded_host) || app.cfg.public_url.as_deref() == Some(origin.as_str());
        if !ours {
            tracing::warn!(origin, path = request.uri().path(), "refused a change requested from another site");
            let page = ui::message_page(
                "That did not work",
                "This request came from another website, so it was not carried out. If you reach this server through a proxy \
                 and see this on every form, set APT_PUBLIC_URL to the address you open it with.",
            );
            return (StatusCode::FORBIDDEN, page).into_response();
        }
    }
    next.run(request).await
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "public, max-age=604800")], body).into_response()
}

/// Headers that stop other sites framing these pages or running scripts in them.
async fn security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' https: data: blob:; style-src 'self'; script-src 'self'; \
             frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    );
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    // Other sites learn nothing of where a visitor came from; this site's own forms still
    // say they came from here, which `same_origin_writes` relies on.
    headers.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    response
}

async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, ui::message_page("Page not found", "There is nothing at this address.")).into_response()
}

// ------------------------------------------------------------ plumbing

pub struct WebError(Problem);

impl<E: Into<Problem>> From<E> for WebError {
    fn from(e: E) -> Self {
        WebError(e.into())
    }
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        match self.0 {
            Problem::User(message) => (StatusCode::BAD_REQUEST, ui::message_page("That did not work", &message)).into_response(),
            Problem::Internal(e) => {
                tracing::error!("request failed: {e:#}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ui::message_page("Something went wrong", "The server hit a problem. The details are in its log."),
                )
                    .into_response()
            }
        }
    }
}

type WebResult = Result<Response, WebError>;

#[derive(Deserialize, Default)]
struct PageQuery {
    msg: Option<String>,
    err: Option<String>,
    store: Option<String>,
    /// How much of a price chart to show: 30, 90 or 365 days; anything else is all of it.
    range: Option<String>,
}

impl PageQuery {
    fn flash(&self) -> Option<Flash> {
        match (&self.err, &self.msg) {
            (Some(text), _) => Some(Flash { ok: false, text: text.clone() }),
            (None, Some(text)) => Some(Flash { ok: true, text: text.clone() }),
            _ => None,
        }
    }
}

fn encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

/// Redirects to `path` with a message to show there.
fn go(path: &str, ok: bool, text: &str) -> Response {
    // A position on the page ("#add") goes last, after the message.
    let (path, position) = match path.split_once('#') {
        Some((before, position)) => (before, format!("#{position}")),
        None => (path, String::new()),
    };
    let joiner = if path.contains('?') { '&' } else { '?' };
    let key = if ok { "msg" } else { "err" };
    Redirect::to(&format!("{path}{joiner}{key}={}{position}", encode(text))).into_response()
}

/// Sends the person back with the message when the problem is theirs to fix;
/// server faults become an error page.
fn back(path: &str, problem: Problem) -> WebResult {
    match problem {
        Problem::User(message) => Ok(go(path, false, &message)),
        other => Err(WebError(other)),
    }
}

async fn chrome<'a>(app: &App, user: &'a User, q: &PageQuery) -> Result<Chrome<'a>, WebError> {
    chrome_with(app, user, q.flash()).await
}

/// The page frame for a person: their unread count and chosen appearance.
async fn chrome_with<'a>(app: &App, user: &'a User, flash: Option<Flash>) -> Result<Chrome<'a>, WebError> {
    let settings = db::settings(&app.db, user.id).await?;
    Ok(Chrome {
        user,
        unread: db::unread_count(&app.db, user.id).await?,
        flash,
        theme: ui::theme_id(&settings.theme),
        accent: ui::accent_colour(&settings.accent),
    })
}

fn html_page(markup: Markup) -> Response {
    markup.into_response()
}

fn checked(value: &Option<String>) -> bool {
    value.is_some()
}

/// Reads an optional number typed into a form: blank means "none".
fn optional_percent(text: &str) -> Result<Option<f64>, Problem> {
    let text = text.trim().trim_end_matches('%').trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<f64>() {
        Ok(v) if v > 0.0 && v <= 100.0 => Ok(Some(v)),
        _ => Err(service::user_problem("A percentage must be a number between 0 and 100.")),
    }
}

fn optional_price(text: &str) -> Result<Option<i64>, Problem> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    parse_dollars(text).map(Some).ok_or_else(|| service::user_problem("A price must be a number like 19.99."))
}

// ------------------------------------------------------- setup and sign-in

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

fn credentials_form(action: &str, button: &str, new_password: bool) -> Markup {
    html! {
        form method="post" action=(action) data-once {
            div.field {
                label for="username" { "Username" }
                input #username type="text" name="username" required autofocus autocomplete="username" autocapitalize="none";
            }
            div.field {
                label for="password" { "Password" }
                input #password type="password" name="password" required
                    minlength=[new_password.then_some("8")]
                    autocomplete=(if new_password { "new-password" } else { "current-password" });
                @if new_password { div.hint { "At least 8 characters." } }
            }
            button type="submit" { (button) }
        }
    }
}

fn setup_view(error: Option<&str>) -> Markup {
    ui::auth_page(
        "Create the first account",
        error,
        html! {
            p.muted { "This server has no accounts yet. The account you create now is the administrator." }
            (credentials_form("/setup", "Create account", true))
        },
    )
}

async fn setup_form(State(app): State<App>) -> WebResult {
    if db::user_count(&app.db).await? > 0 {
        return Ok(Redirect::to("/login").into_response());
    }
    Ok(html_page(setup_view(None)))
}

async fn setup_submit(State(app): State<App>, Form(form): Form<Credentials>) -> WebResult {
    if db::user_count(&app.db).await? > 0 {
        return Ok(Redirect::to("/login").into_response());
    }
    if let Err(message) = auth::validate_new_account(&form.username, &form.password) {
        return Ok((StatusCode::BAD_REQUEST, setup_view(Some(message))).into_response());
    }
    let hash = auth::hash_password(form.password).await?;
    // Someone else may have finished setting up while the password was being hashed.
    match db::create_first_user(&app.db, form.username.trim(), &hash).await? {
        Some(id) => signed_in(&app, id).await,
        None => Ok(Redirect::to("/login").into_response()),
    }
}

async fn signed_in(app: &App, user_id: i64) -> WebResult {
    let token = auth::create_session(&app.db, user_id, "web", None).await?;
    let cookie = auth::session_cookie(&token, app.cfg.secure_cookies);
    Ok(([(header::SET_COOKIE, cookie)], Redirect::to("/")).into_response())
}

fn login_view(error: Option<&str>) -> Markup {
    ui::auth_page("Sign in", error, credentials_form("/login", "Sign in", false))
}

async fn login_form(State(app): State<App>) -> WebResult {
    if db::user_count(&app.db).await? == 0 {
        return Ok(Redirect::to("/setup").into_response());
    }
    Ok(html_page(login_view(None)))
}

async fn login_submit(State(app): State<App>, Form(form): Form<Credentials>) -> WebResult {
    let user = db::user_by_name(&app.db, form.username.trim()).await?;
    let ok = match &user {
        Some(u) => auth::verify_password(form.password, u.password_hash.clone()).await,
        None => false,
    };
    match (ok, user) {
        (true, Some(user)) => signed_in(&app, user.id).await,
        _ => {
            // Slow down password guessing.
            tokio::time::sleep(Duration::from_millis(700)).await;
            Ok((StatusCode::UNAUTHORIZED, login_view(Some("That username or password is not right."))).into_response())
        }
    }
}

async fn logout(State(app): State<App>, request: axum::extract::Request) -> WebResult {
    let (parts, _) = request.into_parts();
    if let Some(token) = auth::cookie_value(&parts) {
        auth::delete_session(&app.db, &token).await?;
    }
    Ok(([(header::SET_COOKIE, auth::clear_cookie())], Redirect::to("/login")).into_response())
}

// --------------------------------------------------------------- watchlist

fn browser_notice(app: &App, views: &[WatchView]) -> Option<Markup> {
    if app.fetcher.has_browser() {
        return None;
    }
    let needs_browser = views.iter().any(|v| retailers::default_mode(&v.product.retailer) == FetchMode::Browser);
    needs_browser.then(|| {
        html! {
            div.notice {
                "Staples, Best Buy, Kohl's and Harbor Freight only answer a real browser, and this server has none. "
                "Install Chrome on the server (see the README), or let the phone app report those prices."
            }
        }
    })
}

/// "Buy by Nov 28", highlighted when the day is close.
fn buy_by_tag(day: i64, now: i64) -> Markup {
    let left = db::days_until(day, now);
    html! {
        span.tag.warn[(0..=3).contains(&left)] {
            "Buy by " (ui::when(day, true))
            @match left {
                0 => { " (today)" }
                1 => { " (tomorrow)" }
                2..=7 => { " (" (left) " days)" }
                d if d < 0 => { " (passed)" }
                _ => {}
            }
        }
    }
}

fn item_row(v: &WatchView, now: i64) -> Markup {
    let p = &v.product;
    html! {
        li.item {
            a href={ "/products/" (p.id) } tabindex="-1" aria-hidden="true" { (ui::thumb(p.image_url.as_deref())) }
            div {
                a.item-title href={ "/products/" (p.id) } { (p.display_title()) }
                div.item-meta {
                    span { (ui::store_name(&p.retailer)) }
                    (ui::badges(v))
                    @if let Some(target) = v.watch.target_price_cents { span { "Target " (dollars(target)) } }
                    @if let Some(minutes) = v.watch.check_minutes { span { "Watched closely: every " (interval_text(minutes as u64)) } }
                    @if let Some(day) = v.watch.buy_by { (buy_by_tag(day, now)) }
                    @if let Some(checked) = p.last_success_at { span { "Checked " (ui::ago(now, checked)) } }
                }
            }
            div.item-price { (ui::price_cell(v)) }
        }
    }
}

async fn watchlist(State(app): State<App>, CurrentUser(user): CurrentUser, Query(q): Query<PageQuery>) -> WebResult {
    let chrome = chrome(&app, &user, &q).await?;
    let views = service::watch_views(&app, user.id).await?;
    let now = db::now();

    let mut stores: Vec<&str> = views.iter().map(|v| v.product.retailer.as_str()).collect();
    stores.sort();
    stores.dedup();
    let filter = q.store.as_deref().filter(|s| stores.contains(s));
    let shown: Vec<&WatchView> = views.iter().filter(|v| filter.is_none_or(|s| v.product.retailer == s)).collect();

    let body = html! {
        div.page-head {
            div {
                h1 { "Watchlist" }
                p.muted { "Paste a product link, or type a product's name or barcode, to start tracking its price." }
            }
        }
        @if let Some(notice) = browser_notice(&app, &views) { (notice) }
        div.card {
            form method="post" action="/watches" data-once {
                div.row {
                    div.grow {
                        label for="url" { "Product link, name or barcode" }
                        input #url type="text" name="url" required autocomplete="off" autocapitalize="none"
                            placeholder="https://www.staples.com/\u{2026}/product_123456";
                    }
                    div.fixed {
                        label for="target" { "Target price (optional)" }
                        input #target type="text" name="target" inputmode="decimal" placeholder="$";
                    }
                    button type="submit" data-busy="Adding\u{2026}" { "Track" }
                }
                div.hint {
                    "A link works for Staples, Best Buy, Walmart, Target, Amazon, Kohl's, Harbor Freight and Hot Topic, "
                    "and most other shops that publish standard product data. A name or a barcode number is looked up at each store for you to pick from."
                }
            }
        }
        @if views.is_empty() {
            div.card { div.empty {
                strong { "Nothing tracked yet" }
                "Open a product on a store's website, copy the link from the address bar, and paste it above."
            } }
        } @else {
            @if stores.len() > 1 {
                div.chips {
                    a.chip.active[filter.is_none()] href="/" { "All (" (views.len()) ")" }
                    @for store in &stores {
                        a.chip.active[filter == Some(*store)] href={ "/?store=" (encode(store)) } { (ui::store_name(store)) }
                    }
                }
            }
            div.card { ul.items { @for v in &shown { (item_row(v, now)) } } }
        }
    };
    Ok(html_page(ui::page(&chrome, "watchlist", "Watchlist", body)))
}

#[derive(Deserialize)]
struct AddWatchForm {
    url: String,
    #[serde(default)]
    target: String,
}

async fn add_watch(State(app): State<App>, CurrentUser(user): CurrentUser, Form(form): Form<AddWatchForm>) -> WebResult {
    let target = match optional_price(&form.target) {
        Ok(t) => t,
        Err(p) => return back("/", p),
    };
    // A name or a barcode rather than a link: look it up at the stores.
    if !form.url.trim().is_empty() && !crate::find::is_link(&form.url) {
        return Ok(Redirect::to(&format!("/find?q={}", encode(form.url.trim()))).into_response());
    }
    match service::add_by_url(&app, user.id, &form.url, target).await {
        Ok(id) => Ok(go(&format!("/products/{id}"), true, "Tracking started. The first price check runs in the background.")),
        Err(p) => back("/", p),
    }
}

// ------------------------------------------------------------ product page

/// The "Other stores" card: where else this product was found, and for how much.
fn other_stores_card(v: &WatchView, rows: &[crate::compare::tracking::StoreRow], ignore_marketplace: bool) -> Markup {
    let id = v.product.id;
    let store = ui::store_name(&v.product.retailer);
    let searching = rows.iter().any(|r| r.comparison.status == "pending");
    // Both sides with delivery included, and for the same amount of the item.
    let mine = v.price.total_cents();
    let best = rows
        .iter()
        .filter(|r| r.comparison.status == "found" && r.product.as_ref().is_some_and(|p| p.in_stock != Some(false)))
        .filter(|r| !(ignore_marketplace && r.marketplace))
        .filter_map(|r| r.equivalent_cents.map(|cents| (cents, r)))
        .min_by_key(|(cents, _)| *cents);
    html! {
        div.card #stores data-refresh[searching] {
            div.card-head {
                h2 { "Other stores" }
                @if !rows.is_empty() && !searching {
                    form method="post" action={ "/products/" (id) "/compare" } { button.link type="submit" { "Search again" } }
                }
            }
            @if rows.is_empty() {
                p.muted { "Look for this same product at the other stores and compare prices." }
                form method="post" action={ "/products/" (id) "/compare" } { button.secondary type="submit" { "Compare across stores" } }
            } @else {
                @if let (Some((cents, row)), Some(mine)) = (best, mine) {
                    @if cents < mine {
                        p {
                            strong { (dollars(mine - cents)) " cheaper at " (row.store_name) "." } " "
                            (dollars(cents)) " there"
                            @if let Some(amount) = &row.your_amount { " for " (amount) }
                            ", " (dollars(mine)) " at " (store) "."
                        }
                    } @else {
                        p { (store) " has the best price among the stores where this product was found." }
                    }
                }
                @if searching {
                    p.muted {
                        @if v.product.title.is_none() { "The search starts once this product's own page has been read." }
                        @else { "Searching the other stores. This page updates by itself." }
                    }
                }
                div.table-wrap { table {
                    thead { tr { th { "Store" } th { "What was found" } th.num { "Your price" } } }
                    tbody {
                        @for row in rows {
                            tr {
                                td.nowrap { (row.store_name) }
                                @match (row.comparison.status.as_str(), &row.product) {
                                    ("found" | "possible", Some(p)) => {
                                        td {
                                            a href=(p.url) target="_blank" rel="noopener noreferrer" { (p.display_title()) }
                                            div.item-meta {
                                                @if row.comparison.status == "found" { span.tag.good { "Same product" } }
                                                @else { span.tag.warn { "Possibly the same" } }
                                                @if row.comparison.ai_judged() { span.tag { "Judged by AI" } }
                                                @else if row.comparison.confirmed { span.tag { "Confirmed by you" } }
                                                @if let Some(reason) = &row.comparison.reason { span { (reason) } }
                                                @if p.in_stock == Some(false) { span.tag.bad { "Out of stock" } }
                                                @if let Some(amount) = &row.their_amount { span.tag.warn { "Sold as " (amount) } }
                                                @if row.marketplace {
                                                    span.tag.warn { "Marketplace seller" @if let Some(seller) = &p.seller { ": " (seller) } }
                                                }
                                            }
                                            div.inline-forms {
                                                @if row.comparison.status == "found" && row.their_amount.is_none()
                                                    && matches!((row.equivalent_cents, mine), (Some(theirs), Some(mine)) if theirs < mine) {
                                                    a href={ "/comparisons/" (row.comparison.id) "/match" } { "Price match card" }
                                                }
                                                @if row.comparison.status == "possible" || row.comparison.ai_judged() {
                                                    form method="post" action={ "/comparisons/" (row.comparison.id) "/same" } {
                                                        button.link type="submit" { "Yes, same product" }
                                                    }
                                                }
                                                form method="post" action={ "/comparisons/" (row.comparison.id) "/different" } {
                                                    button.link type="submit" { "Not the same" }
                                                }
                                            }
                                        }
                                        td.num {
                                            @match (row.equivalent_cents, row.your_cents) {
                                                (Some(comparable), Some(cents)) => {
                                                    strong { (dollars(comparable)) }
                                                    // What the figure is made of, when it is not simply the listed price.
                                                    @if let (Some(theirs), Some(yours)) = (&row.their_amount, &row.your_amount) {
                                                        div.price-note { "for " (yours) }
                                                        div.price-note { (dollars(cents)) " for " (theirs) }
                                                    }
                                                    @if let Some(shipping) = row.shipping_cents.filter(|s| *s > 0) {
                                                        div.price-note { "incl. " (dollars(shipping)) " delivery" }
                                                    } @else if row.shipping_extra {
                                                        div.price-note { "plus delivery" }
                                                    }
                                                    @if p.current_price_cents.is_some_and(|listed| listed > cents) {
                                                        div.was { (dollars(p.current_price_cents.unwrap_or(cents))) }
                                                    }
                                                }
                                                _ => { span.muted { "no price" } }
                                            }
                                        }
                                    }
                                    ("pending", _) => { td.muted colspan="2" { "Searching\u{2026}" } }
                                    ("failed", _) => {
                                        td.muted colspan="2" { "Could not search: " (row.comparison.reason.as_deref().unwrap_or("unknown problem")) }
                                    }
                                    ("rejected", _) => { td.muted colspan="2" { "You said the suggested product was not the same." } }
                                    _ => { td.muted colspan="2" { (row.comparison.reason.as_deref().unwrap_or("Not found")) } }
                                }
                            }
                        }
                    }
                } }
            }
        }
    }
}

fn reader_description(v: &WatchView, last_source: Option<&str>) -> &'static str {
    if last_source == Some("device") {
        return "Reported by a phone";
    }
    match v.product.remembered_mode() {
        Some(FetchMode::Http) => "Read directly by the server",
        Some(FetchMode::Browser) => "Read by the server's browser",
        None => "Not read yet",
    }
}

async fn product_page(
    State(app): State<App>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
    Query(q): Query<PageQuery>,
) -> WebResult {
    let Some(v) = service::watch_view(&app, user.id, id).await? else {
        return Ok(not_found().await);
    };
    let chrome = chrome(&app, &user, &q).await?;
    let p = &v.product;
    let now = db::now();
    let history = db::history(&app.db, id, user.id).await?;
    let stores = crate::compare::tracking::for_product(&app.db, user.id, id).await?;
    let personal = v.price.is_personal();
    let store = ui::store_name(&p.retailer);
    let last_source = history.iter().rev().find(|h| !h.member).map(|h| h.source.as_str());
    let pending = p.current_price_cents.is_none() && p.last_error.is_none() && v.price.your_cents.is_none();
    let usual_minutes = (app.cfg.check_interval.as_secs() / 60) as i64;
    let paces = pace_choices(usual_minutes, v.watch.check_minutes);
    // The chart's range, offered once the history is long enough for it to matter.
    let range = ui::CHART_RANGES.iter().find(|(_, name, _)| q.range.as_deref() == Some(name));
    let since = range.map(|(_, _, days)| now - days * db::DAY);
    let oldest = history.iter().map(|h| h.ts).min().unwrap_or(now);
    let offered: Vec<_> = ui::CHART_RANGES.iter().filter(|(_, _, days)| now - oldest > days * db::DAY).collect();
    let settings = db::settings(&app.db, user.id).await?;
    let imported = history.iter().filter(|h| h.source == "import").count();

    let body = html! {
        p.small { a href="/" { "\u{2190} Watchlist" } }
        div.card {
            div.product-head {
                (ui::thumb(p.image_url.as_deref()))
                div {
                    h1 { (p.display_title()) }
                    p.muted {
                        (store)
                        @if let Some(brand) = &p.brand { " \u{b7} " (brand) }
                        " \u{b7} "
                        a href=(p.url) target="_blank" rel="noopener noreferrer" { "Open at " (store) " \u{2197}" }
                    }
                    @match v.price.your_cents {
                        Some(cents) => {
                            div.big-price.personal[personal] { (dollars(cents)) }
                            @if personal {
                                p {
                                    @if let (Some(name), Some(pct)) = (&v.price.discount_name, v.price.discount_percent) {
                                        "Your price with " strong { (name) } " (" (pct) "% off)"
                                    } @else { "Your signed-in price" }
                                    @if let Some(public) = v.price.public_cents { ". Listed at " (dollars(public)) "." }
                                }
                            }
                            @if let Some(shipping) = v.price.shipping_cents.filter(|s| *s > 0) {
                                p.muted { "Plus " (dollars(shipping)) " delivery." }
                            } @else if v.price.shipping_extra {
                                p.muted { "Delivery is charged on top; the store does not say how much." }
                            } @else if let Some(name) = &v.price.shipping_membership {
                                p.muted { "Free delivery with " (name) "." }
                            }
                            @if p.third_party == Some(true) {
                                p.muted { "Sold by " (p.seller.as_deref().unwrap_or("another seller")) ", a marketplace seller, not by " (store) " itself." }
                            }
                        }
                        None => {
                            @if pending { p.muted { "Checking the price now. Reload this page in a moment." } }
                            @else { p.muted { "No price has been read yet." } }
                        }
                    }
                    div.item-meta { (ui::badges(&v)) }
                }
            }
        }
        @if let Some(error) = &p.last_error {
            div.notice {
                "The last check failed: " (error) ". "
                @if p.fetch_mode.is_none() && retailers::default_mode(&p.retailer) == FetchMode::Browser && !app.fetcher.has_browser() {
                    "Install Chrome on the server, or let the phone app report this price."
                } @else {
                    "It will be tried again automatically."
                }
            }
        }
        div.grid-2 {
            div {
                div.card {
                    h2 { "Price history" }
                    @if !offered.is_empty() {
                        p.small.ranges {
                            @for (label, name, _) in &offered {
                                @if range.is_some_and(|(_, shown, _)| shown == name) { strong { (label) } }
                                @else { a href={ "/products/" (id) "?range=" (name) } { (label) } }
                                " \u{b7} "
                            }
                            @if range.is_none() { strong { "All" } } @else { a href={ "/products/" (id) } { "All" } }
                        }
                    }
                    (ui::chart(&history, v.watch.target_price_cents, now, since))
                }
                (other_stores_card(&v, &stores, settings.ignore_marketplace))
                @if !history.is_empty() {
                    div.card {
                        h2 { "Recorded changes" }
                        div.table-wrap { table {
                            thead { tr { th { "When" } th.num { "Price" } th { "Kind" } th { "Stock" } } }
                            tbody {
                                @for h in history.iter().rev().take(30) {
                                    tr {
                                        td { (ui::when(h.ts, false)) }
                                        td.num { (dollars(h.price_cents)) }
                                        td { @if h.member { "Signed-in" } @else if h.source == "import" { "Added later" } @else { "Listed" } }
                                        td { @match h.in_stock { Some(true) => { "In stock" } Some(false) => { "Out of stock" } None => { "" } } }
                                    }
                                }
                            }
                        } }
                        p.small { a href={ "/products/" (id) "/history.csv" } { "Download all " (history.len()) " as a spreadsheet (CSV)" } }
                    }
                }
                div.card #past {
                    h2 { "Prices from before" }
                    p.muted {
                        "Tracking starts with an empty history. Prices from before can be added here, so the chart, "
                        "the average and the lowest price have something to go on."
                        @if imported > 0 { " " (imported) " added so far; adding again replaces them." }
                    }
                    @if p.retailer == "amazon" {
                        @if settings.keepa_key.is_empty() {
                            p.small { "For Amazon, Keepa has the history. With a Keepa key on the " a href="/settings#keepa" { "Settings page" } ", one button fetches it." }
                        } @else {
                            form method="post" action={ "/products/" (id) "/history/keepa" } data-once {
                                button.secondary type="submit" data-busy="Asking Keepa\u{2026}" { "Get Amazon's past prices from Keepa" }
                            }
                        }
                    }
                    details {
                        summary { "Paste dates and prices" }
                        form method="post" action={ "/products/" (id) "/history" } {
                            div.field {
                                label for="prices" { "One date and price per line" }
                                textarea #prices name="prices" rows="6" required placeholder="2025-03-01, 19.99\n2025-06-14, 17.49" {}
                                div.hint { "Copied from a spreadsheet works too, and so does a file downloaded above. Only prices from before tracking started here are kept." }
                            }
                            button.secondary type="submit" { "Add" }
                        }
                    }
                }
            }
            div {
                div.card {
                    h2 { "Alert me when" }
                    form method="post" action={ "/products/" (id) "/watch" } {
                        div.field {
                            label for="target" { "The price reaches" }
                            input #target type="text" name="target" inputmode="decimal" placeholder="No target"
                                value=[v.watch.target_price_cents.map(|c| format!("{:.2}", c as f64 / 100.0))];
                            div.hint { "Compared with your own price, after your discounts." }
                        }
                        div.field {
                            label for="drop" { "It drops by at least (%)" }
                            input #drop type="text" name="drop_percent" inputmode="decimal" placeholder="Any amount"
                                value=[v.watch.drop_percent.map(|d| d.to_string())];
                        }
                        label.check {
                            input type="checkbox" name="any_drop" checked[v.watch.notify_any_drop];
                            span { "Any price drop (when no minimum is set above)" }
                        }
                        label.check {
                            input type="checkbox" name="back_in_stock" checked[v.watch.notify_back_in_stock];
                            span { "It comes back in stock" }
                        }
                        div.field {
                            label for="note" { "Note" }
                            input #note type="text" name="note" maxlength="200" value=[v.watch.note.as_deref()];
                        }
                        div.field {
                            label for="buy_by" { "I want to buy by" }
                            input #buy_by type="date" name="buy_by" value=[v.watch.buy_by.map(service::day_text)];
                            div.hint {
                                "Optional. In the last week before that day every price drop is announced and the price is checked more often; "
                                "you are reminded a few days ahead, the day before and on the day, with how the price compares."
                            }
                        }
                        div.field {
                            label for="check_minutes" { "Check the price" }
                            select #check_minutes name="check_minutes" {
                                option value="" selected[v.watch.check_minutes.is_none()] {
                                    "At the usual pace (about every " (interval_text(usual_minutes as u64)) ")"
                                }
                                @for minutes in &paces {
                                    option value=(minutes) selected[v.watch.check_minutes == Some(*minutes)] {
                                        "Every " (interval_text(*minutes as u64))
                                    }
                                }
                            }
                            div.hint {
                                "Closer checks catch a sale that only lasts a few hours. The same product at the other stores is checked as often. "
                                "Stores notice frequent visits, so keep this for the few things you are waiting on."
                            }
                        }
                        button type="submit" { "Save" }
                    }
                }
                div.card {
                    h2 { "Details" }
                    dl.facts {
                        dt { "Listed price" }
                        dd { @match p.current_price_cents { Some(c) => { (dollars(c)) } None => { "unknown" } } }
                        @if let Some(member) = v.price.member_cents { dt { "Signed-in price" } dd { (dollars(member)) } }
                        @if let Some(list) = p.list_price_cents { dt { "Regular price" } dd { (dollars(list)) } }
                        @if let Some(avg) = v.deal.usual_cents { dt { "90-day average" } dd { (dollars(avg)) } }
                        @if let (Some(low), Some(at)) = (p.lowest_price_cents, p.lowest_price_at) {
                            dt { "Lowest recorded" } dd { (dollars(low)) " on " (ui::when(at, true)) }
                        }
                        @if let Some(stock) = p.in_stock { dt { "Stock" } dd { @if stock { "In stock" } @else { "Out of stock" } } }
                        @if let Some(pickup) = p.pickup {
                            dt { "At your store" }
                            dd { @if pickup { "Ready to collect today" } @else { "Not on the shelf; delivery only" } }
                        }
                        @if let Some(seller) = &p.seller { dt { "Sold by" } dd { (seller) } }
                        @match (v.price.shipping_cents, v.price.shipping_extra) {
                            (Some(0), _) => { dt { "Delivery" } dd { "Free" } }
                            (Some(shipping), _) => { dt { "Delivery" } dd { (dollars(shipping)) } }
                            (None, true) => { dt { "Delivery" } dd { "Charged on top" } }
                            (None, false) => {}
                        }
                        @if let Some(model) = &p.model { dt { "Model" } dd { (model) } }
                        @if let Some(gtin) = &p.gtin { dt { "Barcode" } dd { (gtin) } }
                        dt { "Item number" } dd { (p.retailer_sku) }
                        dt { "Tracking since" } dd { (ui::when(v.watch.created_at, true)) }
                        dt { "Last checked" }
                        dd { @match p.last_success_at { Some(t) => { (ui::ago(now, t)) } None => { "not yet" } } }
                        dt { "How" } dd { (reader_description(&v, last_source)) }
                    }
                }
                div.actions {
                    form method="post" action={ "/products/" (id) "/check" } { button.secondary type="submit" { "Check now" } }
                    form method="post" action={ "/products/" (id) "/remove" }
                        data-confirm="Stop tracking this product? Its price history is deleted if nobody else tracks it." {
                        button.danger type="submit" { "Stop tracking" }
                    }
                }
            }
        }
    };
    Ok(html_page(ui::page(&chrome, "watchlist", &p.display_title(), body)))
}

#[derive(Deserialize)]
struct WatchForm {
    #[serde(default)]
    target: String,
    #[serde(default)]
    drop_percent: String,
    any_drop: Option<String>,
    back_in_stock: Option<String>,
    #[serde(default)]
    note: String,
    #[serde(default)]
    check_minutes: String,
    #[serde(default)]
    buy_by: String,
}

/// The paces a person can pick for one product, in minutes: the ones
/// shorter than the server's usual pace, plus whatever is saved already.
fn pace_choices(usual_minutes: i64, saved: Option<i64>) -> Vec<i64> {
    let mut choices: Vec<i64> = [30, 60, 120].into_iter().filter(|m| *m < usual_minutes).collect();
    if let Some(saved) = saved.filter(|s| !choices.contains(s)) {
        choices.push(saved);
        choices.sort();
    }
    choices
}

async fn update_watch(
    State(app): State<App>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
    Form(form): Form<WatchForm>,
) -> WebResult {
    let page = format!("/products/{id}");
    let buy_by = match service::buy_by_day(&form.buy_by) {
        Ok(day) => day,
        // A day that has passed stays as it is when something else on the form is saved.
        Err(_)
            if db::watch(&app.db, user.id, id).await?.and_then(|w| w.buy_by).map(service::day_text).as_deref()
                == Some(form.buy_by.trim()) =>
        {
            db::watch(&app.db, user.id, id).await?.and_then(|w| w.buy_by)
        }
        Err(p) => return back(&page, p),
    };
    let edit = match (optional_price(&form.target), optional_percent(&form.drop_percent)) {
        (Ok(target), Ok(drop)) => WatchEdit {
            buy_by,
            target_price_cents: target,
            drop_percent: drop,
            notify_any_drop: checked(&form.any_drop),
            notify_back_in_stock: checked(&form.back_in_stock),
            note: Some(form.note.trim().chars().take(200).collect::<String>()).filter(|n| !n.is_empty()),
            check_minutes: form.check_minutes.trim().parse().ok(),
        },
        (Err(p), _) | (_, Err(p)) => return back(&page, p),
    };
    if !db::update_watch(&app.db, user.id, id, &edit).await? {
        return Ok(not_found().await);
    }
    app.wake.notify_one();
    // The new target may already be met.
    alerts::evaluate(&app, id).await?;
    Ok(go(&page, true, "Alert settings saved."))
}

/// The product's whole price history as a file a spreadsheet opens.
async fn history_csv(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> WebResult {
    let Some(v) = service::watch_view(&app, user.id, id).await? else {
        return Ok(not_found().await);
    };
    let csv = crate::history::to_csv(&db::history(&app.db, id, user.id).await?);
    let name = crate::history::file_name(&v.product.display_title());
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
        ],
        csv,
    )
        .into_response())
}

#[derive(Deserialize)]
struct PastPricesForm {
    #[serde(default)]
    prices: String,
}

fn added_text(count: usize) -> String {
    format!(
        "Added {count} past price{}. The chart, average and lowest price now include them.",
        if count == 1 { "" } else { "s" }
    )
}

async fn import_history(
    State(app): State<App>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
    Form(form): Form<PastPricesForm>,
) -> WebResult {
    let page = format!("/products/{id}#past");
    let points = match crate::history::parse_pasted(&form.prices, db::now()) {
        Ok(points) => points,
        Err(why) => return Ok(go(&page, false, &why)),
    };
    match crate::history::import(&app, user.id, id, points).await {
        Ok(count) => Ok(go(&page, true, &added_text(count))),
        Err(problem) => back(&page, problem),
    }
}

async fn import_keepa(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> WebResult {
    let page = format!("/products/{id}#past");
    let Some(v) = service::watch_view(&app, user.id, id).await? else {
        return Ok(not_found().await);
    };
    let key = db::settings(&app.db, user.id).await?.keepa_key;
    if v.product.retailer != "amazon" || key.is_empty() {
        return Ok(go(&page, false, "Keepa only has Amazon products, and needs your Keepa key on the Settings page."));
    }
    let points = match crate::history::from_keepa(&app, &key, &v.product.retailer_sku).await {
        Ok(points) => points,
        Err(problem) => return back(&page, problem),
    };
    match crate::history::import(&app, user.id, id, points).await {
        Ok(count) => Ok(go(&page, true, &added_text(count))),
        Err(problem) => back(&page, problem),
    }
}

#[derive(Deserialize)]
struct KeepaForm {
    #[serde(default)]
    keepa_key: String,
}

async fn save_keepa(State(app): State<App>, CurrentUser(user): CurrentUser, Form(form): Form<KeepaForm>) -> WebResult {
    let key = form.keepa_key.trim();
    // An empty field keeps the saved key; the word "none" removes it.
    let key = match key {
        "" => return Ok(go("/settings#keepa", true, "Nothing changed.")),
        "none" => "",
        key if key.len() > 200 || !key.chars().all(|c| c.is_ascii_alphanumeric()) => {
            return Ok(go("/settings#keepa", false, "That does not look like a Keepa key: it is letters and numbers only."));
        }
        key => key,
    };
    sqlx::query("UPDATE user_settings SET keepa_key = ? WHERE user_id = ?").bind(key).bind(user.id).execute(&app.db).await?;
    Ok(go("/settings#keepa", true, if key.is_empty() { "The Keepa key was removed." } else { "Keepa key saved." }))
}

async fn check_now(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> WebResult {
    if db::watch(&app.db, user.id, id).await?.is_none() {
        return Ok(not_found().await);
    }
    db::check_soon(&app.db, id).await?;
    app.wake.notify_one();
    Ok(go(&format!("/products/{id}"), true, "Checking now. Reload in a few seconds to see the result."))
}

async fn remove_watch(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> WebResult {
    db::remove_watch(&app.db, user.id, id).await?;
    Ok(go("/", true, "Stopped tracking that product."))
}

async fn compare_now(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> WebResult {
    if db::watch(&app.db, user.id, id).await?.is_none() {
        return Ok(not_found().await);
    }
    crate::compare::tracking::enqueue(&app, user.id, id, true).await?;
    Ok(go(&format!("/products/{id}#stores"), true, "Searching the other stores for this product."))
}

async fn comparison_decision(app: &App, user: &User, id: i64, same: bool) -> WebResult {
    match crate::compare::tracking::decide(app, user.id, id, same).await? {
        Some(product_id) => Ok(Redirect::to(&format!("/products/{product_id}#stores")).into_response()),
        None => Ok(not_found().await),
    }
}

async fn comparison_same(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> WebResult {
    comparison_decision(&app, &user, id, true).await
}

async fn comparison_different(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> WebResult {
    comparison_decision(&app, &user, id, false).await
}

/// The row of one comparison, with the tracked product it belongs to.
async fn comparison_row(
    app: &App,
    user: &User,
    comparison_id: i64,
) -> Result<Option<(WatchView, crate::compare::tracking::StoreRow)>, WebError> {
    let source: Option<i64> = sqlx::query_scalar("SELECT source_product_id FROM comparisons WHERE id = ? AND user_id = ?")
        .bind(comparison_id)
        .bind(user.id)
        .fetch_optional(&app.db)
        .await?;
    let Some(source) = source else { return Ok(None) };
    let Some(view) = service::watch_view(app, user.id, source).await? else { return Ok(None) };
    let rows = crate::compare::tracking::for_product(&app.db, user.id, source).await?;
    Ok(rows.into_iter().find(|r| r.comparison.id == comparison_id).map(|row| (view, row)))
}

/// A page to show at the register when asking a store to match another store's price.
async fn price_match_page(
    State(app): State<App>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
    Query(q): Query<PageQuery>,
) -> WebResult {
    let Some((v, row)) = comparison_row(&app, &user, id).await? else {
        return Ok(not_found().await);
    };
    let Some(other) = row.product.as_ref().filter(|_| row.comparison.status == "found") else {
        return Ok(not_found().await);
    };
    let chrome = chrome(&app, &user, &q).await?;
    let now = db::now();
    let mine = &v.product;
    let (my_store, their_store) = (ui::store_name(&mine.retailer), row.store_name.clone());
    let stale = other.last_success_at.is_none_or(|t| now - t > 6 * 3600);
    let saving = match (mine.current_price_cents, other.current_price_cents) {
        (Some(here), Some(there)) if there < here => Some(here - there),
        _ => None,
    };
    let facts = |p: &db::Product| {
        html! {
            dl.facts {
                @if let Some(model) = &p.model { dt { "Model" } dd { (model) } }
                @if let Some(gtin) = &p.gtin { dt { "Barcode" } dd { (gtin) } }
                dt { "Item number" } dd { (p.retailer_sku) }
            }
        }
    };
    let body = html! {
        p.small { a href={ "/products/" (mine.id) "#stores" } { "\u{2190} " (mine.display_title()) } }
        div.card {
            h1 { "Price match" }
            p.muted { "Show this at " (my_store) " when asking them to match the price at " (their_store) "." }
            div.match-grid {
                div.match-side {
                    div.match-label { (my_store) }
                    div.match-title { (mine.display_title()) }
                    @match mine.current_price_cents {
                        Some(cents) => { div.big-price { (dollars(cents)) } }
                        None => { div.muted { "No price read yet" } }
                    }
                    (facts(mine))
                }
                div.match-side.better {
                    div.match-label { (their_store) }
                    div.match-title { (other.display_title()) }
                    @match other.current_price_cents {
                        Some(cents) => { div.big-price.personal { (dollars(cents)) } }
                        None => { div.muted { "No price read yet" } }
                    }
                    (facts(other))
                    p.small.muted {
                        "Read "
                        @match other.last_success_at { Some(t) => { (ui::ago(now, t)) } None => { "never" } }
                    }
                    a.btn href=(other.url) target="_blank" rel="noopener noreferrer" { "Open the listing at " (their_store) " \u{2197}" }
                }
            }
            @if let Some(saving) = saving {
                p { strong { (dollars(saving)) " lower at " (their_store) "." } }
            } @else {
                div.notice { "The listed price at " (their_store) " is not lower than at " (my_store) " right now." }
            }
            ul.checks {
                @if row.marketplace {
                    li.warn {
                        "Sold by " (other.seller.as_deref().unwrap_or("another seller")) ", a marketplace seller. "
                        "Stores usually match only what the other store sells itself."
                    }
                } @else if let Some(seller) = &other.seller {
                    li { "Sold by " (seller) "." }
                }
                @if other.in_stock == Some(false) { li.warn { "Out of stock at " (their_store) ". Stores usually match only items that can be bought now." } }
                @else if other.in_stock == Some(true) { li { "In stock at " (their_store) "." } }
                @if let Some(shipping) = row.shipping_cents.filter(|s| *s > 0) {
                    li.warn { "Delivery adds " (dollars(shipping)) " at " (their_store) ". Stores usually match the item's price, not delivery." }
                } @else if row.shipping_extra {
                    li.warn { "Delivery is charged on top at " (their_store) "." }
                }
                @if row.comparison.ai_judged() {
                    li.warn { "Your AI judged these to be the same product. Check that the model or barcode matches." }
                } @else if row.comparison.confirmed {
                    li { "You confirmed these are the same product." }
                } @else if let Some(reason) = &row.comparison.reason {
                    li { "Matched by: " (reason) "." }
                }
                @if stale { li.warn { "This price was read a while ago. Check it again before you ask." } }
            }
            div.actions {
                form method="post" action={ "/comparisons/" (id) "/refresh" } {
                    button.secondary type="submit" { "Check both prices again" }
                }
            }
            p.hint {
                "Price-match rules differ from store to store and change over time. Most ask for the identical item, in stock, "
                "sold and shipped by the other store itself. Check the current policy at " (my_store) " before relying on this."
            }
        }
    };
    Ok(html_page(ui::page(&chrome, "watchlist", "Price match", body)))
}

async fn price_match_refresh(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> WebResult {
    let Some((v, row)) = comparison_row(&app, &user, id).await? else {
        return Ok(not_found().await);
    };
    db::check_soon(&app.db, v.product.id).await?;
    if let Some(other) = &row.product {
        db::check_soon(&app.db, other.id).await?;
    }
    app.wake.notify_one();
    Ok(go(&format!("/comparisons/{id}/match"), true, "Checking both prices now. Reload in a few seconds to see the result."))
}

// -------------------------------------------------------------------- find

#[derive(Deserialize)]
struct FindQuery {
    #[serde(default)]
    q: String,
    /// When given, this one store is searched while the page is built (the
    /// page's script otherwise asks for each store's results itself).
    store: Option<String>,
    msg: Option<String>,
    err: Option<String>,
}

fn find_results(answer: &crate::find::StoreAnswer) -> Markup {
    html! {
        @if let Some(problem) = &answer.problem {
            p.muted { "Could not search this store: " (problem) }
        } @else if answer.results.is_empty() {
            p.muted { "Nothing found." }
        } @else {
            @for item in &answer.results {
                div.find-item {
                    a href=(item.url) target="_blank" rel="noopener noreferrer" { (item.title) }
                    form method="post" action="/watches" data-once {
                        input type="hidden" name="url" value=(item.url);
                        button.secondary type="submit" data-busy="Adding\u{2026}" { "Track" }
                    }
                }
            }
        }
    }
}

/// Looks a product up by name or barcode at every store, to pick one to track.
async fn find_page(State(app): State<App>, CurrentUser(user): CurrentUser, Query(q): Query<FindQuery>) -> WebResult {
    let flash = PageQuery { msg: q.msg.clone(), err: q.err.clone(), ..Default::default() }.flash();
    let query = match crate::find::query(&q.q) {
        Ok(query) => query,
        Err(problem) => return back("/", problem),
    };
    let chrome = chrome_with(&app, &user, flash).await?;
    let stores = crate::find::stores(&app).await?;
    let tracked = crate::find::already_tracked(&app, user.id, &query).await?;
    let ready = match q.store.as_deref() {
        Some(store) if stores.iter().any(|s| s.retailer == store) => Some(crate::find::at_store(&app, &query, store).await?),
        _ => None,
    };
    let body = html! {
        p.small { a href="/" { "\u{2190} Watchlist" } }
        div.page-head {
            div {
                h1 { "Find a product" }
                p.muted {
                    @if query.barcode { "Stores searched for the barcode " } @else { "Stores searched for " }
                    strong { (query.text) }
                    ". Pick the right one to start tracking it; it is then looked up at the other stores for you."
                }
            }
        }
        div.card {
            form method="get" action="/find" {
                div.row {
                    div.grow {
                        label for="q" { "Product name or barcode" }
                        input #q type="text" name="q" required value=(query.text) autocomplete="off";
                    }
                    button type="submit" { "Search" }
                }
            }
        }
        @if !tracked.is_empty() {
            div.card {
                h2 { "Already on your list" }
                @for p in &tracked {
                    p { a href={ "/products/" (p.id) } { (p.display_title()) } " \u{b7} " (ui::store_name(&p.retailer)) }
                }
            }
        }
        div.card #find data-query=(query.text) {
            @for store in &stores {
                section.find-store data-store=(store.retailer) {
                    h2 { (store.name) }
                    div.find-results {
                        @match ready.as_ref().filter(|a| a.store == store.retailer) {
                            Some(answer) => { (find_results(answer)) }
                            None => {
                                // Without the page's script, each store is searched by following its link.
                                p.muted.find-waiting hidden { "Searching\u{2026}" }
                                noscript {
                                    a href={ "/find?q=" (encode(&query.text)) "&store=" (encode(&store.retailer)) } { "Search " (store.name) }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(html_page(ui::page(&chrome, "watchlist", "Find a product", body)))
}

/// One store's results, for the find page's script.
async fn find_store(State(app): State<App>, CurrentUser(_user): CurrentUser, Query(q): Query<FindQuery>) -> Response {
    let answer = match crate::find::query(&q.q) {
        Ok(query) => crate::find::at_store(&app, &query, q.store.as_deref().unwrap_or("")).await,
        Err(problem) => Err(problem),
    };
    match answer {
        Ok(answer) => Json(serde_json::json!(answer)).into_response(),
        Err(Problem::User(message)) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": message }))).into_response(),
        Err(Problem::Internal(e)) => {
            tracing::error!("find failed: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "The server hit a problem." }))).into_response()
        }
    }
}

#[derive(Deserialize)]
struct CompareSettingsForm {
    auto_compare: Option<String>,
    ignore_marketplace: Option<String>,
}

async fn save_compare_settings(
    State(app): State<App>,
    CurrentUser(user): CurrentUser,
    Form(form): Form<CompareSettingsForm>,
) -> WebResult {
    sqlx::query("UPDATE user_settings SET auto_compare = ?, ignore_marketplace = ? WHERE user_id = ?")
        .bind(checked(&form.auto_compare))
        .bind(checked(&form.ignore_marketplace))
        .bind(user.id)
        .execute(&app.db)
        .await?;
    Ok(go("/settings", true, "Saved."))
}

// ------------------------------------------------------------------- deals

async fn deals(State(app): State<App>, CurrentUser(user): CurrentUser, Query(q): Query<PageQuery>) -> WebResult {
    let chrome = chrome(&app, &user, &q).await?;
    let now = db::now();
    let mut views = service::watch_views(&app, user.id).await?;
    let tracked = views.len();
    views.retain(|v| v.product.in_stock != Some(false) && (v.deal.percent_below.is_some_and(|p| p >= 1.0) || v.deal.at_lowest));
    views.sort_by(|a, b| b.deal.percent_below.unwrap_or(0.0).total_cmp(&a.deal.percent_below.unwrap_or(0.0)));

    let body = html! {
        div.page-head {
            div {
                h1 { "Deals" }
                p.muted {
                    "Tracked products priced below what they usually cost, best first. "
                    "\u{201c}Usual\u{201d} is the 90-day average once a week of history exists, and the store's regular price before that."
                }
            }
        }
        @if views.is_empty() {
            div.card { div.empty {
                strong { "No deals right now" }
                @if tracked == 0 { "Add products on the Watchlist page and deals will show up here." }
                @else { "None of your " (tracked) " tracked products is below its usual price at the moment." }
            } }
        } @else {
            div.card { ul.items { @for v in &views { (item_row(v, now)) } } }
        }
    };
    Ok(html_page(ui::page(&chrome, "deals", "Deals", body)))
}

// ------------------------------------------------------------------ alerts

async fn alerts_page(State(app): State<App>, CurrentUser(user): CurrentUser, Query(q): Query<PageQuery>) -> WebResult {
    let list = db::notifications(&app.db, user.id, 0, 100).await?;
    db::mark_all_read(&app.db, user.id).await?;
    // The page itself shows what was unread, so the badge is already cleared.
    let chrome = Chrome { unread: 0, ..chrome(&app, &user, &q).await? };
    let settings = db::settings(&app.db, user.id).await?;
    let body = html! {
        div.page-head {
            div {
                h1 { "Alerts" }
                p.muted { "Price drops, targets reached and restocks for the products you track." }
            }
        }
        @if settings.ntfy_url.is_empty() && settings.webhook_url.is_empty() {
            div.notice {
                "Alerts are only shown here and in the phone app so far. "
                a href="/settings" { "Add ntfy or a webhook" } " to get them pushed to you."
            }
        }
        div.card {
            @if list.is_empty() {
                div.empty { strong { "No alerts yet" } "You will see one here as soon as a tracked price drops." }
            }
            @for n in &list {
                div.alert-row.unread[n.read_at.is_none()] {
                    div.alert-title {
                        @match n.product_id {
                            Some(pid) => { a href={ "/products/" (pid) } { (n.title) } }
                            None => { (n.title) }
                        }
                    }
                    div.alert-body { (n.body) }
                    div.small.muted { (ui::when(n.ts, false)) }
                }
            }
        }
    };
    Ok(html_page(ui::page(&chrome, "alerts", "Alerts", body)))
}

// ------------------------------------------------------------------ stores

fn reading_text(mode: FetchMode, has_browser: bool) -> &'static str {
    match (mode, has_browser) {
        (FetchMode::Http, _) => "Read directly",
        (FetchMode::Browser, true) => "Read with the server's browser",
        (FetchMode::Browser, false) => "Needs a browser on the server, which is not set up",
    }
}

async fn stores_page(State(app): State<App>, CurrentUser(user): CurrentUser, Query(q): Query<PageQuery>) -> WebResult {
    let chrome = chrome(&app, &user, &q).await?;
    let added = db::custom_stores(&app.db).await?;
    let location = crate::stores::location(&app.db).await?;
    let has_browser = app.fetcher.has_browser();
    let health = crate::health::overview(&app.db).await?;
    let now = db::now();
    // What people set up for each store in the server's own browser.
    let mut setups = std::collections::HashMap::new();
    let mut sign_ins = std::collections::HashMap::new();
    for retailer in retailers::RETAILERS.iter().map(|r| r.id.to_string()).chain(added.iter().map(|s| s.retailer.clone())) {
        let setup = crate::stores::browser_setup(&app.db, &retailer).await?;
        sign_ins.insert(retailer.clone(), crate::stores::sign_in_state(&app.db, &retailer, &setup).await?);
        setups.insert(retailer, setup);
    }
    let set_up = |retailer: &str| -> Markup {
        let setup = setups.get(retailer).cloned().unwrap_or_default();
        html! {
            @if setup.read_with_browser && has_browser { div.hint { "Read with the server's browser, where your store or sign-in is set." } }
            @if let Some(who) = &setup.signed_in_by {
                div.hint {
                    "Signed in by " (who)
                    @if let Some(at) = setup.signed_in_at { " on " (ui::when(at, true)) }
                    ". Prices read here are that account's, for everyone on this server."
                }
                @match sign_ins.get(retailer).copied().flatten() {
                    Some(crate::stores::SignIn::Lapsed(since)) => {
                        div.hint {
                            span.tag.warn { "Looks signed out" }
                            " The store's pages have asked to sign in since " (ui::when(since, true))
                            ", so the ordinary prices are being recorded."
                            @if user.is_admin && has_browser { " Open it in the server's browser to sign in again." }
                        }
                    }
                    Some(crate::stores::SignIn::Holds) => { div.hint { "The sign-in still held at the last price check." } }
                    Some(crate::stores::SignIn::CannotTell) => {
                        div.hint { "This store's pages do not show the server whether the sign-in still holds, so look in now and then." }
                    }
                    Some(crate::stores::SignIn::NotReadYet) | None => {}
                }
            }
            @if user.is_admin && has_browser {
                div.hint { a href={ "/stores/browser?store=" (encode(retailer)) } { "Open in the server's browser" } }
            }
        }
    };
    // How a store's price checks are going, for the table below.
    let status = |retailer: &str| -> Markup {
        let Some(store) = health.iter().find(|h| h.retailer == retailer) else {
            return html! { span.muted { "Nothing tracked here" } };
        };
        html! {
            @if store.unreadable(now) {
                span.tag.bad { "Not readable" }
                div.hint {
                    "Nothing read since " (ui::when(store.down_since().unwrap_or(now), true))
                    @if let Some(error) = &store.last_error { ": " (error) }
                }
            } @else if store.down_since().is_some() {
                span.tag.warn { "Last checks failed" }
                @if let Some(error) = &store.last_error { div.hint { (error) } }
            } @else {
                span.tag.good { "Working" }
                @if let Some(t) = store.last_success_at { div.hint { "Last read " (ui::ago(now, t)) } }
            }
        }
    };

    let body = html! {
        div.page-head {
            div {
                h1 { "Stores" }
                p.muted { "The stores whose prices are tracked and compared, and where you shop." }
            }
        }

        div.card #location {
            h2 { "Your area" }
            p.muted {
                "Some chains charge different prices from store to store. With your ZIP code, the server reads the prices of the store nearest you. "
                "A ZIP code is all it needs; no street address."
            }
            @if user.is_admin {
                form method="post" action="/stores/location" {
                    div.row {
                        div.grow {
                            label for="zip" { "ZIP code" }
                            input #zip type="text" name="zip" inputmode="numeric" maxlength="10" autocomplete="postal-code"
                                placeholder="For example 55403" value=(location.zip.as_deref().unwrap_or(""));
                        }
                        button type="submit" { "Save" }
                    }
                    div.hint { "Leave it empty and save to stop using a location." }
                }
            } @else {
                p { @match &location.zip { Some(zip) => { "ZIP code: " strong { (zip) } } None => { "No location is set. The administrator can set one." } } }
            }
            dl.facts {
                dt { "Target" }
                dd {
                    @match &location.target_store {
                        Some(store) => {
                            "Prices from the " strong { (store.name) } " store"
                            @if !store.place.is_empty() { " in " (store.place) }
                            @if let Some(miles) = store.miles { " (" (format!("{miles:.1}")) " miles away)" }
                        }
                        None => { "Prices from store number " (app.fetcher.target_store()) ", the server's default" }
                    }
                }
                dt { "Walmart" }
                dd {
                    @match &location.walmart_store {
                        Some(store) => {
                            "Prices from store number " strong { (store.id) }
                            @if !store.place.is_empty() { " in " (store.place) }
                            "."
                        }
                        None => { "The store is shown here after the first Walmart price check." }
                    }
                    div.hint {
                        "Walmart chooses the store itself, from the internet connection of this server. If that is not your store, "
                        @if user.is_admin && has_browser {
                            a href="/stores/browser?store=walmart" { "open Walmart in the server's browser" }
                            ", choose your store there, and tick “read with this browser”."
                        } @else {
                            "the administrator can choose your store in the server's own browser."
                        }
                    }
                }
                @for (chain, store) in [("Staples", &location.staples_store), ("Best Buy", &location.bestbuy_store)] {
                    dt { (chain) }
                    dd {
                        @match store {
                            Some(store) => {
                                "Your store is number " strong { (store.id) }
                                @if !store.place.is_empty() { " in " (store.place) }
                                "."
                            }
                            None if !has_browser => { "Needs Chrome on the server." }
                            None => { "The store is shown here after the next " (chain) " price check." }
                        }
                        div.hint {
                            (chain) " charges one online price everywhere. The store decides what can be collected today, "
                            "which each product's page here shows."
                            @if location.zip.is_none() { " Without a ZIP code, " (chain) " picks the store from this server's internet connection." }
                        }
                    }
                }
                dt { "Other stores" }
                dd.muted { "Amazon, Kohl's, Harbor Freight and Hot Topic show one online price everywhere." }
            }
        }

        div.card {
            h2 { "Supported stores" }
            div.table-wrap { table {
                thead { tr { th { "Store" } th { "Website" } th { "How its prices are read" } th { "Status" } th {} } }
                tbody {
                    @for r in retailers::RETAILERS {
                        tr {
                            td { (r.name) }
                            td.muted { (r.domain) }
                            td { (reading_text(r.mode, has_browser)) (set_up(r.id)) }
                            td { (status(r.id)) }
                            td {}
                        }
                    }
                    @for s in &added {
                        tr {
                            td { (s.name) }
                            td.muted { (s.retailer) }
                            td { "Standard product data; searched at " span.muted { (s.search_url) } (set_up(&s.retailer)) }
                            td { (status(&s.retailer)) }
                            td {
                                @if user.is_admin || s.added_by == Some(user.id) {
                                    form method="post" action={ "/stores/" (s.id) "/remove" }
                                        data-confirm={ "Remove " (s.name) "? Products you track there stay; it is no longer searched for matches." } {
                                        button.link type="submit" { "Remove" }
                                    }
                                }
                            }
                        }
                    }
                }
            } }
            p.hint { "You get an alert when a store has not been readable for two days, and another when it works again." }
            @if has_browser {
                p.hint {
                    "The server reads prices with a browser of its own. "
                    @if user.is_admin {
                        "\u{201c}Open in the server's browser\u{201d} lets you use that browser from here: sign in at a store, choose your local store, "
                        "or get past a \u{201c}prove you are human\u{201d} page. The server then reads the prices that browser is shown."
                    } @else {
                        "The administrator can sign in at a store in that browser, or choose a local store there."
                    }
                }
            }
        }

        div.card #add {
            h2 { "Add a store" }
            p.muted {
                "Any shop can be tracked by pasting a product link on the Watchlist. Adding the shop here also lets the server search it, "
                "so the things you track are looked up there and compared."
            }
            form method="post" action="/stores/add" {
                div.field {
                    label for="store_site" { "Store website" }
                    input #store_site type="text" name="site" required inputmode="url" autocapitalize="none" placeholder="www.example.com";
                }
                div.field {
                    label for="store_name" { "Name" }
                    input #store_name type="text" name="name" maxlength="40" placeholder="Optional: taken from the website";
                }
                div.field {
                    label for="store_search" { "Search address" }
                    input #store_search type="text" name="search" inputmode="url" autocapitalize="none" placeholder="Optional: usually found automatically";
                    div.hint {
                        "Only needed if the server cannot find it. Search the store for the word \u{201c}" (crate::stores::SEARCH_WORD)
                        "\u{201d} and paste the address of the results page here."
                    }
                }
                button type="submit" { "Add store" }
            }
            p.hint { "Works with shops that publish standard product data, which most do. Prices behind a sign-in are not read." }
        }
    };
    Ok(html_page(ui::page(&chrome, "stores", "Stores", body)))
}

#[derive(Deserialize)]
struct BrowserQuery {
    #[serde(default)]
    store: String,
    /// Start again at the store's front page.
    fresh: Option<String>,
    msg: Option<String>,
    err: Option<String>,
}

/// The server's own browser, used by hand from this page: to sign in at a
/// store, choose a local store, or get past a "prove you are human" page.
async fn browser_page(State(app): State<App>, CurrentUser(user): CurrentUser, Query(q): Query<BrowserQuery>) -> WebResult {
    if !user.is_admin {
        return Ok(go("/stores", false, "Only the administrator can use the server's browser."));
    }
    let Some(browser) = app.fetcher.browser() else {
        return Ok(go("/stores", false, "This server has no browser set up. Install Chrome on it first (see the README)."));
    };
    let Some(home) = crate::stores::store_home(&q.store) else {
        return Ok(not_found().await);
    };
    if let Err(e) = browser.hands_open(&q.store, &home, q.fresh.is_some()).await {
        tracing::warn!("could not open the browser for a person: {e:#}");
        return Ok(go("/stores", false, "The server's browser could not be started. The reason is in the server's log."));
    }
    let flash = PageQuery { msg: q.msg.clone(), err: q.err.clone(), ..Default::default() }.flash();
    let chrome = chrome_with(&app, &user, flash).await?;
    let name = ui::store_name(&q.store);
    let setup = crate::stores::browser_setup(&app.db, &q.store).await?;
    // Stores read with the browser anyway need no switch for it.
    let always_browser = retailers::by_id(&q.store).is_some_and(|r| r.mode == FetchMode::Browser);
    let body = html! {
        p.small { a href="/stores" { "\u{2190} Stores" } }
        div.page-head {
            div {
                h1 { "The server's browser: " (name) }
                p.muted {
                    "This is the browser the server reads prices with. Sign in at " (name) ", choose your local store, or get past a "
                    "\u{201c}prove you are human\u{201d} page. What you set here stays in that browser, and the server reads the prices it is shown."
                }
            }
        }
        div.card #remote {
            p.small.muted { "Showing " span #remote_address { (home) } }
            img #remote_screen.remote-screen alt="The page in the server's browser. Click it to click on the page.";
            p.small.muted #remote_status role="status" { "Loading the picture\u{2026}" }
            div.actions {
                button.secondary type="button" data-remote-action="back" { "Back" }
                button.secondary type="button" data-remote-action="reload" { "Reload" }
                button.secondary type="button" data-remote-scroll="-500" { "Scroll up" }
                button.secondary type="button" data-remote-scroll="500" { "Scroll down" }
                a.btn.secondary href={ "/stores/browser?store=" (encode(&q.store)) "&fresh=1" } { "Front page" }
            }
            form #remote_type autocomplete="off" {
                div.row {
                    div.grow {
                        label for="remote_text" { "Type into the page" }
                        input #remote_text type="password" autocomplete="off" autocapitalize="none" spellcheck="false";
                    }
                    button type="submit" { "Type it" }
                }
                div.actions {
                    button.secondary type="button" data-remote-key="Enter" { "Enter" }
                    button.secondary type="button" data-remote-key="Tab" { "Tab" }
                    button.secondary type="button" data-remote-key="Backspace" { "Backspace" }
                }
                label.check {
                    input #remote_show type="checkbox";
                    span { "Show what I type" }
                }
                label.check {
                    input #remote_hold type="checkbox";
                    span { "My next click is press-and-hold for 10 seconds (for \u{201c}press and hold\u{201d} checks)" }
                }
            }
            p.hint {
                "Click a box on the picture first, then type into it from here. What you type travels from this page to your server, "
                "so do this at home or over HTTPS. Price checks that need the browser wait while this page is open."
            }
        }
        div.card {
            h2 { "When you are done" }
            form method="post" action="/stores/browser/done" {
                input type="hidden" name="store" value=(q.store);
                label.check {
                    input type="checkbox" name="signed_in" checked[setup.signed_in_by.is_some()];
                    span {
                        "I am signed in at " (name) " in this browser"
                        div.hint { "Shown on the Stores page, so everyone on this server knows whose prices these are." }
                    }
                }
                @if !always_browser {
                    label.check {
                        input type="checkbox" name="read_with_browser" checked[setup.read_with_browser];
                        span {
                            "Read " (name) " prices with this browser from now on"
                            div.hint {
                                "Needed for your sign-in or the store you chose to count: otherwise " (name)
                                " is read with plain requests, which carry neither. If the browser is refused, plain requests are used instead."
                            }
                        }
                    }
                }
                button type="submit" { "Done" }
            }
        }
    };
    Ok(html_page(ui::page(&chrome, "stores", "The server's browser", body)))
}

/// A picture of the tab in use by hand.
async fn browser_screen(State(app): State<App>, CurrentUser(user): CurrentUser) -> Response {
    let Some(browser) = app.fetcher.browser().filter(|_| user.is_admin) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    match browser.hands_screen().await {
        Ok((picture, address)) => {
            let mut response =
                ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")], picture).into_response();
            if let Ok(value) = HeaderValue::from_str(&address) {
                response.headers_mut().insert("x-page-address", value);
            }
            response
        }
        Err(e) => (StatusCode::CONFLICT, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}

/// Something the person did on the picture: a click, typing, a key.
async fn browser_act(
    State(app): State<App>,
    CurrentUser(user): CurrentUser,
    Json(action): Json<crate::browser::HandsAction>,
) -> Response {
    let Some(browser) = app.fetcher.browser().filter(|_| user.is_admin) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    match browser.hands_act(&action).await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::CONFLICT, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
struct BrowserDoneForm {
    store: String,
    signed_in: Option<String>,
    read_with_browser: Option<String>,
}

async fn browser_done(State(app): State<App>, CurrentUser(user): CurrentUser, Form(form): Form<BrowserDoneForm>) -> WebResult {
    if !user.is_admin {
        return Ok(go("/stores", false, "Only the administrator can use the server's browser."));
    }
    if crate::stores::store_home(&form.store).is_none() {
        return Ok(not_found().await);
    }
    let before = crate::stores::browser_setup(&app.db, &form.store).await?;
    let signed_in = checked(&form.signed_in);
    let always_browser = retailers::by_id(&form.store).is_some_and(|r| r.mode == FetchMode::Browser);
    // Someone who signs in again after the sign-in lapsed starts afresh; otherwise who and when are kept.
    let kept = before.signed_in_by.clone().filter(|_| before.signed_out_since.is_none());
    let setup = match (signed_in, kept) {
        (true, Some(_)) => crate::stores::BrowserSetup { read_with_browser: true, ..before.clone() },
        (true, None) => crate::stores::BrowserSetup {
            signed_in_by: Some(user.username.clone()),
            signed_in_at: Some(db::now()),
            read_with_browser: true,
            ..Default::default()
        },
        (false, _) => crate::stores::BrowserSetup { read_with_browser: checked(&form.read_with_browser), ..Default::default() },
    };
    // A sign-in only counts when the store is read with the browser that holds it.
    let setup = crate::stores::BrowserSetup { read_with_browser: setup.read_with_browser && !always_browser, ..setup };
    crate::stores::save_browser_setup(&app, &form.store, &setup).await?;
    if let Some(browser) = app.fetcher.browser() {
        browser.hands_close().await;
    }
    let name = ui::store_name(&form.store);
    Ok(go("/stores", true, &format!("Saved. {name} prices are being read again now.")))
}

#[derive(Deserialize)]
struct AddStoreForm {
    #[serde(default)]
    site: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    search: String,
}

async fn add_store(State(app): State<App>, CurrentUser(user): CurrentUser, Form(form): Form<AddStoreForm>) -> WebResult {
    let new = crate::stores::NewStore { site: &form.site, name: &form.name, search: &form.search };
    match crate::stores::add(&app, user.id, new).await {
        Ok(name) => Ok(go("/stores", true, &format!("{name} added. The things you track will be looked up there."))),
        Err(problem) => back("/stores#add", problem),
    }
}

async fn remove_store(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> WebResult {
    let allowed = user.is_admin || db::custom_stores(&app.db).await?.iter().any(|s| s.id == id && s.added_by == Some(user.id));
    if !allowed {
        return Ok(go("/stores", false, "Only the person who added a store, or the administrator, can remove it."));
    }
    Ok(match crate::stores::remove(&app, id).await? {
        Some(name) => go("/stores", true, &format!("{name} removed.")),
        None => go("/stores", false, "That store is no longer listed."),
    })
}

#[derive(Deserialize)]
struct LocationForm {
    #[serde(default)]
    zip: String,
}

async fn save_location(State(app): State<App>, CurrentUser(user): CurrentUser, Form(form): Form<LocationForm>) -> WebResult {
    if !user.is_admin {
        return Ok(go("/stores", false, "Only the administrator can set the server's location."));
    }
    match crate::stores::set_location(&app, &form.zip).await {
        Ok(message) => Ok(go("/stores", true, &message)),
        Err(problem) => back("/stores#location", problem),
    }
}

// --------------------------------------------------------------- discounts

const STAPLES_BRANDS: &str = "TRU RED, Staples, Coastwide Professional, NXT Technologies, Perk, Union & Scale";
const STAPLES_EXCLUSIONS: &str = "gift card, postage, stamps, prepaid";

async fn discounts_page(State(app): State<App>, CurrentUser(user): CurrentUser, Query(q): Query<PageQuery>) -> WebResult {
    let chrome = chrome(&app, &user, &q).await?;
    let rules = db::rules(&app.db, user.id).await?;
    // Offer the supported stores plus any other shop this person tracks.
    let mut stores: Vec<String> = retailers::RETAILERS.iter().map(|r| r.id.to_string()).collect();
    stores.extend(db::custom_stores(&app.db).await?.into_iter().map(|s| s.retailer));
    for (p, _) in db::watched(&app.db, user.id).await? {
        if !stores.contains(&p.retailer) {
            stores.push(p.retailer);
        }
    }
    let has_staples = rules.iter().any(|r| r.retailer == "staples");

    let body = html! {
        div.page-head {
            div {
                h1 { "Discounts" }
                p.muted {
                    "What you personally get at a store: a discount such as an employee discount, or free delivery from a membership such as Prime or Walmart+. "
                    "Discounts are applied to the listed price to show what you would pay, and your alerts and targets use that price. "
                    "Free delivery is counted when stores are compared."
                }
            }
        }
        @if !has_staples {
            div.card {
                h2 { "Staples associate discount" }
                p {
                    "Adds two rules: 10% off everything, and 25% off Staples' own brands. "
                    "Staples takes the discount off at checkout and leaves out some promotional items, "
                    "so treat the result as a close estimate. You can edit the brand list afterwards."
                }
                form method="post" action="/discounts/preset/staples" { button type="submit" { "Add Staples associate discount" } }
            }
        }
        @if !rules.is_empty() {
            div.card {
                h2 { "Your discounts" }
                @for r in &rules {
                    form.rule.off[!r.enabled] method="post" action={ "/discounts/" (r.id) } {
                        div.row {
                            div.grow {
                                label for={ "name" (r.id) } { "Name (" (ui::store_name(&r.retailer)) ")" }
                                input id={ "name" (r.id) } type="text" name="name" required maxlength="60" value=(r.name);
                            }
                            div.fixed {
                                label for={ "pct" (r.id) } { "Percent off" }
                                input id={ "pct" (r.id) } type="text" name="percent" inputmode="decimal" placeholder="None"
                                    value=[(r.percent > 0.0).then_some(r.percent)];
                            }
                        }
                        div.row {
                            div.grow {
                                label for={ "brands" (r.id) } { "Only these brands" }
                                input id={ "brands" (r.id) } type="text" name="brands" value=(r.brands) placeholder="All brands";
                            }
                            div.grow {
                                label for={ "ex" (r.id) } { "Except items mentioning" }
                                input id={ "ex" (r.id) } type="text" name="exclude_keywords" value=(r.exclude_keywords) placeholder="Nothing excluded";
                            }
                        }
                        label.check {
                            input type="checkbox" name="free_shipping" checked[r.free_shipping];
                            span { "Free delivery on what " (ui::store_name(&r.retailer)) " ships itself" }
                        }
                        div.actions {
                            button type="submit" name="action" value="save" { "Save" }
                            button.secondary type="submit" name="action" value="toggle" {
                                @if r.enabled { "Turn off" } @else { "Turn on" }
                            }
                            button.danger type="submit" name="action" value="delete" { "Delete" }
                        }
                    }
                }
            }
        }
        div.card {
            h2 { "Add a discount or membership" }
            form method="post" action="/discounts" {
                div.row {
                    div.fixed {
                        label for="retailer" { "Store" }
                        select #retailer name="retailer" {
                            @for s in &stores { option value=(s) { (ui::store_name(s)) } }
                        }
                    }
                    div.grow {
                        label for="name" { "Name" }
                        input #name type="text" name="name" required maxlength="60" placeholder="Employee discount, Prime\u{2026}";
                    }
                    div.fixed {
                        label for="percent" { "Percent off" }
                        input #percent type="text" name="percent" inputmode="decimal" placeholder="10";
                    }
                }
                div.row {
                    div.grow {
                        label for="brands" { "Only these brands (optional)" }
                        input #brands type="text" name="brands" placeholder="All brands";
                        div.hint { "Separate with commas. Leave empty to cover everything at the store." }
                    }
                    div.grow {
                        label for="exclude" { "Except items mentioning (optional)" }
                        input #exclude type="text" name="exclude_keywords" placeholder="gift card, postage";
                        div.hint { "Items whose name contains one of these words keep the listed price." }
                    }
                }
                label.check {
                    input type="checkbox" name="free_shipping";
                    span {
                        "Free delivery at this store"
                        div.hint { "For a membership such as Prime or Walmart+. Leave the percent empty if that is all it gives you." }
                    }
                }
                button type="submit" { "Add" }
            }
        }
    };
    Ok(html_page(ui::page(&chrome, "discounts", "Discounts", body)))
}

#[derive(Deserialize)]
struct RuleForm {
    #[serde(default)]
    retailer: String,
    name: String,
    percent: String,
    #[serde(default)]
    brands: String,
    #[serde(default)]
    exclude_keywords: String,
    #[serde(default)]
    action: String,
    free_shipping: Option<String>,
}

fn rule_fields(form: &RuleForm) -> Result<(String, f64), Problem> {
    let name = form.name.trim();
    if name.is_empty() {
        return Err(service::user_problem("Give it a name."));
    }
    // A membership may give free delivery and nothing off the price.
    let percent = match optional_percent(&form.percent)? {
        Some(percent) => percent,
        None if checked(&form.free_shipping) => 0.0,
        None => return Err(service::user_problem("Enter the percent off, or tick free delivery.")),
    };
    Ok((name.chars().take(60).collect(), percent))
}

async fn add_rule(State(app): State<App>, CurrentUser(user): CurrentUser, Form(form): Form<RuleForm>) -> WebResult {
    let (name, percent) = match rule_fields(&form) {
        Ok(v) => v,
        Err(p) => return back("/discounts", p),
    };
    let retailer = form.retailer.trim();
    if retailer.is_empty() {
        return back("/discounts", service::user_problem("Choose a store."));
    }
    let (brands, except) = (form.brands.trim(), form.exclude_keywords.trim());
    db::add_terms(&app.db, user.id, retailer, &name, percent, brands, except, checked(&form.free_shipping)).await?;
    service::rebaseline(&app, user.id, retailer).await?;
    Ok(go("/discounts", true, "Added. Your prices now include it."))
}

async fn staples_preset(State(app): State<App>, CurrentUser(user): CurrentUser) -> WebResult {
    db::add_rule(&app.db, user.id, "staples", "Associate discount", 10.0, "", STAPLES_EXCLUSIONS).await?;
    db::add_rule(&app.db, user.id, "staples", "Associate discount, Staples brands", 25.0, STAPLES_BRANDS, STAPLES_EXCLUSIONS)
        .await?;
    service::rebaseline(&app, user.id, "staples").await?;
    Ok(go("/discounts", true, "Staples associate discount added. Check the brand list matches your benefits."))
}

async fn update_rule(
    State(app): State<App>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
    Form(form): Form<RuleForm>,
) -> WebResult {
    let retailer: Option<String> = sqlx::query_scalar("SELECT retailer FROM discount_rules WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user.id)
        .fetch_optional(&app.db)
        .await?;
    let Some(retailer) = retailer else {
        return Ok(not_found().await);
    };
    let message = match form.action.as_str() {
        "delete" => {
            sqlx::query("DELETE FROM discount_rules WHERE id = ? AND user_id = ?")
                .bind(id)
                .bind(user.id)
                .execute(&app.db)
                .await?;
            "Discount deleted."
        }
        "toggle" => {
            sqlx::query("UPDATE discount_rules SET enabled = NOT enabled WHERE id = ? AND user_id = ?")
                .bind(id)
                .bind(user.id)
                .execute(&app.db)
                .await?;
            "Discount updated."
        }
        _ => {
            let (name, percent) = match rule_fields(&form) {
                Ok(v) => v,
                Err(p) => return back("/discounts", p),
            };
            sqlx::query(
                "UPDATE discount_rules SET name = ?, percent = ?, brands = ?, exclude_keywords = ?, free_shipping = ?
                 WHERE id = ? AND user_id = ?",
            )
            .bind(name)
            .bind(percent)
            .bind(form.brands.trim())
            .bind(form.exclude_keywords.trim())
            .bind(checked(&form.free_shipping))
            .bind(id)
            .bind(user.id)
            .execute(&app.db)
            .await?;
            "Discount saved."
        }
    };
    service::rebaseline(&app, user.id, &retailer).await?;
    Ok(go("/discounts", true, message))
}

// --------------------------------------------------------------- assistant

async fn assistant_page(State(app): State<App>, CurrentUser(user): CurrentUser, Query(q): Query<PageQuery>) -> WebResult {
    let chrome = chrome(&app, &user, &q).await?;
    let settings = db::settings(&app.db, user.id).await?;
    let ready = ai::AiConfig::from_settings(&settings, true);
    let messages = db::chat_history(&app.db, user.id, 60).await?;
    let suggestions = [
        "What are my best deals right now?",
        "Which of my products are at their lowest price?",
        "What should I wait on, and why?",
        "How much do my discounts save me in total?",
    ];
    let body = html! {
        div.page-head {
            div {
                h1 { "Assistant" }
                p.muted { "Ask about the products you track. The assistant sees your watchlist, prices, discounts and recent alerts." }
            }
            @if !messages.is_empty() {
                form method="post" action="/assistant/clear" data-confirm="Clear this conversation?" {
                    button.secondary type="submit" { "Clear conversation" }
                }
            }
        }
        @match &ready {
            Err(problem) => {
                div.notice { (problem.to_string()) " " a href="/settings#ai" { "Open Settings" } }
            }
            Ok(_) => {
                div.card {
                    div.chat #chat aria-live="polite" {
                        @if messages.is_empty() {
                            div.empty #chat_empty { strong { "Ask a question to get started" } "For example, pick one of the suggestions below." }
                        }
                        @for m in &messages {
                            div.bubble.user[m.role == "user"].assistant[m.role != "user"] { (m.content) }
                        }
                    }
                    div.suggestions {
                        @for s in suggestions { button.secondary type="button" data-ask=(s) { (s) } }
                    }
                    form.composer #composer {
                        textarea name="message" rows="2" maxlength="4000" required aria-label="Your question"
                            placeholder="Ask about your tracked prices\u{2026}" {}
                        button type="submit" { "Send" }
                    }
                    p.hint {
                        "Answers come from " (ai::provider(&settings.ai_provider).map(|p| p.label).unwrap_or("your AI service"))
                        " using the model " (settings.ai_model) ". Your watchlist is sent to it with each question."
                    }
                }
            }
        }
    };
    Ok(html_page(ui::page(&chrome, "assistant", "Assistant", body)))
}

#[derive(Deserialize)]
struct AskBody {
    message: String,
}

async fn assistant_send(State(app): State<App>, CurrentUser(user): CurrentUser, Json(body): Json<AskBody>) -> Response {
    match service::ask(&app, user.id, &body.message).await {
        Ok(reply) => Json(serde_json::json!({ "reply": reply })).into_response(),
        Err(Problem::User(message)) => Json(serde_json::json!({ "error": message })).into_response(),
        Err(Problem::Internal(e)) => {
            tracing::error!("assistant failed: {e:#}");
            Json(serde_json::json!({ "error": "The server hit a problem answering. The details are in its log." }))
                .into_response()
        }
    }
}

async fn assistant_clear(State(app): State<App>, CurrentUser(user): CurrentUser) -> WebResult {
    sqlx::query("DELETE FROM chat_messages WHERE user_id = ?").bind(user.id).execute(&app.db).await?;
    Ok(Redirect::to("/assistant").into_response())
}

// ---------------------------------------------------------------- settings

fn interval_text(minutes: u64) -> String {
    match minutes {
        m if m % 60 == 0 && m >= 120 => format!("{} hours", m / 60),
        60 => "hour".into(),
        m => format!("{m} minutes"),
    }
}

async fn settings_view(app: &App, user: &User, flash: Option<Flash>, new_token: Option<&str>) -> Result<Markup, WebError> {
    let chrome = chrome_with(app, user, flash).await?;
    let s = db::settings(&app.db, user.id).await?;
    let custom_accent = ui::accent_colour(&s.accent);
    let match_hint = match ai::provider(&s.ai_provider).map(|p| p.match_model).filter(|m| !m.is_empty()) {
        Some(model) => format!("Same as above (a good choice here: {model})"),
        None => "Same as above".to_string(),
    };
    let tokens: Vec<db::SessionRow> = sqlx::query_as(
        "SELECT id, label, created_at, last_used_at FROM sessions WHERE user_id = ? AND kind = 'api' ORDER BY id DESC",
    )
    .bind(user.id)
    .fetch_all(&app.db)
    .await?;
    let users = if user.is_admin { db::list_users(&app.db).await? } else { vec![] };
    let backups = if user.is_admin { crate::backup::list(&app.cfg.backup_dir) } else { vec![] };
    let newer = crate::update::known_newer().filter(|_| user.is_admin);
    let last_update = crate::update::status().filter(|_| user.is_admin);
    let now = db::now();
    let address = app.cfg.public_url.clone().unwrap_or_else(|| format!("http://<this server's address>:{}", app.cfg.bind.port()));

    let body = html! {
        div.page-head { div { h1 { "Settings" } } }

        div.card #appearance {
            h2 { "Appearance" }
            form method="post" action="/settings/appearance" {
                fieldset.swatches-field {
                    legend { "Theme" }
                    div.swatches {
                        @for theme in ui::THEMES {
                            label.swatch {
                                input type="radio" name="theme" value=(theme.id) checked[chrome.theme == theme.id];
                                span class={ "swatch-box sw-" (theme.id) } {
                                    span.swatch-preview { span.sw-bg {} span.sw-surface {} span.sw-accent {} }
                                    span.swatch-name { (theme.label) }
                                }
                            }
                        }
                    }
                }
                label.check {
                    input #use_accent type="checkbox" name="use_accent" checked[custom_accent.is_some()];
                    span { "Use my own accent colour instead of the theme's" }
                }
                div.field {
                    label for="accent" { "Accent colour" }
                    input #accent type="color" name="accent" value=(custom_accent.as_deref().unwrap_or("#e8b923"));
                    div.hint { "Used for buttons, links, the chart line and highlights." }
                }
                button type="submit" { "Save" }
            }
        }

        div.card {
            h2 { "Getting alerts" }
            p.muted {
                "Alerts always appear on the Alerts page and in the phone app. To have them pushed to you as well, add one of these."
            }
            form method="post" action="/settings/alerts" {
                div.field {
                    label for="ntfy_url" { "ntfy topic address" }
                    input #ntfy_url type="url" name="ntfy_url" value=(s.ntfy_url) placeholder="https://ntfy.sh/your-private-topic-name";
                    div.hint {
                        "ntfy is a free push app for Android and iPhone. Pick a hard-to-guess topic name and subscribe to it in the app. "
                        "The AmePriceTrack phone app can set this up for you, under Settings."
                    }
                }
                div.field {
                    label for="webhook_url" { "Webhook address" }
                    input #webhook_url type="url" name="webhook_url" value=(s.webhook_url) placeholder="https://discord.com/api/webhooks/\u{2026}";
                    div.hint { "Discord webhooks work as they are. Other services receive JSON with title, body and url." }
                }
                div.actions {
                    button type="submit" { "Save" }
                    button.secondary type="submit" formaction="/settings/alerts/test" { "Save and send a test" }
                }
            }
        }

        div.card {
            h2 { "Comparing stores" }
            form method="post" action="/settings/compare" {
                label.check {
                    input type="checkbox" name="auto_compare" checked[s.auto_compare];
                    span {
                        "Look every product I add up at the other stores automatically"
                        div.hint {
                            "Each new product is searched for at the other supported stores, and matches are kept up to date so you can see the best price. "
                            "Turn this off to compare only when you press \u{201c}Compare across stores\u{201d} on a product."
                        }
                    }
                }
                label.check {
                    input type="checkbox" name="ignore_marketplace" checked[s.ignore_marketplace];
                    span {
                        "Leave marketplace sellers out"
                        div.hint {
                            "Amazon and Walmart also list offers from other sellers. With this on, those offers are still shown on a product's page "
                            "but never count as the best price and never raise a \u{201c}cheaper elsewhere\u{201d} alert."
                        }
                    }
                }
                button type="submit" { "Save" }
            }
        }

        div.card #keepa {
            h2 { "Past Amazon prices" }
            p.muted {
                "Keepa keeps years of Amazon price history. With your own Keepa key (a paid service of theirs), "
                "an Amazon product's page here gets a button that fetches its past prices."
            }
            form method="post" action="/settings/keepa" {
                div.field {
                    label for="keepa_key" { "Keepa key" }
                    input #keepa_key type="password" name="keepa_key" autocomplete="off"
                        placeholder=(if s.keepa_key.is_empty() { "None" } else { "Saved. Type a new one to replace it, or none to remove it." });
                }
                button.secondary type="submit" { "Save" }
            }
        }

        div.card #ai {
            h2 { "AI assistant" }
            p.muted { "Connect an AI service to ask questions about your tracked prices and to help match products across stores. Use a hosted service with an API key, or a model running on your own machine." }
            form #ai_form method="post" action="/settings/ai" {
                div.field {
                    label for="ai_provider" { "Service" }
                    select #ai_provider name="provider" {
                        option value="" selected[s.ai_provider.is_empty()] data-base="" data-model="" { "None" }
                        @for p in ai::PROVIDERS {
                            option value=(p.id) selected[s.ai_provider == p.id] data-base=(p.base_url) data-model=(p.default_model) { (p.label) }
                        }
                    }
                }
                div #ai_key_field {
                    div.field {
                        label for="ai_api_key" { "API key" }
                        input #ai_api_key type="password" name="api_key" autocomplete="off"
                            placeholder=(if s.ai_api_key.is_empty() { "Paste the key" } else { "A key is saved. Leave empty to keep it." });
                        div.hint { "Stored on this server only. Local services such as Ollama do not need one." }
                    }
                    div.field {
                        label for="ai_base_url" { "Service address" }
                        input #ai_base_url type="text" name="base_url" value=(s.ai_base_url) inputmode="url";
                        div.hint { "Leave empty for the usual address shown in grey. Change it when Ollama or LM Studio runs on another machine." }
                    }
                    div.field {
                        label for="ai_model" { "Model" }
                        input #ai_model type="text" name="model" value=(s.ai_model) list="ai_models" autocomplete="off";
                        datalist #ai_models {}
                        div.actions {
                            button.secondary #load_models type="button" { "Load models" }
                            span.hint #load_models_status role="status" {}
                        }
                    }
                    div.field {
                        label for="ai_match_model" { "Model for matching products" }
                        input #ai_match_model type="text" name="match_model" value=(s.ai_match_model) list="ai_models" autocomplete="off"
                            placeholder=(match_hint);
                        div.hint {
                            "Used when products are looked up at other stores: it picks the right search results and decides whether a listing is the same product. "
                            "A small, cheap model is enough. Leave empty to use the model above."
                        }
                    }
                }
                button type="submit" { "Save" }
            }
        }

        div.card {
            h2 { "Phone app" }
            p.muted {
                "The Android app signs in with this server's address (" (address) ") and your username and password. "
                "Each signed-in device is listed here; revoke one to sign it out."
            }
            @if let Some(token) = new_token {
                div.flash.ok {
                    "Access token created. Copy it now; it will not be shown again."
                    code.secret { (token) }
                }
            }
            @if !tokens.is_empty() {
                div.table-wrap { table {
                    thead { tr { th { "Device" } th { "Added" } th { "Last used" } th {} } }
                    tbody {
                        @for t in &tokens {
                            tr {
                                td { (t.label.as_deref().unwrap_or("Unnamed")) }
                                td { (ui::when(t.created_at, true)) }
                                td { @match t.last_used_at { Some(at) => { (ui::ago(now, at)) } None => { "never" } } }
                                td {
                                    form method="post" action={ "/settings/tokens/" (t.id) "/revoke" } data-confirm="Sign this device out?" {
                                        button.link type="submit" { "Revoke" }
                                    }
                                }
                            }
                        }
                    }
                } }
            }
            form method="post" action="/settings/tokens" {
                div.row {
                    div.grow {
                        label for="token_label" { "Create an access token by hand" }
                        input #token_label type="text" name="label" maxlength="60" placeholder="What it is for, e.g. a script";
                    }
                    button.secondary type="submit" { "Create token" }
                }
            }
        }

        div.card {
            h2 { "Change password" }
            form method="post" action="/settings/password" {
                div.row {
                    div.grow {
                        label for="current" { "Current password" }
                        input #current type="password" name="current" required autocomplete="current-password";
                    }
                    div.grow {
                        label for="new" { "New password" }
                        input #new type="password" name="new" required minlength="8" autocomplete="new-password";
                    }
                    button type="submit" { "Change" }
                }
            }
        }

        @if user.is_admin {
            div.card {
                h2 { "People" }
                div.table-wrap { table {
                    thead { tr { th { "Username" } th { "Role" } th { "Added" } th {} } }
                    tbody {
                        @for u in &users {
                            tr {
                                td { (u.username) }
                                td { @if u.is_admin { "Administrator" } @else { "Member" } }
                                td { (ui::when(u.created_at, true)) }
                                td {
                                    @if u.id != user.id {
                                        form method="post" action={ "/settings/users/" (u.id) "/remove" }
                                            data-confirm={ "Remove " (u.username) " and everything they track?" } {
                                            button.link type="submit" { "Remove" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } }
                form method="post" action="/settings/users" {
                    div.row {
                        div.grow {
                            label for="new_username" { "Add a person" }
                            input #new_username type="text" name="username" required placeholder="Username" autocomplete="off" autocapitalize="none";
                        }
                        div.grow {
                            label for="new_password" { "Their password" }
                            input #new_password type="password" name="password" required minlength="8" autocomplete="new-password";
                        }
                        button.secondary type="submit" { "Add" }
                    }
                }
            }
            div.card {
                h2 { "Server" }
                dl.facts {
                    dt { "Price checks" } dd { "Each product about every " (interval_text(app.cfg.check_interval.as_secs() / 60)) }
                    dt { "Browser for protected stores" }
                    dd { @match &app.cfg.browser_path { Some(p) => { (p.display()) } None => { "Not found. Staples, Best Buy, Kohl's and Harbor Freight need it." } } }
                    dt { "Data folder" } dd { (app.cfg.data_dir.display()) }
                    dt { "Backups" }
                    dd {
                        @match backups.first() {
                            Some(newest) => {
                                "Last copy made "
                                @match newest.made_at { Some(at) => { (ui::ago(now, at)) } None => { "on " (newest.day) } }
                                "; " (backups.len()) " kept in " (app.cfg.backup_dir.display())
                            }
                            None => { "None yet. They go in " (app.cfg.backup_dir.display()) }
                        }
                        div.hint {
                            @if app.cfg.backup_keep == 0 {
                                "Daily backups are turned off (APT_BACKUP_KEEP=0)."
                            } @else {
                                "A copy of everything is written once a day and the newest " (app.cfg.backup_keep) " are kept. "
                                "They are on the same disk as the data unless APT_BACKUP_DIR points somewhere else."
                            }
                        }
                    }
                    dt #version { "Version" }
                    dd {
                        (crate::update::current())
                        @match &newer {
                            Some(release) => {
                                ". Version " strong { (release.version) } " is available ("
                                a href=(release.page) target="_blank" rel="noopener noreferrer" { "what changed" } ")."
                            }
                            None if app.cfg.update_check => { ". No newer version is known." }
                            None => {}
                        }
                        @if let Some(note) = last_update.as_ref().filter(|n| n.state == "failed") {
                            div.hint { "The last update did not go through: " (note.message) }
                        }
                        @if newer.is_some() && !crate::update::can_install(app) {
                            div.hint { "To update, run this on the server: " code { "sudo amepricetrack-update" } }
                        }
                        @if !app.cfg.update_check { div.hint { "Looking for new versions is turned off (APT_UPDATE_CHECK=0)." } }
                    }
                }
                div.actions {
                    form method="post" action="/settings/backup" data-once {
                        button.secondary type="submit" data-busy="Copying\u{2026}" { "Back up now" }
                    }
                    @if newer.is_some() && crate::update::can_install(app) {
                        form method="post" action="/settings/update" data-once
                            data-confirm="Update the server now? It restarts and is away for about a minute." {
                            button type="submit" data-busy="Starting\u{2026}" { "Update now" }
                        }
                    } @else if app.cfg.update_check {
                        form method="post" action="/settings/update/check" data-once {
                            button.secondary type="submit" data-busy="Looking\u{2026}" { "Look for updates" }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::page(&chrome, "settings", "Settings", body))
}

async fn settings_page(State(app): State<App>, CurrentUser(user): CurrentUser, Query(q): Query<PageQuery>) -> WebResult {
    Ok(html_page(settings_view(&app, &user, q.flash(), None).await?))
}

#[derive(Deserialize)]
struct AppearanceForm {
    #[serde(default)]
    theme: String,
    use_accent: Option<String>,
    #[serde(default)]
    accent: String,
}

async fn save_appearance(State(app): State<App>, CurrentUser(user): CurrentUser, Form(form): Form<AppearanceForm>) -> WebResult {
    let accent = if checked(&form.use_accent) { ui::accent_colour(&form.accent).unwrap_or_default() } else { String::new() };
    sqlx::query("UPDATE user_settings SET theme = ?, accent = ? WHERE user_id = ?")
        .bind(ui::theme_id(&form.theme))
        .bind(accent)
        .bind(user.id)
        .execute(&app.db)
        .await?;
    Ok(go("/settings", true, "Appearance saved."))
}

#[derive(Deserialize)]
struct AccentQuery {
    #[serde(default)]
    c: String,
}

/// The small stylesheet for a custom accent colour. It depends only on the
/// colour in the address, so it needs no sign-in and can be cached.
async fn theme_css(Query(q): Query<AccentQuery>) -> Response {
    let css = ui::accent_colour(&q.c).map(|colour| ui::accent_css(&colour)).unwrap_or_default();
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8"), (header::CACHE_CONTROL, "public, max-age=86400")], css).into_response()
}

#[derive(Deserialize)]
struct AlertSettingsForm {
    #[serde(default)]
    ntfy_url: String,
    #[serde(default)]
    webhook_url: String,
}

async fn store_alert_settings(app: &App, user_id: i64, form: &AlertSettingsForm) -> Result<(String, String), Problem> {
    let ntfy = service::web_address(&form.ntfy_url, "ntfy")?;
    let webhook = service::web_address(&form.webhook_url, "webhook")?;
    crate::net::client_for(app, user_id, &[&ntfy, &webhook]).await?;
    sqlx::query("UPDATE user_settings SET ntfy_url = ?, webhook_url = ? WHERE user_id = ?")
        .bind(&ntfy)
        .bind(&webhook)
        .bind(user_id)
        .execute(&app.db)
        .await?;
    Ok((ntfy, webhook))
}

async fn save_alert_settings(
    State(app): State<App>,
    CurrentUser(user): CurrentUser,
    Form(form): Form<AlertSettingsForm>,
) -> WebResult {
    match store_alert_settings(&app, user.id, &form).await {
        Ok(_) => Ok(go("/settings", true, "Alert settings saved.")),
        Err(p) => back("/settings", p),
    }
}

async fn test_alert(State(app): State<App>, CurrentUser(user): CurrentUser, Form(form): Form<AlertSettingsForm>) -> WebResult {
    let (ntfy, webhook) = match store_alert_settings(&app, user.id, &form).await {
        Ok(v) => v,
        Err(p) => return back("/settings", p),
    };
    if ntfy.is_empty() && webhook.is_empty() {
        return Ok(go("/settings", false, "Add an ntfy or webhook address first, then send a test."));
    }
    let link = app.cfg.public_url.clone().unwrap_or_default();
    let http = match crate::net::client_for(&app, user.id, &[&ntfy, &webhook]).await {
        Ok(http) => http,
        Err(p) => return back("/settings", p),
    };
    let sent = alerts::push(
        &http,
        &ntfy,
        &webhook,
        "AmePriceTrack test alert",
        "If you can read this, alerts will reach you here.",
        &link,
        None,
    )
    .await;
    Ok(match sent {
        Ok(()) => go("/settings", true, "Saved, and a test alert was sent. Check that it arrived."),
        Err(e) => go("/settings", false, &format!("Saved, but the test could not be delivered: {e}")),
    })
}

#[derive(Deserialize)]
struct AiForm {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    match_model: String,
}

/// The AI settings a form describes, falling back to the saved key when the
/// key box was left empty for the same service.
fn merge_ai_form(saved: &db::Settings, form: &AiForm) -> db::Settings {
    let provider = form.provider.trim().to_string();
    let typed_key = form.api_key.trim();
    let api_key = if !typed_key.is_empty() {
        typed_key.to_string()
    } else if provider == saved.ai_provider {
        saved.ai_api_key.clone()
    } else {
        String::new()
    };
    db::Settings {
        ai_provider: provider,
        ai_api_key: api_key,
        ai_base_url: form.base_url.trim().to_string(),
        ai_model: form.model.trim().to_string(),
        ai_match_model: form.match_model.trim().to_string(),
        ..saved.clone()
    }
}

async fn save_ai(State(app): State<App>, CurrentUser(user): CurrentUser, Form(form): Form<AiForm>) -> WebResult {
    let saved = db::settings(&app.db, user.id).await?;
    let mut next = merge_ai_form(&saved, &form);
    if next.ai_provider.is_empty() {
        next.ai_api_key.clear();
        next.ai_base_url.clear();
        next.ai_model.clear();
        next.ai_match_model.clear();
    } else if ai::provider(&next.ai_provider).is_none() {
        return Ok(go("/settings#ai", false, "Choose one of the listed AI services."));
    }
    sqlx::query(
        "UPDATE user_settings SET ai_provider = ?, ai_api_key = ?, ai_base_url = ?, ai_model = ?, ai_match_model = ? WHERE user_id = ?",
    )
    .bind(&next.ai_provider)
    .bind(&next.ai_api_key)
    .bind(&next.ai_base_url)
    .bind(&next.ai_model)
    .bind(&next.ai_match_model)
    .bind(user.id)
    .execute(&app.db)
    .await?;
    if next.ai_provider.is_empty() {
        return Ok(go("/settings", true, "AI assistant disconnected."));
    }
    // Say what is still missing, so the person is not surprised on the Assistant page.
    Ok(match ai::AiConfig::from_settings(&next, true) {
        Ok(_) => go("/settings", true, "AI settings saved. Try it on the Assistant page."),
        Err(e) => go("/settings", false, &format!("Saved, but not ready yet: {e}")),
    })
}

async fn ai_models(State(app): State<App>, CurrentUser(user): CurrentUser, Form(form): Form<AiForm>) -> Response {
    let result = async {
        let saved = db::settings(&app.db, user.id).await?;
        let cfg = ai::AiConfig::from_settings(&merge_ai_form(&saved, &form), false)?;
        let http = crate::net::client_for(&app, user.id, &[&cfg.base_url]).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        ai::list_models(&http, &cfg).await
    }
    .await;
    match result {
        Ok(models) => Json(serde_json::json!({ "models": models })).into_response(),
        Err(e) => Json(serde_json::json!({ "error": e.to_string().replace("on the Settings page", "above") })).into_response(),
    }
}

#[derive(Deserialize)]
struct PasswordForm {
    current: String,
    new: String,
}

async fn change_password(State(app): State<App>, CurrentUser(user): CurrentUser, Form(form): Form<PasswordForm>) -> WebResult {
    if !auth::verify_password(form.current, user.password_hash.clone()).await {
        tokio::time::sleep(Duration::from_millis(700)).await;
        return Ok(go("/settings", false, "The current password is not right."));
    }
    if let Err(message) = auth::validate_new_account(&user.username, &form.new) {
        return Ok(go("/settings", false, message));
    }
    let hash = auth::hash_password(form.new).await?;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?").bind(hash).bind(user.id).execute(&app.db).await?;
    Ok(go("/settings", true, "Password changed."))
}

#[derive(Deserialize)]
struct TokenForm {
    #[serde(default)]
    label: String,
}

async fn create_token(State(app): State<App>, CurrentUser(user): CurrentUser, Form(form): Form<TokenForm>) -> WebResult {
    let label = form.label.trim();
    let label = if label.is_empty() { "Access token" } else { label };
    let token = auth::create_session(&app.db, user.id, "api", Some(label)).await?;
    // Shown once in this response only, never put in a link.
    Ok(html_page(settings_view(&app, &user, None, Some(&token)).await?))
}

async fn revoke_token(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> WebResult {
    sqlx::query("DELETE FROM sessions WHERE id = ? AND user_id = ? AND kind = 'api'")
        .bind(id)
        .bind(user.id)
        .execute(&app.db)
        .await?;
    Ok(go("/settings", true, "That device has been signed out."))
}

/// Writes a copy of the database now, in addition to the daily one.
async fn backup_now(State(app): State<App>, CurrentUser(user): CurrentUser) -> WebResult {
    if !user.is_admin {
        return Ok(go("/settings", false, "Only the administrator can make a backup."));
    }
    Ok(match crate::backup::write(&app.db, &app.cfg.backup_dir).await {
        Ok(backup) => {
            if app.cfg.backup_keep > 0 {
                crate::backup::prune(&app.cfg.backup_dir, app.cfg.backup_keep);
            }
            go("/settings", true, &format!("Backup written to {}.", backup.path.display()))
        }
        Err(e) => {
            tracing::error!("backup on request failed: {e:#}");
            go("/settings", false, &format!("The backup could not be written: {e:#}"))
        }
    })
}

/// Has the machine install the latest release; the installer restarts the server.
async fn update_now(State(app): State<App>, CurrentUser(user): CurrentUser) -> WebResult {
    if !user.is_admin {
        return Ok(go("/settings#version", false, "Only the administrator can update the server."));
    }
    match crate::update::request(&app).await {
        Ok(()) => Ok(go(
            "/settings#version",
            true,
            "The update has started. The server restarts by itself; reload this page in about a minute.",
        )),
        Err(problem) => back("/settings#version", problem),
    }
}

async fn update_check(State(app): State<App>, CurrentUser(user): CurrentUser) -> WebResult {
    if !user.is_admin {
        return Ok(go("/settings#version", false, "Only the administrator can look for updates."));
    }
    Ok(match crate::update::latest(&app, true).await {
        Some(release) if crate::update::is_newer(&release.version, crate::update::current()) => {
            go("/settings#version", true, &format!("Version {} is available.", release.version))
        }
        Some(_) => go("/settings#version", true, "This is the latest version."),
        None => go("/settings#version", false, "Could not find out. The server could not reach the releases page."),
    })
}

async fn add_user(State(app): State<App>, CurrentUser(user): CurrentUser, Form(form): Form<Credentials>) -> WebResult {
    if !user.is_admin {
        return Ok(not_found().await);
    }
    if let Err(message) = auth::validate_new_account(&form.username, &form.password) {
        return Ok(go("/settings", false, message));
    }
    let name = form.username.trim();
    if db::user_by_name(&app.db, name).await?.is_some() {
        return Ok(go("/settings", false, "That username is already taken."));
    }
    let hash = auth::hash_password(form.password).await?;
    db::create_user(&app.db, name, &hash, false).await?;
    Ok(go("/settings", true, &format!("{name} can now sign in.")))
}

async fn remove_user(State(app): State<App>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> WebResult {
    if !user.is_admin || id == user.id {
        return Ok(not_found().await);
    }
    sqlx::query("DELETE FROM users WHERE id = ?").bind(id).execute(&app.db).await?;
    // Products only that person tracked are no longer needed.
    db::delete_unused_products(&app.db).await?;
    Ok(go("/settings", true, "That person has been removed."))
}

#[cfg(test)]
#[path = "tests_web.rs"]
mod tests;
