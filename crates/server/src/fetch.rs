//! Gets a product's current price from its store, choosing between a plain
//! web request and the real browser, and keeping requests to each store slow.

use crate::browser::Browser;
use crate::config::Config;
use crate::retailers::{self, FetchMode, Observation, ParseError};
use regex::Regex;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

/// Shortest gap between two requests to the same store.
const PER_STORE_GAP: Duration = Duration::from_secs(8);

/// The key Target's own website uses for its product data. It is public and
/// occasionally changes, so a fresh one is read from target.com when it stops working.
const TARGET_DEFAULT_KEY: &str = "9f36aeafbe60771e321a7cc95a78140772ab3e96";
static RE_TARGET_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"apiKey\\?"\s*:\s*\\?"([0-9a-f]{40})\\?""#).unwrap());

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("the store refused the request ({0})")]
    Blocked(String),
    #[error("no price was found on the page")]
    NoPrice,
    #[error("this store only answers a real browser, and none is set up on the server")]
    NeedsBrowser,
    #[error("the store says this product page no longer exists")]
    Gone,
    #[error("{0}")]
    Other(String),
}

impl From<ParseError> for FetchError {
    fn from(e: ParseError) -> Self {
        match e {
            ParseError::Blocked(why) => FetchError::Blocked(why),
            ParseError::NoPrice => FetchError::NoPrice,
        }
    }
}

pub struct Fetched {
    pub obs: Observation,
    pub mode: FetchMode,
    /// The page the reading came from, when it was a web page. Its links
    /// lead to the product's other colours and sizes.
    pub html: Option<String>,
}

/// A store of a chain near some place.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NearbyStore {
    pub id: String,
    pub name: String,
    /// "City, ST"
    pub place: String,
    pub miles: Option<f64>,
}

/// Reads the stores out of Target's "stores near" answer.
pub fn parse_target_stores(body: &str) -> Vec<NearbyStore> {
    let Ok(data) = serde_json::from_str::<serde_json::Value>(body) else {
        return vec![];
    };
    let Some(stores) = data.pointer("/data/nearby_stores/stores").and_then(|v| v.as_array()) else {
        return vec![];
    };
    stores
        .iter()
        .filter_map(|s| {
            let id = match s.get("store_id")? {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                _ => return None,
            };
            let text = |pointer: &str| s.pointer(pointer).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let (city, region) = (text("/mailing_address/city"), text("/mailing_address/region"));
            Some(NearbyStore {
                id,
                name: text("/location_name"),
                place: [city, region].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(", "),
                miles: s.get("distance").and_then(|v| v.as_f64()),
            })
        })
        .collect()
}

pub struct Fetcher {
    http: reqwest::Client,
    browser: Option<Arc<Browser>>,
    /// The Target store whose prices are read. Changes when the server's location is set.
    target_store_id: std::sync::RwLock<String>,
    target_key: Mutex<String>,
    last_request: Mutex<HashMap<String, Instant>>,
    /// Stores that answer plain requests but are read with the browser
    /// anyway, because a person signed in or chose their local store there.
    browser_first: std::sync::RwLock<std::collections::HashSet<String>>,
}

impl Fetcher {
    pub fn new(cfg: &Config) -> anyhow::Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::ACCEPT, "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".parse()?);
        headers.insert(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9".parse()?);
        // Stores are public websites: a name that resolves to the home network, or a
        // redirect to it, is never followed.
        let http = crate::net::public_only(reqwest::Client::builder())
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(12))
            .build()?;
        let browser = cfg.browser_path.clone().map(|exe| {
            Arc::new(Browser::new(exe, cfg.data_dir.join("browser-profile"), cfg.browser_port, cfg.browser_args.clone()))
        });
        Ok(Fetcher {
            http,
            browser,
            target_store_id: std::sync::RwLock::new(cfg.target_store_id.clone()),
            target_key: Mutex::new(TARGET_DEFAULT_KEY.to_string()),
            last_request: Mutex::new(HashMap::new()),
            browser_first: std::sync::RwLock::new(std::collections::HashSet::new()),
        })
    }

    pub fn has_browser(&self) -> bool {
        self.browser.is_some()
    }

    pub fn target_store(&self) -> String {
        self.target_store_id.read().map(|id| id.clone()).unwrap_or_default()
    }

    /// Reads Target prices from this store from now on.
    pub fn set_target_store(&self, id: &str) {
        if let Ok(mut current) = self.target_store_id.write() {
            *current = id.to_string();
        }
    }

    /// Target stores near a ZIP code, nearest first.
    pub async fn target_stores_near(&self, zip: &str) -> Result<Vec<NearbyStore>, FetchError> {
        let key = self.target_key.lock().await.clone();
        self.take_turn("target").await;
        let api = format!(
            "https://redsky.target.com/redsky_aggregations/v1/web/nearby_stores_v1?limit=5&within=100&place={zip}&key={key}&channel=WEB"
        );
        let body = self.get_text(&api).await?;
        Ok(parse_target_stores(&body))
    }

    pub fn browser(&self) -> Option<&Arc<Browser>> {
        self.browser.as_ref()
    }

    /// Replaces the list of stores read with the browser first.
    pub fn set_browser_first(&self, stores: impl IntoIterator<Item = String>) {
        if let Ok(mut current) = self.browser_first.write() {
            *current = stores.into_iter().collect();
        }
    }

    /// The readers to try for a store, in order.
    fn reading_order(&self, retailer: &str, remembered: Option<FetchMode>) -> Vec<FetchMode> {
        let known_store = retailers::by_id(retailer).is_some();
        let preferred = retailers::default_mode(retailer);
        if known_store && preferred == FetchMode::Browser {
            // These stores are known to reject plain requests; do not bother them with one.
            return vec![FetchMode::Browser];
        }
        // The browser holds the sign-in or the chosen local store; a plain request is the fallback.
        if self.browser.is_some() && self.browser_first.read().is_ok_and(|stores| stores.contains(retailer)) {
            return vec![FetchMode::Browser, FetchMode::Http];
        }
        let first = remembered.unwrap_or(preferred);
        let second = if first == FetchMode::Http { FetchMode::Browser } else { FetchMode::Http };
        vec![first, second]
    }

    /// Waits until this store may be contacted again, then marks it as contacted.
    async fn take_turn(&self, store: &str) {
        let wait = {
            let mut last = self.last_request.lock().await;
            let now = Instant::now();
            let ready_at = last.get(store).map(|t| *t + PER_STORE_GAP).filter(|t| *t > now).unwrap_or(now);
            last.insert(store.to_string(), ready_at);
            ready_at - now
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// Reads the current price of one product.
    ///
    /// `remembered` is the reader that worked last time for this product, if any.
    pub async fn fetch(
        &self,
        retailer: &str,
        sku: &str,
        url: &str,
        remembered: Option<FetchMode>,
    ) -> Result<Fetched, FetchError> {
        let order = self.reading_order(retailer, remembered);

        let mut first_error = None;
        for mode in order {
            if mode == FetchMode::Browser && self.browser.is_none() {
                first_error.get_or_insert(FetchError::NeedsBrowser);
                continue;
            }
            self.take_turn(retailer).await;
            let result = match mode {
                FetchMode::Http => self.fetch_http(retailer, sku, url).await,
                FetchMode::Browser => self.fetch_browser(retailer, url).await,
            };
            match result {
                Ok((obs, html)) => return Ok(Fetched { obs, mode, html }),
                // The product is gone whichever way we ask.
                Err(FetchError::Gone) => return Err(FetchError::Gone),
                Err(e) => {
                    tracing::debug!(retailer, url, mode = mode.as_str(), error = %e, "fetch attempt failed");
                    // Report the first, most informative failure unless it was only "no browser".
                    match &first_error {
                        None | Some(FetchError::NeedsBrowser) => first_error = Some(e),
                        Some(_) => {}
                    }
                }
            }
        }
        Err(first_error.unwrap_or(FetchError::NeedsBrowser))
    }

    async fn fetch_browser(&self, retailer: &str, url: &str) -> Result<(Observation, Option<String>), FetchError> {
        let browser = self.browser.as_ref().ok_or(FetchError::NeedsBrowser)?;
        Self::public_only(url).await?;
        // Some stores refuse the very first visit from a browser they have not
        // seen, or show a one-time welcome page, then behave on the next
        // visit once their cookies are set. So a failed read is tried twice.
        let mut last_error = FetchError::NoPrice;
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            let html = browser.fetch_html(url).await.map_err(|e| FetchError::Other(format!("{e:#}")))?;
            match retailers::parse(retailer, &html) {
                Ok(obs) => return Ok((obs, Some(html))),
                Err(e) => {
                    self.keep_failed_page(retailer, &html);
                    last_error = e.into();
                }
            }
        }
        Err(last_error)
    }

    /// Loads any page of a store (a search results page, say) and returns
    /// its HTML, with the same pacing as price checks.
    pub async fn page(&self, retailer: &str, url: &str, mode: FetchMode) -> Result<String, FetchError> {
        match mode {
            FetchMode::Http => {
                self.take_turn(retailer).await;
                let html = self.get_text(url).await?;
                match retailers::blocked_reason(&html) {
                    Some(why) => Err(FetchError::Blocked(why.to_string())),
                    None => Ok(html),
                }
            }
            FetchMode::Browser => {
                let browser = self.browser.as_ref().ok_or(FetchError::NeedsBrowser)?;
                Self::public_only(url).await?;
                let mut refusal = String::new();
                // As with product pages, a first refusal often clears on the second visit.
                for attempt in 0..2 {
                    if attempt > 0 {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                    self.take_turn(retailer).await;
                    let html = browser.fetch_html(url).await.map_err(|e| FetchError::Other(format!("{e:#}")))?;
                    match retailers::blocked_reason(&html) {
                        Some(why) => {
                            self.keep_failed_page(retailer, &html);
                            refusal = why.to_string();
                        }
                        None => return Ok(html),
                    }
                }
                Err(FetchError::Blocked(refusal))
            }
        }
    }

    /// The browser looks names up by itself, so where it is sent is checked first.
    async fn public_only(url: &str) -> Result<(), FetchError> {
        if crate::net::resolves_public(url).await {
            Ok(())
        } else {
            Err(FetchError::Other("that address is not a website on the public internet".into()))
        }
    }

    /// When `APT_DEBUG_PAGES` names a folder, the last page that could not be
    /// read is saved there per store, to see what the store actually sent.
    fn keep_failed_page(&self, retailer: &str, html: &str) {
        let Some(dir) = std::env::var_os("APT_DEBUG_PAGES") else {
            return;
        };
        let name: String = retailer.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        let path = std::path::Path::new(&dir).join(format!("{name}.html"));
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, html)) {
            tracing::warn!("could not save the unreadable page to {}: {e}", path.display());
        } else {
            tracing::info!("saved the unreadable {retailer} page to {}", path.display());
        }
    }

    async fn fetch_http(&self, retailer: &str, sku: &str, url: &str) -> Result<(Observation, Option<String>), FetchError> {
        if retailer == "target" {
            return Ok((self.fetch_target(sku).await?, None));
        }
        let body = self.get_text(url).await?;
        Ok((retailers::parse(retailer, &body)?, Some(body)))
    }

    async fn get_text(&self, url: &str) -> Result<String, FetchError> {
        let resp = self.http.get(url).send().await.map_err(|e| {
            if e.is_timeout() {
                FetchError::Blocked("no answer".into())
            } else {
                FetchError::Other(format!("could not reach the store: {}", root_cause(&e)))
            }
        })?;
        let status = resp.status();
        match status.as_u16() {
            404 | 410 => return Err(FetchError::Gone),
            401 | 403 | 429 | 503 => return Err(FetchError::Blocked(format!("HTTP {}", status.as_u16()))),
            _ if !status.is_success() => return Err(FetchError::Other(format!("the store answered HTTP {}", status.as_u16()))),
            _ => {}
        }
        resp.text().await.map_err(|e| FetchError::Other(format!("could not read the page: {}", root_cause(&e))))
    }

    async fn fetch_target(&self, tcin: &str) -> Result<Observation, FetchError> {
        let mut key = self.target_key.lock().await.clone();
        let store = self.target_store();
        for attempt in 0..2 {
            let api = format!(
                "https://redsky.target.com/redsky_aggregations/v1/web/pdp_client_v1?key={key}&tcin={tcin}&is_bot=false&store_id={store}&pricing_store_id={store}&has_pricing_store_id=true&channel=WEB"
            );
            match self.get_text(&api).await {
                Ok(body) => return Ok(retailers::parse("target", &body)?),
                // A rejected key looks like a refusal; fetch the current key once and retry.
                Err(FetchError::Blocked(_)) | Err(FetchError::Other(_)) if attempt == 0 => {
                    let page = self.get_text(&format!("https://www.target.com/p/-/A-{tcin}")).await?;
                    match RE_TARGET_KEY.captures_iter(&page).map(|c| c[1].to_string()).last() {
                        Some(fresh) if fresh != key => {
                            *self.target_key.lock().await = fresh.clone();
                            key = fresh;
                        }
                        _ => return Err(FetchError::Blocked("Target product data unavailable".into())),
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Err(FetchError::Blocked("Target product data unavailable".into()))
    }

    /// Follows a share-button short link (a.co and friends) to the real product link.
    pub async fn resolve_short_link(&self, url: &str) -> Result<String, FetchError> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| FetchError::Other(format!("could not follow the short link: {}", root_cause(&e))))?;
        Ok(resp.url().to_string())
    }
}

/// The innermost cause of an error, which is usually the readable part.
pub fn root_cause(e: &dyn std::error::Error) -> String {
    let mut cur = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetcher(with_browser: bool) -> Fetcher {
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            data_dir: std::env::temp_dir(),
            public_url: None,
            check_interval: Duration::from_secs(3600),
            // Never started in these tests; only whether one is set up matters.
            browser_path: with_browser.then(|| "chrome".into()),
            browser_port: 0,
            browser_args: vec![],
            target_store_id: "1375".into(),
            secure_cookies: false,
            backup_keep: 0,
            backup_dir: std::env::temp_dir(),
            members_local_network: false,
        };
        Fetcher::new(&cfg).unwrap()
    }

    #[test]
    fn a_store_set_up_in_the_browser_is_read_with_it_first() {
        use FetchMode::{Browser, Http};
        let f = fetcher(true);
        assert_eq!(f.reading_order("walmart", None), vec![Http, Browser]);
        assert_eq!(f.reading_order("staples", None), vec![Browser], "never bothered with a plain request");
        assert_eq!(f.reading_order("shop.example", Some(Browser)), vec![Browser, Http], "what worked last time");

        // Someone chose their local Walmart in the server's browser: the browser first, plain requests as the fallback.
        f.set_browser_first(["walmart".to_string()]);
        assert_eq!(f.reading_order("walmart", Some(Http)), vec![Browser, Http]);
        assert_eq!(f.reading_order("target", None), vec![Http, Browser], "other stores are unchanged");

        // Without a browser on the server the setting cannot apply.
        let bare = fetcher(false);
        bare.set_browser_first(["walmart".to_string()]);
        assert_eq!(bare.reading_order("walmart", None), vec![Http, Browser]);
    }
}
