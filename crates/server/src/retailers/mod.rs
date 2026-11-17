//! Which stores are supported, how a product link maps to a store item,
//! and how a fetched page turns into a price.

mod parsers;

use anyhow::{anyhow, bail, Result};
use regex::Regex;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use url::Url;

pub use parsers::{blocked_reason, parse};

/// How the server reads a store's product page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FetchMode {
    /// A plain web request.
    Http,
    /// A real Chrome window (the store rejects plain requests).
    Browser,
}

impl FetchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            FetchMode::Http => "http",
            FetchMode::Browser => "browser",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "http" => Some(FetchMode::Http),
            "browser" => Some(FetchMode::Browser),
            _ => None,
        }
    }
}

pub struct Retailer {
    pub id: &'static str,
    pub name: &'static str,
    pub domain: &'static str,
    pub mode: FetchMode,
}

pub static RETAILERS: &[Retailer] = &[
    Retailer { id: "staples", name: "Staples", domain: "staples.com", mode: FetchMode::Browser },
    Retailer { id: "bestbuy", name: "Best Buy", domain: "bestbuy.com", mode: FetchMode::Browser },
    Retailer { id: "walmart", name: "Walmart", domain: "walmart.com", mode: FetchMode::Http },
    Retailer { id: "target", name: "Target", domain: "target.com", mode: FetchMode::Http },
    Retailer { id: "amazon", name: "Amazon", domain: "amazon.com", mode: FetchMode::Http },
    Retailer { id: "kohls", name: "Kohl's", domain: "kohls.com", mode: FetchMode::Browser },
    Retailer { id: "harborfreight", name: "Harbor Freight", domain: "harborfreight.com", mode: FetchMode::Browser },
    Retailer { id: "hottopic", name: "Hot Topic", domain: "hottopic.com", mode: FetchMode::Http },
];

/// Names people gave the stores they added, by site name.
static ADDED_NAMES: LazyLock<RwLock<HashMap<String, String>>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// Replaces the list of added stores' names (site name, display name).
pub fn set_added_names(names: impl IntoIterator<Item = (String, String)>) {
    if let Ok(mut map) = ADDED_NAMES.write() {
        *map = names.into_iter().collect();
    }
}

pub fn by_id(id: &str) -> Option<&'static Retailer> {
    RETAILERS.iter().find(|r| r.id == id)
}

/// Display name for a retailer id. Stores without a dedicated reader use
/// their site name as the id; that is shown unless someone named the store.
pub fn display_name(id: &str) -> String {
    if let Some(retailer) = by_id(id) {
        return retailer.name.to_string();
    }
    ADDED_NAMES.read().ok().and_then(|names| names.get(id).cloned()).unwrap_or_else(|| id.to_string())
}

/// Checks that an address typed by a person names a public website, and
/// returns it parsed. Addresses of machines on the local network are refused.
pub fn public_site(raw: &str) -> Result<Url> {
    let raw = raw.trim();
    let raw = if raw.contains("://") { raw.to_string() } else { format!("https://{raw}") };
    let url = Url::parse(&raw).map_err(|_| anyhow!("That does not look like a web address."))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("Only http and https addresses can be used.");
    }
    let host = url.host_str().ok_or_else(|| anyhow!("That address has no site name."))?.to_ascii_lowercase();
    if !matches!(url.host(), Some(url::Host::Domain(_)))
        || !host.contains('.')
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".lan")
    {
        bail!("Only public store websites can be used.");
    }
    Ok(url)
}

/// The site name used as the retailer id for a shop without a dedicated reader.
pub fn site_name(url: &Url) -> String {
    url.host_str().unwrap_or("").to_ascii_lowercase().trim_start_matches("www.").to_string()
}

/// The reader tried first for a retailer id.
pub fn default_mode(id: &str) -> FetchMode {
    by_id(id).map(|r| r.mode).unwrap_or(FetchMode::Http)
}

/// A product link resolved to a store and that store's item number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductRef {
    pub retailer: String,
    pub sku: String,
    /// Clean link without tracking parameters.
    pub url: String,
}

/// Link shorteners used by store apps' share buttons. These have to be
/// followed before the product can be identified.
pub fn is_short_link(url: &Url) -> bool {
    matches!(
        url.host_str().unwrap_or("").trim_start_matches("www."),
        "a.co" | "amzn.to" | "amzn.com" | "bby.me" | "goto.walmart.com" | "goto.target.com"
    )
}

static RE_STAPLES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/product_([A-Za-z0-9]+)").unwrap());
static RE_BESTBUY_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(?:sku/(\d{5,10})|(\d{5,10})\.p)").unwrap());
static RE_WALMART: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/ip/(?:[^/]+/)?(\d{4,})").unwrap());
static RE_TARGET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/A-(\d{6,})").unwrap());
static RE_AMAZON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/(?:dp|gp/product|gp/aw/d|product)/([A-Z0-9]{10})(?:[/?]|$)").unwrap());
static RE_KOHLS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/product/prd-([A-Za-z0-9]+)").unwrap());
static RE_HARBOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-(\d{5,6})\.html$").unwrap());
static RE_HOTTOPIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/product/(?:[^/]+/)?(\d{6,10})\.html$").unwrap());

/// Works out which store and item a pasted link points at.
pub fn identify(raw: &str) -> Result<ProductRef> {
    let raw = raw.trim();
    let raw = if raw.contains("://") { raw.to_string() } else { format!("https://{raw}") };
    let url = Url::parse(&raw).map_err(|_| anyhow!("That does not look like a web link."))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("Only http and https links can be tracked.");
    }
    let host = url.host_str().ok_or_else(|| anyhow!("That link has no site name."))?.to_ascii_lowercase();
    // Only public site names: no IP addresses, no machines on the local network.
    if !matches!(url.host(), Some(url::Host::Domain(_)))
        || !host.contains('.')
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".lan")
    {
        bail!("Only links to public store websites can be tracked.");
    }
    let path = url.path();
    let on = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    let cap = |re: &Regex| re.captures(path).and_then(|c| c.iter().skip(1).flatten().next()).map(|m| m.as_str().to_string());
    let missing = |store: &str| anyhow!("That {store} link is not a product page. Open the product and copy its link.");
    let make = |retailer: &str, sku: String, url: String| Ok(ProductRef { retailer: retailer.to_string(), sku, url });

    if on("staples.com") {
        let sku = cap(&RE_STAPLES).ok_or_else(|| missing("Staples"))?;
        return make("staples", sku, format!("https://www.staples.com{path}"));
    }
    if on("bestbuy.com") {
        let sku = url
            .query_pairs()
            .find(|(k, _)| k == "skuId")
            .map(|(_, v)| v.to_string())
            .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()))
            .or_else(|| cap(&RE_BESTBUY_PATH))
            .ok_or_else(|| missing("Best Buy"))?;
        let url = if let Some((before, _)) = path.split_once("/sku/") {
            // Drop anything after the item number (such as /reviews).
            format!("https://www.bestbuy.com{before}/sku/{sku}")
        } else {
            format!("https://www.bestbuy.com{path}?skuId={sku}")
        };
        return make("bestbuy", sku, url);
    }
    if on("walmart.com") {
        let sku = cap(&RE_WALMART).ok_or_else(|| missing("Walmart"))?;
        return make("walmart", sku.clone(), format!("https://www.walmart.com/ip/{sku}"));
    }
    if on("target.com") {
        let sku = cap(&RE_TARGET).ok_or_else(|| missing("Target"))?;
        return make("target", sku.clone(), format!("https://www.target.com/p/-/A-{sku}"));
    }
    if on("amazon.com") {
        let sku = cap(&RE_AMAZON).ok_or_else(|| missing("Amazon"))?;
        return make("amazon", sku.clone(), format!("https://www.amazon.com/dp/{sku}"));
    }
    if on("kohls.com") {
        let sku = cap(&RE_KOHLS).ok_or_else(|| missing("Kohl's"))?;
        return make("kohls", sku, format!("https://www.kohls.com{path}"));
    }
    if on("harborfreight.com") {
        let sku = cap(&RE_HARBOR).ok_or_else(|| missing("Harbor Freight"))?;
        return make("harborfreight", sku, format!("https://www.harborfreight.com{path}"));
    }

    if on("hottopic.com") {
        let sku = cap(&RE_HOTTOPIC).ok_or_else(|| missing("Hot Topic"))?;
        return make("hottopic", sku, format!("https://www.hottopic.com{path}"));
    }

    // Any other store: tracked through the product data most shop pages publish.
    if path.len() <= 1 {
        bail!("That link is a store's front page. Open a product and copy its link.");
    }
    let site = host.trim_start_matches("www.").to_string();
    // The query is kept because some shops put the product number there.
    let mut clean = url.clone();
    clean.set_fragment(None);
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| {
            let k = k.to_ascii_lowercase();
            !(k.starts_with("utm_") || matches!(k.as_str(), "gclid" | "fbclid" | "msclkid" | "ref" | "srsltid"))
        })
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    if kept.is_empty() {
        clean.set_query(None);
    } else {
        clean.query_pairs_mut().clear().extend_pairs(kept);
    }
    let sku = match clean.query() {
        Some(q) => format!("{}?{}", clean.path(), q),
        None => clean.path().to_string(),
    };
    make(&site, sku, clean.to_string())
}

/// What one look at a product page told us. Every field is optional because
/// stores publish different amounts of detail.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Observation {
    pub title: Option<String>,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub gtin: Option<String>,
    pub image_url: Option<String>,
    pub price_cents: Option<i64>,
    pub list_price_cents: Option<i64>,
    pub in_stock: Option<bool>,
    pub promo_text: Option<String>,
    /// Barcodes of the listing's other options (colours, sizes) when the
    /// page covers several. Used to recognise the product at another store.
    pub other_gtins: Vec<String>,
    /// The listing's other colours or sizes that have a page of their own.
    pub variants: Vec<Variant>,
    /// Colour, size, pack count and a line of description, as plain text.
    pub details: Option<String>,
    /// The local store this price is for, at chains that price per store
    /// and say which store they used.
    pub local_store: Option<LocalStore>,
    /// Who sells the item, at stores that host other sellers.
    pub seller: Option<String>,
    /// True when that seller is not the store itself.
    pub third_party: Option<bool>,
    /// True when the store itself ships the item, whoever sells it.
    pub store_ships: Option<bool>,
    /// True when the page says delivery is free, false when it is charged.
    pub ships_free: Option<bool>,
    /// The delivery charge, when the page states one.
    pub shipping_cents: Option<i64>,
}

/// The local store a chain used for a price.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LocalStore {
    /// The chain's own store number.
    pub id: String,
    /// "City, ST 12345", as far as the page says.
    pub place: String,
}

/// Another colour or size of a product, sold as its own item.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Variant {
    pub sku: String,
    /// What sets it apart, e.g. "Blue".
    pub label: String,
}

/// The address of a store's product page, given only its item number.
pub fn product_url(retailer: &str, sku: &str) -> Option<String> {
    match retailer {
        "bestbuy" => Some(format!("https://www.bestbuy.com/site/{sku}.p?skuId={sku}")),
        "walmart" => Some(format!("https://www.walmart.com/ip/{sku}")),
        "target" => Some(format!("https://www.target.com/p/-/A-{sku}")),
        "amazon" => Some(format!("https://www.amazon.com/dp/{sku}")),
        _ => None,
    }
}

/// Why a page could not be turned into a price.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// The store answered with a "prove you are human" or "access denied" page.
    #[error("the store refused the request ({0})")]
    Blocked(String),
    #[error("no price was found on the page")]
    NoPrice,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(url: &str) -> (String, String, String) {
        let p = identify(url).unwrap();
        (p.retailer, p.sku, p.url)
    }

    #[test]
    fn identifies_each_store() {
        assert_eq!(
            id("https://www.staples.com/hammermill-copy-paper/product_122374?akamai=1"),
            ("staples".into(), "122374".into(), "https://www.staples.com/hammermill-copy-paper/product_122374".into())
        );
        assert_eq!(id("https://www.bestbuy.com/site/apple-airpods-4/6447385.p?skuId=6447385&intl=nosplash").1, "6447385");
        assert_eq!(
            id("https://www.bestbuy.com/product/apple-airpods-4-white/JJGCQ832YX/sku/10190061"),
            (
                "bestbuy".into(),
                "10190061".into(),
                "https://www.bestbuy.com/product/apple-airpods-4-white/JJGCQ832YX/sku/10190061".into()
            )
        );
        assert_eq!(
            id("walmart.com/ip/AirPods-Max-2-Blue/19864722287?athbdg=L1600"),
            ("walmart".into(), "19864722287".into(), "https://www.walmart.com/ip/19864722287".into())
        );
        assert_eq!(id("https://www.walmart.com/ip/19864722287").1, "19864722287");
        assert_eq!(
            id("https://www.target.com/p/airpods-pro/-/A-85978622#lnk=sametab"),
            ("target".into(), "85978622".into(), "https://www.target.com/p/-/A-85978622".into())
        );
        assert_eq!(
            id("https://www.amazon.com/Apple-AirPods/dp/B0D1XD1ZV3/ref=sr_1_1?crid=1"),
            ("amazon".into(), "B0D1XD1ZV3".into(), "https://www.amazon.com/dp/B0D1XD1ZV3".into())
        );
        assert_eq!(id("https://smile.amazon.com/gp/product/B0D1XD1ZV3").1, "B0D1XD1ZV3");
        assert_eq!(
            id("https://www.kohls.com/product/prd-2849015/towel-set.jsp?color=Blue"),
            ("kohls".into(), "2849015".into(), "https://www.kohls.com/product/prd-2849015/towel-set.jsp".into())
        );
        assert_eq!(id("https://www.harborfreight.com/click-torque-wrench-63882.html").1, "63882");
        assert_eq!(
            id("https://www.hottopic.com/product/funko-pop-angel-vinyl-figure/36447708.html?cgid=funko#reviews"),
            (
                "hottopic".into(),
                "36447708".into(),
                "https://www.hottopic.com/product/funko-pop-angel-vinyl-figure/36447708.html".into()
            )
        );
        assert!(identify("https://www.hottopic.com/toys-collectibles/funko/").is_err());
        assert_eq!(
            id("https://www.bestbuy.com/product/apple-airpods-max/JJ8ZHPWTJL/sku/12922275/reviews").2,
            "https://www.bestbuy.com/product/apple-airpods-max/JJ8ZHPWTJL/sku/12922275"
        );
        // An address built from an item number alone is recognised as that same item.
        for (store, sku) in [("bestbuy", "6550083"), ("walmart", "19864722287"), ("target", "85978622"), ("amazon", "B0D1XD1ZV3")]
        {
            let built = product_url(store, sku).unwrap();
            assert_eq!((id(&built).0.as_str(), id(&built).1.as_str()), (store, sku));
        }
    }

    #[test]
    fn other_stores_use_their_domain() {
        let p = identify("https://www.newegg.com/p/N82E16819113771?utm_source=x&Item=N82E16819113771").unwrap();
        assert_eq!(p.retailer, "newegg.com");
        assert_eq!(p.url, "https://www.newegg.com/p/N82E16819113771?Item=N82E16819113771");
    }

    #[test]
    fn added_stores_show_the_name_they_were_given() {
        assert_eq!(display_name("walmart"), "Walmart");
        assert_eq!(display_name("unnamed-shop.example"), "unnamed-shop.example");
        set_added_names([("boxlunch.com".to_string(), "BoxLunch".to_string())]);
        assert_eq!(display_name("boxlunch.com"), "BoxLunch");
        assert!(public_site("http://192.168.1.10/shop").is_err());
        assert!(public_site("nas.local").is_err());
        assert_eq!(site_name(&public_site("WWW.BoxLunch.com/search").unwrap()), "boxlunch.com");
    }

    #[test]
    fn rejects_non_products_and_private_hosts() {
        assert!(identify("https://www.walmart.com/search?q=airpods").is_err());
        assert!(identify("https://www.amazon.com/").is_err());
        assert!(identify("http://192.168.1.1/admin").is_err());
        assert!(identify("http://localhost/x").is_err());
        assert!(identify("ftp://example.com/x").is_err());
        assert!(identify("https://example.com/").is_err());
    }
}
