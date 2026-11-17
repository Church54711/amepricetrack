//! Stores people add to the server, and where the server's people shop.
//!
//! Any shop can already be tracked by pasting a product link. Adding it as a
//! store also teaches the server how to search it, so tracked products are
//! looked up there too. The location picks which local store's prices are
//! read at chains that price per store.

use crate::db;
use crate::fetch::NearbyStore;
use crate::retailers::{self, FetchMode};
use crate::service::{user_problem, Problem};
use crate::App;
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;
use std::sync::LazyLock;
use url::Url;

/// A word to search a store for, so the address of the results page shows
/// where search words go.
pub const SEARCH_WORD: &str = "pricetrack";

/// Makes the names of added stores known to the rest of the server.
pub async fn refresh_names(db: &db::Db) -> anyhow::Result<()> {
    let stores = db::custom_stores(db).await?;
    retailers::set_added_names(stores.into_iter().map(|s| (s.retailer, s.name)));
    Ok(())
}

static RE_LD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<script[^>]*type\s*=\s*["']?application/ld\+json["']?[^>]*>(.*?)</script>"#).unwrap());
static RE_PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{[^{}]*\}").unwrap());

/// Input names shops use for their search box.
const SEARCH_INPUTS: &[&str] =
    &["q", "query", "search", "s", "keyword", "keywords", "searchterm", "term", "text", "st", "k", "ntt", "search_query", "w"];

/// Finds a schema.org SearchAction address anywhere in a JSON-LD value.
fn search_action(value: &Value, depth: usize) -> Option<String> {
    if depth > 6 {
        return None;
    }
    match value {
        Value::Array(items) => items.iter().find_map(|v| search_action(v, depth + 1)),
        Value::Object(map) => {
            if let Some(action) = map.get("potentialAction") {
                let actions: Vec<&Value> = match action {
                    Value::Array(list) => list.iter().collect(),
                    other => vec![other],
                };
                for action in actions {
                    let target = match action.get("target") {
                        Some(Value::String(text)) => Some(text.clone()),
                        Some(Value::Object(entry)) => entry.get("urlTemplate").and_then(|v| v.as_str()).map(str::to_string),
                        _ => None,
                    };
                    if let Some(target) = target.filter(|t| t.contains('{')) {
                        return Some(target);
                    }
                }
            }
            map.get("@graph").and_then(|v| search_action(v, depth + 1))
        }
        _ => None,
    }
}

/// Works out a store's search address from its front page. The result has
/// `{q}` where the search words go.
pub fn detect_search(base: &Url, html: &str) -> Option<String> {
    // 1. The address the shop itself publishes for search engines.
    for block in RE_LD.captures_iter(html) {
        let Ok(value) = serde_json::from_str::<Value>(block[1].trim()) else { continue };
        if let Some(target) = search_action(&value, 0) {
            let template = RE_PLACEHOLDER.replace(&target, "{q}").to_string();
            if let Ok(checked) = normalise_search(base, &template) {
                return Some(checked);
            }
        }
    }
    // 2. The search box on the page.
    let doc = Html::parse_document(html);
    let forms = Selector::parse("form").expect("valid selector");
    let inputs = Selector::parse("input").expect("valid selector");
    for form in doc.select(&forms) {
        if form.value().attr("method").is_some_and(|m| m.eq_ignore_ascii_case("post")) {
            continue;
        }
        let mut search_name = None;
        let mut fixed: Vec<(String, String)> = Vec::new();
        for input in form.select(&inputs) {
            let Some(name) = input.value().attr("name").filter(|n| !n.is_empty()) else { continue };
            let kind = input.value().attr("type").unwrap_or("text").to_ascii_lowercase();
            if kind == "hidden" {
                fixed.push((name.to_string(), input.value().attr("value").unwrap_or("").to_string()));
            } else if search_name.is_none() && (kind == "search" || SEARCH_INPUTS.contains(&name.to_ascii_lowercase().as_str())) {
                search_name = Some(name.to_string());
            }
        }
        let Some(name) = search_name else { continue };
        let Ok(mut action) = base.join(form.value().attr("action").unwrap_or("")) else { continue };
        action.set_fragment(None);
        {
            let mut pairs = action.query_pairs_mut();
            for (key, value) in fixed.iter().filter(|(_, v)| !v.is_empty() && v.len() <= 40) {
                pairs.append_pair(key, value);
            }
            pairs.append_pair(&name, "APTSEARCHWORDS");
        }
        if let Ok(checked) = normalise_search(base, &action.as_str().replace("APTSEARCHWORDS", "{q}")) {
            return Some(checked);
        }
    }
    // 3. Shopify shops all search the same way.
    if html.contains("cdn.shopify.com") || html.contains("Shopify.theme") {
        return base.join("/search?type=product&q={q}").ok().map(|u| u.as_str().replace("%7Bq%7D", "{q}"));
    }
    None
}

/// The shop's own name from its front page, if it gives one: the name it
/// publishes, or else the part of the page title that spells the site name
/// ("BoxLunch" in "BoxLunch | Pop Culture Gifts" for boxlunch.com).
pub fn detect_name(site: &Url, html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let tidy = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let meta = Selector::parse(r#"meta[property="og:site_name"], meta[name="application-name"]"#).expect("valid selector");
    let published = doc
        .select(&meta)
        .filter_map(|m| m.value().attr("content"))
        .map(tidy)
        .find(|name| !name.is_empty() && name.chars().count() <= 40);
    if published.is_some() {
        return published;
    }
    let squash = |text: &str| text.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    let label = squash(retailers::site_name(site).split('.').next().unwrap_or(""));
    let title = Selector::parse("title").expect("valid selector");
    let title: String = doc.select(&title).next().map(|t| t.text().collect()).unwrap_or_default();
    if label.is_empty() {
        return None;
    }
    // The run of words in the title that spells the site name.
    let words: Vec<&str> = title.split_whitespace().collect();
    for start in 0..words.len() {
        let mut spelled = String::new();
        for end in start..words.len().min(start + 5) {
            spelled.push_str(&squash(words[end]));
            if spelled == label {
                return Some(words[start..=end].join(" ").trim_matches(|c: char| !c.is_alphanumeric()).to_string());
            }
            if !label.starts_with(&spelled) {
                break;
            }
        }
    }
    None
}

/// Checks a search address and puts it in the stored form: on the store's own
/// site, with `{q}` where the search words go. A results page reached by
/// searching for the word "pricetrack" is accepted as well.
pub fn normalise_search(site: &Url, typed: &str) -> Result<String, Problem> {
    let typed = typed.trim().replace("%7Bq%7D", "{q}").replace("%7bq%7d", "{q}");
    static RE_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("(?i){SEARCH_WORD}")).unwrap());
    let template = if typed.contains("{q}") {
        typed
    } else if RE_WORD.is_match(&typed) {
        RE_WORD.replace_all(&typed, "{q}").to_string()
    } else {
        return Err(user_problem(format!(
            "The search address needs to show where the search words go. Search the store for the word \u{201c}{SEARCH_WORD}\u{201d} and paste the address of the results page."
        )));
    };
    let probe = retailers::public_site(&template.replace("{q}", "x")).map_err(|e| user_problem(e.to_string()))?;
    if retailers::site_name(&probe) != retailers::site_name(site) {
        return Err(user_problem("The search address has to be on the store's own website."));
    }
    if template.len() > 500 {
        return Err(user_problem("That search address is too long."));
    }
    Ok(if template.contains("://") { template } else { format!("https://{template}") })
}

/// What a person typed to add a store.
pub struct NewStore<'a> {
    /// The store's website (any page of it).
    pub site: &'a str,
    /// Display name; worked out from the site when empty.
    pub name: &'a str,
    /// Search address; worked out from the site when empty.
    pub search: &'a str,
}

/// Adds a store (or updates one already added) and returns its display name.
pub async fn add(app: &App, user_id: i64, new: NewStore<'_>) -> Result<String, Problem> {
    if new.site.trim().is_empty() {
        return Err(user_problem("Type the store's web address first."));
    }
    let site = retailers::public_site(new.site).map_err(|e| user_problem(e.to_string()))?;
    let retailer = retailers::site_name(&site);
    if let Some(known) =
        retailers::RETAILERS.iter().find(|r| retailer == r.domain || retailer.ends_with(&format!(".{}", r.domain)))
    {
        return Err(user_problem(format!("{} is already supported; nothing to add.", known.name)));
    }
    let home = format!("{}://{}/", site.scheme(), site.host_str().unwrap_or_default());
    let typed_name = new.name.split_whitespace().collect::<Vec<_>>().join(" ");
    if typed_name.chars().count() > 40 {
        return Err(user_problem("That name is too long; keep it to 40 characters."));
    }

    // The front page is only needed for what the person left blank.
    let mut front_page: Option<String> = None;
    if new.search.trim().is_empty() || typed_name.is_empty() {
        let mut page = app.fetcher.page(&retailer, &home, FetchMode::Http).await;
        if page.is_err() && app.fetcher.has_browser() {
            page = app.fetcher.page(&retailer, &home, FetchMode::Browser).await;
        }
        match page {
            Ok(html) => front_page = Some(html),
            Err(e) => tracing::info!(store = retailer, "could not read the store's front page: {e}"),
        }
    }
    let search_url = if !new.search.trim().is_empty() {
        normalise_search(&site, new.search)?
    } else {
        front_page.as_deref().and_then(|html| detect_search(&site, html)).ok_or_else(|| {
            user_problem(format!(
                "Could not work out how to search {retailer} by itself. Search the store for the word \u{201c}{SEARCH_WORD}\u{201d}, \
                 copy the address of the results page, and paste it into \u{201c}Search address\u{201d}."
            ))
        })?
    };
    let name = if typed_name.is_empty() {
        front_page.as_deref().and_then(|html| detect_name(&site, html)).unwrap_or_else(|| retailer.clone())
    } else {
        typed_name
    };

    db::save_custom_store(&app.db, &retailer, &name, &search_url, user_id).await?;
    refresh_names(&app.db).await?;
    // Everything this person tracks gets looked up at the new store.
    if db::settings(&app.db, user_id).await?.auto_compare {
        for (product, _) in db::watched(&app.db, user_id).await? {
            crate::compare::tracking::enqueue(app, user_id, product.id, false).await?;
        }
    }
    Ok(name)
}

pub async fn remove(app: &App, id: i64) -> Result<Option<String>, Problem> {
    let removed = db::remove_custom_store(&app.db, id).await?;
    refresh_names(&app.db).await?;
    Ok(removed.map(|s| s.name))
}

// ---------------------------------------------------------------- location

/// Where the people using this server shop.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Location {
    pub zip: Option<String>,
    /// The Target store whose prices are read, when the ZIP code chose one.
    pub target_store: Option<NearbyStore>,
    /// The Walmart store whose prices the server last received. Walmart
    /// chooses it from the server's internet connection.
    pub walmart_store: Option<retailers::LocalStore>,
}

pub async fn location(db: &db::Db) -> anyhow::Result<Location> {
    let zip = db::server_setting(db, "zip").await?;
    let target_store = match db::server_setting(db, "target_store").await? {
        Some(json) => serde_json::from_str::<Value>(&json).ok().and_then(|v| {
            Some(NearbyStore {
                id: v.get("id")?.as_str()?.to_string(),
                name: v.get("name")?.as_str()?.to_string(),
                place: v.get("place").and_then(|p| p.as_str()).unwrap_or("").to_string(),
                miles: v.get("miles").and_then(|m| m.as_f64()),
            })
        }),
        None => None,
    };
    let walmart_store = db::server_setting(db, "walmart_store").await?.and_then(|json| serde_json::from_str(&json).ok());
    Ok(Location { zip, target_store, walmart_store })
}

/// Called at startup: read local-store prices from the saved location.
pub async fn apply_saved_location(app: &App) -> anyhow::Result<()> {
    if let Some(store) = location(&app.db).await?.target_store {
        app.fetcher.set_target_store(&store.id);
    }
    Ok(())
}

/// A US ZIP code as five digits ("12345" or "12345-6789").
pub fn parse_zip(text: &str) -> Option<String> {
    let text = text.trim();
    let five = text.split('-').next().unwrap_or("");
    let rest_ok = text.len() == 5 || (text.len() == 10 && text[6..].chars().all(|c| c.is_ascii_digit()));
    (five.len() == 5 && five.chars().all(|c| c.is_ascii_digit()) && rest_ok && text.is_ascii()).then(|| five.to_string())
}

/// Saves the ZIP code and picks the nearest local store at each chain that
/// prices per store. Returns a sentence describing what changed.
pub async fn set_location(app: &App, zip_text: &str) -> Result<String, Problem> {
    if zip_text.trim().is_empty() {
        db::set_server_setting(&app.db, "zip", "").await?;
        db::set_server_setting(&app.db, "target_store", "").await?;
        app.fetcher.set_target_store(&app.cfg.target_store_id);
        recheck_local_prices(app).await?;
        return Ok("Location cleared. Target prices are read from the server's default store again.".into());
    }
    let zip = parse_zip(zip_text).ok_or_else(|| user_problem("Type a five-digit US ZIP code, such as 55403."))?;
    db::set_server_setting(&app.db, "zip", &zip).await?;
    let nearest = match app.fetcher.target_stores_near(&zip).await {
        Ok(stores) => stores.into_iter().next(),
        Err(e) => {
            return Err(user_problem(format!(
                "The ZIP code was saved, but Target's store finder could not be reached ({e}). Target prices are unchanged; save again later."
            )))
        }
    };
    let Some(store) = nearest else {
        db::set_server_setting(&app.db, "target_store", "").await?;
        app.fetcher.set_target_store(&app.cfg.target_store_id);
        return Ok(format!("Saved {zip}. Target has no store within 100 miles, so its prices stay as they were."));
    };
    db::set_server_setting(&app.db, "target_store", &serde_json::to_string(&store).map_err(anyhow::Error::from)?).await?;
    app.fetcher.set_target_store(&store.id);
    recheck_local_prices(app).await?;
    Ok(format!("Saved {zip}. Target prices now come from the {} store.", store.name))
}

/// Prices that depend on the local store are read again soon.
async fn recheck_local_prices(app: &App) -> anyhow::Result<()> {
    sqlx::query("UPDATE products SET next_check_at = 0 WHERE retailer = 'target'").execute(&app.db).await?;
    app.wake.notify_one();
    Ok(())
}

// ------------------------------------------------- the server's browser, used by hand

/// What a person set up for one store in the server's own browser.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct BrowserSetup {
    /// Someone signed in at the store there: who, and when.
    pub signed_in_by: Option<String>,
    pub signed_in_at: Option<i64>,
    /// Read this store with the browser even though plain requests work,
    /// so the sign-in or the local store chosen there counts.
    pub read_with_browser: bool,
    /// Whether the first product page read after signing in still asked to
    /// sign in. Only when it did not can a later request to sign in mean
    /// the sign-in has lapsed.
    pub prompt_after_sign_in: Option<bool>,
    /// Pages in a row that asked to sign in again.
    pub prompt_reads: u32,
    /// Since when the store has looked signed out, once that was announced.
    pub signed_out_since: Option<i64>,
}

/// Pages in a row that must ask to sign in before the sign-in is called lapsed.
const LAPSE_READS: u32 = 2;

/// What the server can say about a sign-in made in its browser.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SignIn {
    /// Nothing has been read since the person signed in.
    NotReadYet,
    /// The last pages read did not ask to sign in, and signed-out pages here do.
    Holds,
    /// Pages ask to sign in again, since this time.
    Lapsed(i64),
    /// This store's pages look the same signed in or out, as far as the server can see.
    CannotTell,
}

static RE_SIGN_IN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(?:>|aria-label\s*=\s*["'])\s*(?:hello,\s*)?(?:sign|log)[\s-]?in(?:\s*(?:or|/|&amp;|&)\s*[a-z ]{2,24})?\s*(?:<|["'])"#,
    )
    .unwrap()
});

/// True when a page offers to sign in, the way stores do in their header
/// for a visitor who is not signed in ("Sign in", "Hello, sign in",
/// "Log in / Register").
pub fn shows_sign_in_prompt(html: &str) -> bool {
    RE_SIGN_IN.is_match(html)
}

fn prompt_key(retailer: &str) -> String {
    format!("signin_prompt_{retailer}")
}

/// What is known about the sign-in recorded in `setup`.
pub async fn sign_in_state(db: &db::Db, retailer: &str, setup: &BrowserSetup) -> Result<Option<SignIn>, Problem> {
    if setup.signed_in_by.is_none() {
        return Ok(None);
    }
    if let Some(since) = setup.signed_out_since {
        return Ok(Some(SignIn::Lapsed(since)));
    }
    let asked_when_signed_out = db::server_setting(db, &prompt_key(retailer)).await?.as_deref() == Some("yes");
    Ok(Some(match setup.prompt_after_sign_in {
        None => SignIn::NotReadYet,
        Some(false) if asked_when_signed_out => SignIn::Holds,
        _ => SignIn::CannotTell,
    }))
}

/// Looks at a product page the server just read for whether the store's
/// sign-in still holds, and tells the person who signed in when it does not.
pub async fn note_reading(app: &App, retailer: &str, mode: FetchMode, html: &str) -> Result<(), Problem> {
    let asks = shows_sign_in_prompt(html);
    let before = browser_setup(&app.db, retailer).await?;
    if before.signed_in_by.is_none() {
        // Nobody is signed in here: remember whether a signed-out page asks to sign in.
        let key = prompt_key(retailer);
        let known = db::server_setting(&app.db, &key).await?;
        if (asks && known.as_deref() != Some("yes")) || (!asks && known.is_none()) {
            db::set_server_setting(&app.db, &key, if asks { "yes" } else { "no" }).await?;
        }
        return Ok(());
    }
    // A plain request carries no sign-in, so it says nothing about one.
    if mode != FetchMode::Browser {
        return Ok(());
    }
    let mut setup = before.clone();
    let mut lapsed_now = false;
    match setup.prompt_after_sign_in {
        None => setup.prompt_after_sign_in = Some(asks),
        // The pages asked to sign in even right after signing in: nothing can be told from them.
        Some(true) => {}
        Some(false) if asks => {
            setup.prompt_reads += 1;
            if setup.prompt_reads >= LAPSE_READS && setup.signed_out_since.is_none() {
                setup.signed_out_since = Some(db::now());
                lapsed_now = true;
            }
        }
        Some(false) => {
            setup.prompt_reads = 0;
            setup.signed_out_since = None;
        }
    }
    if setup != before {
        db::set_server_setting(&app.db, &setup_key(retailer), &serde_json::to_string(&setup).map_err(anyhow::Error::from)?)
            .await?;
    }
    if lapsed_now {
        let name = retailers::display_name(retailer);
        let title = format!("{name} looks signed out in the server's browser");
        let body = format!(
            "The last {LAPSE_READS} {name} pages the server read asked to sign in. Until someone signs in again, the prices recorded \
             for {name} are the ordinary ones, not the account's. Open {name} in the server's browser from the Stores page to sign in again."
        );
        let link = app.cfg.public_url.as_ref().map(|base| format!("{base}/stores")).unwrap_or_default();
        tracing::warn!(store = retailer, "{title}");
        // The person who signed in, or failing that the administrators.
        let signer = match &setup.signed_in_by {
            Some(name) => db::user_by_name(&app.db, name).await?,
            None => None,
        };
        let people: Vec<i64> = match signer {
            Some(user) => vec![user.id],
            None => db::list_users(&app.db).await?.into_iter().filter(|u| u.is_admin).map(|u| u.id).collect(),
        };
        for user_id in people {
            crate::alerts::notify(app, user_id, None, "store", &title, &body, None, &link).await?;
        }
    }
    Ok(())
}

fn setup_key(retailer: &str) -> String {
    format!("browser_setup_{retailer}")
}

pub async fn browser_setup(db: &db::Db, retailer: &str) -> Result<BrowserSetup, Problem> {
    let stored = db::server_setting(db, &setup_key(retailer)).await?;
    Ok(stored.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default())
}

/// Saves what was set up for a store and applies it to price checks.
pub async fn save_browser_setup(app: &App, retailer: &str, setup: &BrowserSetup) -> Result<(), Problem> {
    let value = if *setup == BrowserSetup::default() {
        String::new()
    } else {
        serde_json::to_string(setup).map_err(anyhow::Error::from)?
    };
    db::set_server_setting(&app.db, &setup_key(retailer), &value).await?;
    apply_browser_setups(app).await?;
    // Read that store's products again soon, the new way.
    sqlx::query("UPDATE products SET next_check_at = 0 WHERE retailer = ?").bind(retailer).execute(&app.db).await?;
    app.wake.notify_one();
    Ok(())
}

/// Tells the price checker which stores to read with the browser first.
pub async fn apply_browser_setups(app: &App) -> Result<(), Problem> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM server_settings WHERE key LIKE 'browser_setup_%'").fetch_all(&app.db).await?;
    let stores = rows.into_iter().filter_map(|(key, value)| {
        let setup: BrowserSetup = serde_json::from_str(&value).ok()?;
        setup.read_with_browser.then(|| key.trim_start_matches("browser_setup_").to_string())
    });
    app.fetcher.set_browser_first(stores);
    Ok(())
}

/// The page to open for a store in the server's browser: its front page.
pub fn store_home(retailer: &str) -> Option<String> {
    match retailers::by_id(retailer) {
        Some(store) => Some(format!("https://www.{}/", store.domain)),
        None => retailers::public_site(retailer).ok().map(|site| site.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(address: &str) -> Url {
        retailers::public_site(address).unwrap()
    }

    #[test]
    fn search_address_from_published_data() {
        let html = r#"<html><head><script type="application/ld+json">
            {"@context":"https://schema.org","@type":"WebSite","url":"https://www.shop.example/",
             "potentialAction":{"@type":"SearchAction","target":"https://www.shop.example/search?q={search_term_string}","query-input":"required name=search_term_string"}}
            </script></head><body></body></html>"#;
        assert_eq!(detect_search(&site("shop.example"), html).as_deref(), Some("https://www.shop.example/search?q={q}"));

        let nested = r#"<script type="application/ld+json">{"@graph":[{"@type":"WebSite",
            "potentialAction":[{"@type":"SearchAction","target":{"@type":"EntryPoint","urlTemplate":"https://shop.example/find?text={query}&x=1"}}]}]}</script>"#;
        assert_eq!(detect_search(&site("shop.example"), nested).as_deref(), Some("https://shop.example/find?text={q}&x=1"));
    }

    #[test]
    fn search_address_from_the_search_box() {
        let html = r#"<html><body>
            <form action="/newsletter" method="post"><input type="email" name="email"></form>
            <form action="/catalogsearch/result/" method="get">
              <input type="hidden" name="type" value="product"><input type="search" name="terms" placeholder="Search">
            </form></body></html>"#;
        assert_eq!(
            detect_search(&site("https://www.shop.example/some/page"), html).as_deref(),
            Some("https://www.shop.example/catalogsearch/result/?type=product&terms={q}")
        );
        let shopify = r#"<html><head><link href="https://cdn.shopify.com/s/files/theme.css"></head></html>"#;
        assert_eq!(
            detect_search(&site("shop.example"), shopify).as_deref(),
            Some("https://shop.example/search?type=product&q={q}")
        );
        assert_eq!(detect_search(&site("shop.example"), "<html><body>Nothing here</body></html>"), None);
    }

    #[test]
    fn typed_search_addresses() {
        let shop = site("shop.example");
        assert_eq!(normalise_search(&shop, "https://shop.example/s?k={q}").unwrap(), "https://shop.example/s?k={q}");
        // A results page for the test word works too, whatever its capitals.
        assert_eq!(
            normalise_search(&shop, "https://www.shop.example/search?q=PriceTrack&sort=best").unwrap(),
            "https://www.shop.example/search?q={q}&sort=best"
        );
        assert_eq!(
            normalise_search(&shop, "www.shop.example/pricetrack/results").unwrap(),
            "https://www.shop.example/{q}/results"
        );
        assert!(normalise_search(&shop, "https://shop.example/search?q=shoes").is_err(), "no place for the words");
        assert!(normalise_search(&shop, "https://elsewhere.example/search?q={q}").is_err(), "another site");
        assert!(normalise_search(&shop, "http://192.168.1.5/search?q={q}").is_err());
    }

    #[test]
    fn shop_name_from_its_front_page() {
        let html = r#"<html><head><meta property="og:site_name" content=" Box  Lunch "></head></html>"#;
        let shop = site("www.boxlunch.com");
        assert_eq!(detect_name(&shop, html).as_deref(), Some("Box Lunch"));
        let titled = "<html><head><title>Pop Culture Gifts &amp; Merch | BoxLunch</title></head></html>";
        assert_eq!(detect_name(&shop, titled).as_deref(), Some("BoxLunch"));
        let wordy = "<html><head><title>BoxLunch Gifts | The Fandom Merch &amp; Gifts Shop</title></head></html>";
        assert_eq!(detect_name(&shop, wordy).as_deref(), Some("BoxLunch"));
        let spaced = "<html><head><title>Welcome to Hot Topic: Band Tees</title></head></html>";
        assert_eq!(detect_name(&site("hottopic.com"), spaced).as_deref(), Some("Hot Topic"));
        assert_eq!(detect_name(&shop, "<html><head><title>Welcome to our shop</title></head></html>"), None);
        assert_eq!(detect_name(&shop, "<html></html>"), None);
    }

    #[test]
    fn pages_that_ask_to_sign_in() {
        for asking in [
            r#"<span class="nav-line-1">Hello, sign in</span>"#,
            r#"<a href="/account/login"><div>Sign In</div><div>Account</div></a>"#,
            r#"<button type="button" aria-label="Sign in">"#,
            "<a href=\"/login\">\n   Log in / Register </a>",
            "<a>Sign in or create account</a>",
            "<button>Sign In &amp; Earn Rewards</button>",
        ] {
            assert!(shows_sign_in_prompt(asking), "{asking}");
        }
        for not_asking in [
            r#"<span class="nav-line-1">Hello, Sam</span><a href="/signout">Sign out</a>"#,
            "<p>Sign in to your account to see the member price on this item today.</p>",
            r#"<script>{"signInText":"Sign in","url":"/signin"}</script>"#,
            "<a>Design ink refills</a>",
        ] {
            assert!(!shows_sign_in_prompt(not_asking), "{not_asking}");
        }
    }

    #[tokio::test]
    async fn a_sign_in_that_lapses_is_announced_once_to_the_person_who_signed_in() {
        let app = crate::test_app().await;
        let sam = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        let other = db::create_user(&app.db, "alex", "h", true).await.unwrap();
        let asks = r#"<html><a href="/login">Sign In</a><h1>Chair</h1></html>"#;
        let greets = r#"<html><a href="/account">Hi, Sam</a><h1>Chair</h1></html>"#;
        let titles = |user: i64| {
            let db = app.db.clone();
            async move { db::notifications(&db, user, 0, 20).await.unwrap().into_iter().map(|n| n.title).collect::<Vec<_>>() }
        };
        let state = || async {
            let setup = browser_setup(&app.db, "staples").await.unwrap();
            sign_in_state(&app.db, "staples", &setup).await.unwrap()
        };

        // Before anyone signs in, the server learns what a signed-out page looks like.
        note_reading(&app, "staples", FetchMode::Browser, asks).await.unwrap();
        assert_eq!(state().await, None);

        let signed_in = BrowserSetup { signed_in_by: Some("sam".into()), signed_in_at: Some(db::now()), ..Default::default() };
        save_browser_setup(&app, "staples", &signed_in).await.unwrap();
        assert_eq!(state().await, Some(SignIn::NotReadYet));
        note_reading(&app, "staples", FetchMode::Browser, greets).await.unwrap();
        assert_eq!(state().await, Some(SignIn::Holds));

        // One page asking to sign in could be a fluke; two in a row is announced, once.
        note_reading(&app, "staples", FetchMode::Browser, asks).await.unwrap();
        assert_eq!(state().await, Some(SignIn::Holds));
        assert!(titles(sam).await.is_empty());
        note_reading(&app, "staples", FetchMode::Browser, asks).await.unwrap();
        note_reading(&app, "staples", FetchMode::Browser, asks).await.unwrap();
        assert!(matches!(state().await, Some(SignIn::Lapsed(_))));
        assert_eq!(titles(sam).await, vec!["Staples looks signed out in the server's browser"]);
        assert!(titles(other).await.is_empty(), "only the person who signed in is told");

        // Signing in again starts over.
        save_browser_setup(&app, "staples", &signed_in).await.unwrap();
        note_reading(&app, "staples", FetchMode::Browser, greets).await.unwrap();
        assert_eq!(state().await, Some(SignIn::Holds));
        assert_eq!(titles(sam).await.len(), 1);
    }

    #[tokio::test]
    async fn a_store_whose_pages_always_ask_to_sign_in_raises_no_alarm() {
        let app = crate::test_app().await;
        let sam = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        let asks = r#"<html><a href="/login">Sign In</a></html>"#;
        let signed_in = BrowserSetup { signed_in_by: Some("sam".into()), signed_in_at: Some(db::now()), ..Default::default() };
        save_browser_setup(&app, "kohls", &signed_in).await.unwrap();
        for _ in 0..4 {
            note_reading(&app, "kohls", FetchMode::Browser, asks).await.unwrap();
        }
        let setup = browser_setup(&app.db, "kohls").await.unwrap();
        assert_eq!(sign_in_state(&app.db, "kohls", &setup).await.unwrap(), Some(SignIn::CannotTell));
        assert!(db::notifications(&app.db, sam, 0, 20).await.unwrap().is_empty());

        // A page fetched with a plain request says nothing about a sign-in in the browser.
        save_browser_setup(&app, "walmart", &signed_in).await.unwrap();
        note_reading(&app, "walmart", FetchMode::Http, asks).await.unwrap();
        let setup = browser_setup(&app.db, "walmart").await.unwrap();
        assert_eq!(sign_in_state(&app.db, "walmart", &setup).await.unwrap(), Some(SignIn::NotReadYet));
    }

    #[test]
    fn zip_codes() {
        assert_eq!(parse_zip(" 55403 ").as_deref(), Some("55403"));
        assert_eq!(parse_zip("55403-1234").as_deref(), Some("55403"));
        for bad in ["5540", "554033", "abcde", "55403-12", "123 Main St", "\u{ff15}5403"] {
            assert_eq!(parse_zip(bad), None, "{bad}");
        }
    }

    #[tokio::test]
    async fn adding_a_store_with_a_typed_search_address() {
        let app = crate::test_app().await;
        let user = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        let new =
            NewStore { site: "https://www.boxlunch.example/", name: "BoxLunch", search: "boxlunch.example/search?q=pricetrack" };
        assert_eq!(add(&app, user, new).await.unwrap(), "BoxLunch");
        let stores = db::custom_stores(&app.db).await.unwrap();
        assert_eq!(stores.len(), 1);
        assert_eq!(
            (stores[0].retailer.as_str(), stores[0].search_url.as_str()),
            ("boxlunch.example", "https://boxlunch.example/search?q={q}")
        );
        assert_eq!(retailers::display_name("boxlunch.example"), "BoxLunch");

        // A built-in store is not added twice, and local addresses are refused.
        let again = NewStore { site: "www.walmart.com", name: "", search: "" };
        assert!(matches!(add(&app, user, again).await, Err(Problem::User(_))));
        let local = NewStore { site: "http://192.168.1.20", name: "NAS", search: "http://192.168.1.20/s?q={q}" };
        assert!(matches!(add(&app, user, local).await, Err(Problem::User(_))));

        assert_eq!(remove(&app, stores[0].id).await.unwrap().as_deref(), Some("BoxLunch"));
        assert!(db::custom_stores(&app.db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_store_walmart_priced_from_is_remembered_from_the_servers_own_readings() {
        use crate::retailers::{LocalStore, Observation, ProductRef};
        let app = crate::test_app().await;
        let reference = ProductRef { retailer: "walmart".into(), sku: "1".into(), url: "https://www.walmart.com/ip/1".into() };
        let id = db::upsert_product(&app.db, &reference).await.unwrap();
        let seen = |store: &str| Observation {
            price_cents: Some(347),
            local_store: Some(LocalStore { id: store.into(), place: "Springfield, IL 62704".into() }),
            ..Default::default()
        };
        assert_eq!(location(&app.db).await.unwrap().walmart_store, None);
        db::record_observation(&app.db, id, &seen("1234"), "server", db::Audience::Public, None, None).await.unwrap();
        assert_eq!(location(&app.db).await.unwrap().walmart_store.unwrap().id, "1234");
        // A phone's reading comes from the phone's store, not the server's.
        db::record_observation(&app.db, id, &seen("9999"), "device", db::Audience::Public, None, None).await.unwrap();
        assert_eq!(location(&app.db).await.unwrap().walmart_store.unwrap().id, "1234");
    }

    #[test]
    fn target_store_finder_answer() {
        let body = r#"{"data":{"nearby_stores":{"stores":[
            {"store_id":"1375","location_name":"Minneapolis Nicollet Mall","distance":0.4,
             "mailing_address":{"city":"Minneapolis","region":"MN","postal_code":"55403"}},
            {"store_id":2189,"location_name":"Minneapolis Uptown","mailing_address":{"city":"Minneapolis"}}]}}}"#;
        let stores = crate::fetch::parse_target_stores(body);
        assert_eq!(stores.len(), 2);
        assert_eq!(
            stores[0],
            NearbyStore {
                id: "1375".into(),
                name: "Minneapolis Nicollet Mall".into(),
                place: "Minneapolis, MN".into(),
                miles: Some(0.4)
            }
        );
        assert_eq!((stores[1].id.as_str(), stores[1].place.as_str(), stores[1].miles), ("2189", "Minneapolis", None));
        assert!(crate::fetch::parse_target_stores("<html>Access Denied</html>").is_empty());
    }
}
