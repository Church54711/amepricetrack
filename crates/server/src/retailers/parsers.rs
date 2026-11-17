//! Turns a fetched product page into an [`Observation`].
//!
//! Most stores publish schema.org product data (JSON-LD) in the page, so that
//! is the common path. Stores that keep the price somewhere else get a few
//! extra lines of their own.

use super::{LocalStore, Observation, ParseError, Variant};
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;
use std::sync::LazyLock;

static RE_LD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<script[^>]*type\s*=\s*["']?application/ld\+json["']?[^>]*>(.*?)</script>"#).unwrap());
static RE_TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
static RE_META: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<meta\s[^>]*>").unwrap());
static RE_ATTR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)([a-z:-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap());
static RE_NEXT_DATA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<script[^>]*id="__NEXT_DATA__"[^>]*>(.*?)</script>"#).unwrap());

/// Reads a page (or, for Target, the store's product JSON) for the given retailer.
pub fn parse(retailer: &str, body: &str) -> Result<Observation, ParseError> {
    let looks_like_json = body.trim_start().starts_with('{');
    if !looks_like_json {
        if let Some(reason) = blocked_reason(body) {
            return Err(ParseError::Blocked(reason.to_string()));
        }
    }
    let mut obs = match retailer {
        "walmart" => walmart(body),
        "target" if looks_like_json => target_json(body),
        "amazon" => amazon(body),
        "bestbuy" => bestbuy(body),
        "staples" => staples(body),
        _ => generic(body),
    };

    // Tidy up: drop nonsense values so they never reach the price history.
    if obs.price_cents.is_some_and(|p| p <= 0) {
        obs.price_cents = None;
    }
    if let (Some(list), Some(price)) = (obs.list_price_cents, obs.price_cents) {
        if list <= price {
            obs.list_price_cents = None;
        }
    }
    for field in [&mut obs.title, &mut obs.brand, &mut obs.model, &mut obs.gtin, &mut obs.image_url, &mut obs.seller] {
        if let Some(v) = field {
            let cleaned = decode_entities(v.trim());
            *field = if cleaned.is_empty() { None } else { Some(cleaned) };
        }
    }
    if let Some(img) = &obs.image_url {
        if img.starts_with("//") {
            obs.image_url = Some(format!("https:{img}"));
        } else if !img.starts_with("http") {
            obs.image_url = None;
        }
    }

    // A description that only repeats the name adds nothing.
    if obs.details.is_some() && obs.details == obs.title {
        obs.details = None;
    }

    // A page that plainly says "out of stock" with no price is still a valid reading.
    if obs.price_cents.is_none() && obs.in_stock != Some(false) {
        return Err(ParseError::NoPrice);
    }
    Ok(obs)
}

/// Recognises the pages stores send instead of the product when they suspect a robot.
pub fn blocked_reason(html: &str) -> Option<&'static str> {
    let title = RE_TITLE.captures(html).map(|c| c[1].trim().to_ascii_lowercase()).unwrap_or_default();
    const TITLES: &[(&str, &str)] = &[
        ("access denied", "access denied"),
        ("access to this page has been denied", "access denied"),
        ("robot or human", "robot check"),
        ("are you a human", "robot check"),
        ("just a moment", "robot check"),
        ("attention required", "robot check"),
        ("pardon our interruption", "robot check"),
        ("verify your identity", "robot check"),
    ];
    if let Some((_, reason)) = TITLES.iter().find(|(needle, _)| title.contains(needle)) {
        return Some(reason);
    }
    if html.contains("/errors/validateCaptcha") || html.contains("To discuss automated access to Amazon data") {
        return Some("robot check");
    }
    // Amazon's "click to continue shopping" interstitial is a tiny page.
    if html.len() < 12_000 && title.starts_with("amazon") && html.contains("Continue shopping") {
        return Some("robot check");
    }
    None
}

// ---------------------------------------------------------------- helpers

/// "$1,299.00", "64.95" or 64.95 -> cents.
pub fn money(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_f64().and_then(cents_from_f64),
        Value::String(s) => money_str(s),
        _ => None,
    }
}

pub fn money_str(s: &str) -> Option<i64> {
    let cleaned: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse::<f64>().ok().and_then(cents_from_f64)
}

fn cents_from_f64(f: f64) -> Option<i64> {
    (f.is_finite() && f > 0.0 && f < 10_000_000.0).then(|| (f * 100.0).round() as i64)
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&reg;", "\u{ae}")
        .replace("&trade;", "\u{2122}")
        .replace("&nbsp;", " ")
}

fn first_capture(re: &Regex, html: &str) -> Option<String> {
    re.captures(html).map(|c| c[1].to_string())
}

/// `<meta property="og:title" content="...">` style tags, by property/name/itemprop.
fn meta(html: &str, key: &str) -> Option<String> {
    for tag in RE_META.find_iter(html) {
        let mut matches = false;
        let mut content = None;
        for cap in RE_ATTR.captures_iter(tag.as_str()) {
            let name = cap[1].to_ascii_lowercase();
            let value = cap.get(2).or(cap.get(3)).map(|m| m.as_str()).unwrap_or("");
            match name.as_str() {
                "property" | "name" | "itemprop" if value.eq_ignore_ascii_case(key) => matches = true,
                "content" => content = Some(value.to_string()),
                _ => {}
            }
        }
        if matches {
            if let Some(c) = content.filter(|c| !c.trim().is_empty()) {
                return Some(c);
            }
        }
    }
    None
}

// ---------------------------------------------------------------- JSON-LD

fn is_type(v: &Value, wanted: &[&str]) -> bool {
    match v.get("@type") {
        Some(Value::String(s)) => wanted.contains(&s.as_str()),
        Some(Value::Array(a)) => a.iter().any(|x| x.as_str().is_some_and(|s| wanted.contains(&s))),
        _ => false,
    }
}

fn collect_products<'a>(v: &'a Value, out: &mut Vec<&'a Value>, depth: usize) {
    if depth > 6 {
        return;
    }
    match v {
        Value::Array(items) => items.iter().for_each(|x| collect_products(x, out, depth + 1)),
        Value::Object(map) => {
            if is_type(v, &["Product", "ProductGroup", "IndividualProduct"]) {
                out.push(v);
                return;
            }
            for key in ["@graph", "mainEntity", "mainEntityOfPage", "itemListElement", "item"] {
                if let Some(child) = map.get(key) {
                    collect_products(child, out, depth + 1);
                }
            }
        }
        _ => {}
    }
}

#[derive(Default)]
struct OfferSummary {
    /// Lowest price among offers that are in stock, else lowest of all.
    price: Option<i64>,
    in_stock: Option<bool>,
}

fn availability(v: &Value) -> Option<bool> {
    let s = v.get("availability")?.as_str()?.to_ascii_lowercase();
    if s.contains("outofstock") || s.contains("soldout") || s.contains("discontinued") {
        Some(false)
    } else if s.contains("instock")
        || s.contains("limitedavailability")
        || s.contains("onlineonly")
        || s.contains("instoreonly")
        || s.contains("preorder")
        || s.contains("backorder")
    {
        Some(true)
    } else {
        None
    }
}

fn offer_price(o: &Value) -> Option<i64> {
    for key in ["price", "lowPrice"] {
        if let Some(p) = o.get(key).and_then(money) {
            return Some(p);
        }
    }
    let spec = o.get("priceSpecification")?;
    match spec {
        Value::Array(a) => a.iter().filter_map(|s| s.get("price").and_then(money)).min(),
        _ => spec.get("price").and_then(money),
    }
}

fn gather_offers<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| gather_offers(x, out)),
        Value::Object(_) => {
            out.push(v);
            // An AggregateOffer may also list the individual offers.
            if let Some(inner) = v.get("offers") {
                gather_offers(inner, out);
            }
        }
        _ => {}
    }
}

fn summarize_offers(product: &Value) -> OfferSummary {
    let mut offers = Vec::new();
    if let Some(o) = product.get("offers") {
        gather_offers(o, &mut offers);
    }
    if let Some(Value::Array(variants)) = product.get("hasVariant") {
        for variant in variants {
            if let Some(o) = variant.get("offers") {
                gather_offers(o, &mut offers);
            }
        }
    }
    let priced: Vec<(i64, Option<bool>)> = offers.iter().filter_map(|o| offer_price(o).map(|p| (p, availability(o)))).collect();
    let any_in = offers.iter().any(|o| availability(o) == Some(true));
    let any_out = offers.iter().any(|o| availability(o) == Some(false));
    let in_stock = if any_in {
        Some(true)
    } else if any_out {
        Some(false)
    } else {
        None
    };
    let price = priced.iter().filter(|(_, avail)| !any_in || *avail == Some(true)).map(|(p, _)| *p).min();
    OfferSummary { price, in_stock }
}

fn image_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => a.iter().find_map(image_of),
        Value::Object(_) => v.get("url").or(v.get("contentUrl")).and_then(image_of),
        _ => None,
    }
}

/// Product details from the page's schema.org data, if it has any.
fn from_json_ld(html: &str) -> Observation {
    let blocks: Vec<Value> = RE_LD.captures_iter(html).filter_map(|c| serde_json::from_str::<Value>(c[1].trim()).ok()).collect();
    let mut products = Vec::new();
    for block in &blocks {
        collect_products(block, &mut products, 0);
    }
    // Prefer the entry that actually carries a price.
    let Some(product) = products.iter().find(|p| summarize_offers(p).price.is_some()).or(products.first()) else {
        return Observation::default();
    };
    let offers = summarize_offers(product);
    let brand = product.get("brand").and_then(|b| match b {
        Value::Object(_) => b.get("name").and_then(text),
        other => text(other),
    });
    let gtin = ["gtin13", "gtin12", "gtin14", "gtin", "gtin8"].iter().find_map(|k| product.get(*k).and_then(text));
    Observation {
        title: product.get("name").and_then(text),
        brand,
        model: product.get("mpn").and_then(text).or_else(|| product.get("model").and_then(text)),
        gtin,
        image_url: product.get("image").and_then(image_of),
        price_cents: offers.price,
        in_stock: offers.in_stock,
        details: {
            let field = |key: &str| product.get(key).and_then(text);
            let mut facts: Vec<(String, Option<String>)> = vec![
                ("Colour".into(), field("color")),
                ("Size".into(), field("size")),
                ("Material".into(), field("material")),
                ("Pattern".into(), field("pattern")),
            ];
            // Shops list further specifications as name and value pairs.
            if let Some(Value::Array(extra)) = product.get("additionalProperty") {
                for property in extra.iter().take(8) {
                    if let Some(name) = property.get("name").and_then(text) {
                        facts.push((name, property.get("value").and_then(text)));
                    }
                }
            }
            details_of(facts, product.get("description").and_then(|v| v.as_str()))
        },
        ..Default::default()
    }
}

static RE_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());

/// Longest details text kept for a product.
const DETAILS_MAX: usize = 500;

/// One plain-text line of what sets a product apart: labelled facts
/// ("Colour: Blue"), then the start of its description.
fn details_of(facts: Vec<(String, Option<String>)>, description: Option<&str>) -> Option<String> {
    let plain =
        |raw: &str| -> String { decode_entities(&RE_TAG.replace_all(raw, " ")).split_whitespace().collect::<Vec<_>>().join(" ") };
    let mut parts: Vec<String> = Vec::new();
    for (label, value) in facts {
        let value = value.map(|v| plain(&v)).unwrap_or_default();
        if !value.is_empty() && value.len() <= 80 {
            parts.push(format!("{}: {value}", plain(&label)));
        }
    }
    let about = plain(description.unwrap_or(""));
    if !about.is_empty() {
        parts.push(about);
    }
    let joined = parts.join(". ");
    if joined.is_empty() {
        return None;
    }
    // Cut at a character boundary, on a word where possible.
    let mut cut: String = joined.chars().take(DETAILS_MAX).collect();
    if cut.len() < joined.len() {
        if let Some(space) = cut.rfind(' ') {
            cut.truncate(space);
        }
        cut.push('\u{2026}');
    }
    Some(cut)
}

static RE_STRUCK_PRICE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"class="strike-through[^"]*">\s*<span class="value" content="([0-9.]+)""#).unwrap());

/// Any shop that publishes standard product data.
fn generic(html: &str) -> Observation {
    let mut obs = from_json_ld(html);
    if obs.price_cents.is_none() {
        obs.price_cents = ["product:price:amount", "og:price:amount", "product:sale_price:amount", "price"]
            .iter()
            .find_map(|k| meta(html, k))
            .and_then(|s| money_str(&s));
    }
    if obs.title.is_none() {
        obs.title = meta(html, "og:title").or_else(|| first_capture(&RE_TITLE, html));
    }
    if obs.image_url.is_none() {
        obs.image_url = meta(html, "og:image");
    }
    if obs.brand.is_none() {
        obs.brand = meta(html, "product:brand").or_else(|| meta(html, "og:brand"));
    }
    if obs.details.is_none() {
        obs.details = details_of(vec![], meta(html, "og:description").or_else(|| meta(html, "description")).as_deref());
    }
    // Shops built on Salesforce Commerce Cloud show the regular price struck through.
    if obs.list_price_cents.is_none() {
        obs.list_price_cents = first_capture(&RE_STRUCK_PRICE, html).and_then(|s| money_str(&s));
    }
    if obs.in_stock.is_none() {
        obs.in_stock = meta(html, "product:availability")
            .or_else(|| meta(html, "og:availability"))
            .map(|s| s.to_ascii_lowercase())
            .and_then(|s| {
                if s.contains("out") {
                    Some(false)
                } else if s.contains("in") {
                    Some(true)
                } else {
                    None
                }
            });
    }
    obs
}

// ---------------------------------------------------------------- stores

static RE_STAPLES_LIST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""listPrice":([0-9.]+)"#).unwrap());
static RE_STAPLES_FINAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""finalPrice":([0-9.]+)"#).unwrap());

fn staples(html: &str) -> Observation {
    let mut obs = generic(html);
    if obs.price_cents.is_none() {
        obs.price_cents = first_capture(&RE_STAPLES_FINAL, html).and_then(|s| money_str(&s));
    }
    obs.list_price_cents = first_capture(&RE_STAPLES_LIST, html).and_then(|s| money_str(&s));
    obs
}

static RE_BBY_PRICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""customerPrice":([0-9.]+)"#).unwrap());
static RE_BBY_SAVINGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""customerPrice":([0-9.]+)[^{}]{0,400}?"totalSavings":([0-9.]+)"#).unwrap());
static RE_BBY_UPC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""upc":"(\d{8,14})""#).unwrap());

static RE_BBY_OPTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\{"sku":"(\d{5,10})","state":"[a-zA-Z]+","seller":\{[^{}]*\},"displayName":"([^"]{1,60})""#).unwrap()
});

fn bestbuy(html: &str) -> Observation {
    // The schema.org data lists every marketplace seller, so the price Best Buy
    // itself shows the shopper ("customerPrice") is the one to trust.
    let mut obs = generic(html);
    if let Some(price) = first_capture(&RE_BBY_PRICE, html).and_then(|s| money_str(&s)) {
        obs.price_cents = Some(price);
        if let Some(c) = RE_BBY_SAVINGS.captures(html) {
            if let (Some(p), Some(saved)) = (money_str(&c[1]), money_str(&c[2])) {
                obs.list_price_cents = Some(p + saved);
            }
        }
    }
    if html.contains(r#""buttonState":"SOLD_OUT""#) && !html.contains(r#""buttonState":"ADD_TO_CART""#) {
        obs.in_stock = Some(false);
    } else if html.contains(r#""buttonState":"ADD_TO_CART""#) {
        obs.in_stock = Some(true);
    }
    if obs.gtin.is_none() {
        obs.gtin = first_capture(&RE_BBY_UPC, html);
    }
    // Each colour is its own item at Best Buy; the page lists them by item number.
    for cap in RE_BBY_OPTION.captures_iter(html) {
        let variant = Variant { sku: cap[1].to_string(), label: decode_entities(&cap[2]) };
        if !obs.variants.contains(&variant) {
            obs.variants.push(variant);
        }
    }
    obs
}

fn walmart(html: &str) -> Observation {
    let data = RE_NEXT_DATA.captures(html).and_then(|c| serde_json::from_str::<Value>(c[1].trim()).ok());
    let Some(product) =
        data.as_ref().and_then(|d| d.pointer("/props/pageProps/initialData/data/product")).filter(|p| p.is_object())
    else {
        return generic(html);
    };
    let status = product.get("availabilityStatus").and_then(|v| v.as_str());
    // Walmart hosts other sellers. "INTERNAL" is Walmart itself; other
    // sellers may still have Walmart ship for them.
    let seller_type = product.get("sellerType").and_then(|v| v.as_str());
    let third_party = seller_type.map(|kind| kind != "INTERNAL");
    let store_ships = match (third_party, product.get("wfsEnabled").and_then(|v| v.as_bool())) {
        (Some(false), _) | (_, Some(true)) => Some(true),
        (Some(true), Some(false)) => Some(false),
        _ => None,
    };
    // The delivery choice: free, a stated charge, or charged without the amount.
    let shipping = product
        .get("fulfillmentOptions")
        .and_then(|v| v.as_array())
        .and_then(|options| options.iter().find(|o| o.get("type").and_then(|t| t.as_str()) == Some("SHIPPING")));
    let stated_charge = shipping
        .and_then(|s| s.pointer("/speedDetails/fulfillmentPrice/price"))
        .or(product.pointer("/fulfillmentSummary/0/fulfillmentPrice/price"))
        .and_then(|v| v.as_f64())
        .filter(|f| f.is_finite() && *f >= 0.0)
        .map(|f| (f * 100.0).round() as i64);
    let ships_free = match stated_charge {
        Some(cents) => Some(cents == 0),
        None => shipping.and_then(|s| s.pointer("/speedDetails/freeFulfillment")).and_then(|v| v.as_bool()),
    };
    Observation {
        title: product.get("name").and_then(text),
        brand: product.get("brand").and_then(text),
        model: product.get("model").and_then(text),
        gtin: product.get("upc").and_then(text),
        image_url: product.pointer("/imageInfo/thumbnailUrl").and_then(text),
        price_cents: product.pointer("/priceInfo/currentPrice/price").and_then(money),
        list_price_cents: product.pointer("/priceInfo/wasPrice/price").and_then(money),
        in_stock: status.map(|s| s == "IN_STOCK"),
        promo_text: None,
        other_gtins: vec![],
        variants: vec![],
        details: details_of(vec![], product.get("shortDescription").and_then(|v| v.as_str())),
        // Walmart prices by store, and chooses the store from where the request comes from.
        local_store: product.pointer("/location/storeIds/0").and_then(text).map(|id| {
            let part = |key: &str| product.pointer(&format!("/location/{key}")).and_then(text).unwrap_or_default();
            let (city, state, zip) = (part("city"), part("stateOrProvinceCode"), part("postalCode"));
            let town = [city, state].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(", ");
            LocalStore { id, place: format!("{town} {zip}").trim().to_string() }
        }),
        seller: product.get("sellerDisplayName").and_then(text).or_else(|| product.get("sellerName").and_then(text)),
        third_party,
        store_ships,
        ships_free,
        shipping_cents: stated_charge.filter(|cents| *cents > 0),
    }
}

static RE_TARGET_BARCODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""primary_barcode"\s*:\s*"(\d{8,14})""#).unwrap());

/// Target's own product JSON (the same data its website loads).
fn target_json(body: &str) -> Observation {
    let Ok(data) = serde_json::from_str::<Value>(body) else {
        return Observation::default();
    };
    let Some(product) = data.pointer("/data/product") else {
        return Observation::default();
    };
    let price = product.pointer("/price/current_retail").or(product.pointer("/price/current_retail_min")).and_then(money);
    let list = product.pointer("/price/reg_retail").or(product.pointer("/price/reg_retail_max")).and_then(money);
    let promo = product
        .pointer("/price/formatted_current_price_type")
        .and_then(|v| v.as_str())
        .filter(|t| *t != "reg")
        .map(|t| t.to_string());
    Observation {
        title: product.pointer("/item/product_description/title").and_then(text),
        brand: product.pointer("/item/primary_brand/name").and_then(text),
        model: None,
        gtin: product.pointer("/item/primary_barcode").and_then(text),
        image_url: product
            .pointer("/item/enrichment/image_info/primary_image/url")
            .or(product.pointer("/item/enrichment/images/primary_image_url"))
            .and_then(text),
        price_cents: price,
        list_price_cents: list,
        in_stock: None,
        promo_text: promo,
        // A listing with colour or size options carries one barcode per option.
        other_gtins: RE_TARGET_BARCODE.captures_iter(body).map(|c| c[1].to_string()).collect(),
        variants: vec![],
        details: {
            let bullets: Vec<&str> = product
                .pointer("/item/product_description/bullet_descriptions")
                .and_then(|v| v.as_array())
                .map(|list| list.iter().filter_map(|b| b.as_str()).take(6).collect())
                .unwrap_or_default();
            details_of(vec![], Some(&bullets.join("; ")))
        },
        ..Default::default()
    }
}

static RE_AMZ_MODEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)Item model number\s*(?:</th>\s*<td[^>]*>|[^<]*</span>\s*<span[^>]*>)\s*([^<]{2,60}?)\s*</(?:td|span)>")
        .unwrap()
});

static RE_AMZ_PRICE_AMOUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""priceAmount":([0-9.]+)"#).unwrap());

/// "Item model number" from Amazon's product details, without the invisible
/// text-direction marks Amazon puts in front of values.
fn amazon_model(html: &str) -> Option<String> {
    let raw = first_capture(&RE_AMZ_MODEL, html)?;
    let cleaned = raw.replace("&lrm;", "").replace("&rlm;", "").replace(['\u{200e}', '\u{200f}'], "");
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_string())
}

fn amazon(html: &str) -> Observation {
    let doc = Html::parse_document(html);
    let pick = |css: &str| -> Option<String> {
        let sel = Selector::parse(css).ok()?;
        doc.select(&sel)
            .map(|e| e.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" "))
            .find(|t| !t.is_empty())
    };
    let attr = |css: &str, name: &str| -> Option<String> {
        let sel = Selector::parse(css).ok()?;
        doc.select(&sel).find_map(|e| e.value().attr(name).map(str::to_string)).filter(|v| !v.is_empty())
    };

    // The price the shopper pays. Amazon sometimes leaves the hidden full-text
    // copy blank and only fills the split dollars / cents parts.
    let mut price = [
        "#corePriceDisplay_desktop_feature_div .priceToPay .a-offscreen",
        "#corePrice_feature_div .a-price .a-offscreen",
        "#corePrice_desktop .a-price .a-offscreen",
        "#apex_desktop .priceToPay .a-offscreen",
        "#price_inside_buybox",
        "#priceblock_ourprice",
        "#priceblock_dealprice",
        "#newBuyBoxPrice",
        "#kindle-price",
    ]
    .iter()
    .find_map(|css| pick(css).and_then(|t| money_str(&t)));
    if price.is_none() {
        let whole = pick("#corePriceDisplay_desktop_feature_div .priceToPay .a-price-whole")
            .or_else(|| pick("#apex_desktop .priceToPay .a-price-whole"));
        let fraction = pick("#corePriceDisplay_desktop_feature_div .priceToPay .a-price-fraction")
            .or_else(|| pick("#apex_desktop .priceToPay .a-price-fraction"));
        if let Some(whole) = whole {
            let digits: String = whole.chars().filter(|c| c.is_ascii_digit()).collect();
            let frac: String = fraction.unwrap_or_default().chars().filter(|c| c.is_ascii_digit()).collect();
            price = money_str(&format!("{digits}.{}", if frac.is_empty() { "00" } else { &frac }));
        }
    }
    if price.is_none() {
        price = first_capture(&RE_AMZ_PRICE_AMOUNT, html).and_then(|s| money_str(&s));
    }

    let list = [
        "#corePriceDisplay_desktop_feature_div .basisPrice .a-offscreen",
        "#corePrice_feature_div .a-text-price .a-offscreen",
        "#apex_desktop .basisPrice .a-offscreen",
    ]
    .iter()
    .find_map(|css| pick(css).and_then(|t| money_str(&t)));

    let availability = pick("#availability").map(|t| t.to_ascii_lowercase());
    let in_stock = match availability.as_deref() {
        Some(t) if t.contains("unavailable") || t.contains("out of stock") => Some(false),
        Some(t) if t.contains("in stock") || t.contains("left in stock") || t.contains("available") => Some(true),
        _ if price.is_some() => Some(true),
        _ => None,
    };

    let brand = pick("#bylineInfo")
        .map(|t| t.trim_start_matches("Visit the ").trim_start_matches("Brand: ").trim_end_matches(" Store").to_string());

    // Amazon hosts other sellers. The buying box names who sells the item
    // and, when that is someone else, who ships it.
    let is_amazon = |name: &str| name.to_ascii_lowercase().starts_with("amazon");
    let seller = pick(r#"[offer-display-feature-name$="merchant-info"] .offer-display-feature-text-message"#);
    let shipper = pick(r#"[offer-display-feature-name$="fulfiller-info"] .offer-display-feature-text-message"#);
    let third_party = seller.as_deref().map(|name| !is_amazon(name));
    let store_ships = shipper.as_deref().or(seller.as_deref()).map(is_amazon);
    // "FREE", or the charge for delivering this one item.
    let delivery = attr(
        "#deliveryBlockMessage [data-csa-c-delivery-price], #mir-layout-DELIVERY_BLOCK [data-csa-c-delivery-price]",
        "data-csa-c-delivery-price",
    );
    let shipping_cents = delivery.as_deref().and_then(money_str);
    let ships_free = match (&delivery, shipping_cents) {
        (_, Some(_)) => Some(false),
        (Some(text), None) if text.eq_ignore_ascii_case("free") => Some(true),
        _ => None,
    };

    Observation {
        title: pick("#productTitle").or_else(|| pick("#title")),
        brand,
        model: amazon_model(html),
        gtin: None,
        image_url: attr("#landingImage", "data-old-hires")
            .or_else(|| attr("#landingImage", "src"))
            .or_else(|| attr("#imgBlkFront", "src")),
        price_cents: price,
        list_price_cents: list,
        in_stock,
        promo_text: None,
        other_gtins: vec![],
        variants: vec![],
        details: {
            let colour =
                pick("#variation_color_name .selection").or_else(|| pick("#inline-twister-expanded-dimension-text-color_name"));
            let size =
                pick("#variation_size_name .selection").or_else(|| pick("#inline-twister-expanded-dimension-text-size_name"));
            let bullets = Selector::parse("#feature-bullets li span.a-list-item")
                .ok()
                .map(|sel| doc.select(&sel).map(|e| e.text().collect::<String>()).take(4).collect::<Vec<_>>().join("; "));
            details_of(vec![("Colour".into(), colour), ("Size".into(), size)], bullets.as_deref())
        },
        local_store: None,
        seller,
        third_party,
        store_ships,
        ships_free,
        shipping_cents,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn money_formats() {
        assert_eq!(money_str("$1,299.00"), Some(129900));
        assert_eq!(money_str("64.95"), Some(6495));
        assert_eq!(money(&serde_json::json!(21.99)), Some(2199));
        assert_eq!(money(&serde_json::json!(449)), Some(44900));
        assert_eq!(money_str("call for price"), None);
        assert_eq!(money(&serde_json::json!(0)), None);
    }

    #[test]
    fn walmart_page() {
        let o = parse("walmart", &fixture("walmart.html")).unwrap();
        assert_eq!(o.title.as_deref(), Some("AirPods Max 2 - Blue"));
        assert_eq!(o.price_cents, Some(44900));
        assert_eq!(o.list_price_cents, Some(54900));
        assert_eq!(o.in_stock, Some(true));
        assert_eq!(o.brand.as_deref(), Some("Apple"));
        assert_eq!(o.gtin.as_deref(), Some("195951039305"));
    }

    #[test]
    fn target_product_json() {
        let o = parse("target", &fixture("target.json")).unwrap();
        assert_eq!(o.price_cents, Some(24999));
        assert_eq!(o.list_price_cents, None, "same as the price, so not a discount");
        assert_eq!(o.brand.as_deref(), Some("Apple"));
        assert_eq!(o.gtin.as_deref(), Some("195949704529"));
        assert!(o.title.unwrap().contains("AirPods Pro 2"));
        assert!(o.image_url.unwrap().starts_with("https://"));
    }

    #[test]
    fn bestbuy_page_uses_best_buys_own_price() {
        let o = parse("bestbuy", &fixture("bestbuy.html")).unwrap();
        assert_eq!(o.price_cents, Some(15999));
        assert_eq!(o.list_price_cents, Some(15999 + 3877));
        assert_eq!(o.brand.as_deref(), Some("Apple"));
        assert_eq!(o.model.as_deref(), Some("MXP93LL/A"));
        assert!(o.image_url.unwrap().contains("bbystatic.com"));
    }

    #[test]
    fn bestbuy_colour_options_are_listed() {
        let html = r#"<html><head><title>Headphones - Best Buy</title></head><body><script>var x = {"customerPrice":449,
            "productVariation":[{"variationType":"Color","options":[
            {"sku":"6550083","state":"available","seller":{"classification":"1P","id":"bby"},"displayName":"Blue","colorSwatch":"Blue"},
            {"sku":"6550081","state":"selected","seller":{"classification":"1P","id":"bby"},"displayName":"Starlight","colorSwatch":"Starlight"}]}]};
            </script></body></html>"#;
        let o = parse("bestbuy", html).unwrap();
        assert_eq!(
            o.variants,
            vec![
                Variant { sku: "6550083".into(), label: "Blue".into() },
                Variant { sku: "6550081".into(), label: "Starlight".into() }
            ]
        );
    }

    #[test]
    fn kohls_page_takes_the_lowest_variant() {
        let o = parse("kohls", &fixture("kohls.html")).unwrap();
        assert_eq!(o.price_cents, Some(6495));
        assert_eq!(o.in_stock, Some(true));
        assert_eq!(o.brand.as_deref(), Some("Madison Park"));
    }

    #[test]
    fn harbor_freight_page() {
        let o = parse("harborfreight", &fixture("harborfreight.html")).unwrap();
        assert_eq!(o.price_cents, Some(2199));
        assert_eq!(o.brand.as_deref(), Some("PITTSBURGH"));
        assert_eq!(o.in_stock, Some(true));
    }

    #[test]
    fn walmart_page_says_which_store_the_price_is_from() {
        let html = r#"<html><body><script id="__NEXT_DATA__" type="application/json">
            {"props":{"pageProps":{"initialData":{"data":{"product":{
              "name":"Whole Milk, 1 Gallon","priceInfo":{"currentPrice":{"price":3.47}},"availabilityStatus":"IN_STOCK",
              "location":{"postalCode":"62704","stateOrProvinceCode":"IL","city":"Springfield","storeIds":["1234"]}
            }}}}}}</script></body></html>"#;
        let obs = parse("walmart", html).unwrap();
        assert_eq!(obs.price_cents, Some(347));
        assert_eq!(obs.local_store, Some(LocalStore { id: "1234".into(), place: "Springfield, IL 62704".into() }));
        assert_eq!((obs.seller, obs.third_party, obs.ships_free), (None, None, None), "this page does not say");
        // A page that does not say is fine too.
        assert_eq!(parse("walmart", &fixture("walmart.html")).unwrap().local_store, None);
    }

    fn walmart_offer(extra: &str) -> Observation {
        let html = format!(
            r#"<html><body><script id="__NEXT_DATA__" type="application/json">
            {{"props":{{"pageProps":{{"initialData":{{"data":{{"product":{{
              "name":"Clear Phone Case","priceInfo":{{"currentPrice":{{"price":12.99}}}},"availabilityStatus":"IN_STOCK",{extra}
            }}}}}}}}}}}}</script></body></html>"#
        );
        parse("walmart", &html).unwrap()
    }

    #[test]
    fn walmart_page_says_who_sells_and_whether_delivery_is_free() {
        // Sold and shipped by Walmart, free delivery.
        let own = walmart_offer(
            r#""sellerDisplayName":"Walmart.com","sellerType":"INTERNAL","wfsEnabled":false,
               "fulfillmentOptions":[{"type":"SHIPPING","speedDetails":{"fulfillmentPrice":null,"freeFulfillment":true}}]"#,
        );
        assert_eq!(own.seller.as_deref(), Some("Walmart.com"));
        assert_eq!(
            (own.third_party, own.store_ships, own.ships_free, own.shipping_cents),
            (Some(false), Some(true), Some(true), None)
        );

        // Another seller that Walmart ships for; delivery is charged, the amount not given.
        let other = walmart_offer(
            r#""sellerDisplayName":"Brand Bargain Depot","sellerType":"EXTERNAL","wfsEnabled":true,
               "fulfillmentOptions":[{"type":"PICKUP"},{"type":"SHIPPING","speedDetails":{"fulfillmentPrice":null,"freeFulfillment":false}}]"#,
        );
        assert_eq!(other.seller.as_deref(), Some("Brand Bargain Depot"));
        assert_eq!(
            (other.third_party, other.store_ships, other.ships_free, other.shipping_cents),
            (Some(true), Some(true), Some(false), None)
        );

        // Another seller that ships it itself and states the charge.
        let charged = walmart_offer(
            r#""sellerDisplayName":"Far Away Goods","sellerType":"EXTERNAL","wfsEnabled":false,
               "fulfillmentOptions":[{"type":"SHIPPING","speedDetails":{"fulfillmentPrice":{"price":5.99},"freeFulfillment":false}}]"#,
        );
        assert_eq!(
            (charged.third_party, charged.store_ships, charged.ships_free, charged.shipping_cents),
            (Some(true), Some(false), Some(false), Some(599))
        );
    }

    fn amazon_offer(buy_box: &str) -> Observation {
        let html = format!(
            r##"<html><head><title>Amazon.com: Thing</title></head><body>
            <span id="productTitle">Thing</span>
            <div id="corePriceDisplay_desktop_feature_div"><span class="a-price priceToPay"><span class="a-offscreen">$15.95</span></span></div>
            {buy_box}</body></html>"##
        );
        parse("amazon", &html).unwrap()
    }

    #[test]
    fn amazon_page_says_who_sells_ships_and_what_delivery_costs() {
        let line = |name: &str, value: &str| {
            format!(
                r#"<div offer-display-feature-name="desktop-{name}" class="offer-display-feature-text">
                   <span class="a-size-small offer-display-feature-text-message">{value}</span></div>"#
            )
        };
        let delivery = |price: &str| {
            format!(r#"<div id="deliveryBlockMessage"><span data-csa-c-delivery-price="{price}">delivery</span></div>"#)
        };

        // One line when Amazon both ships and sells.
        let own = amazon_offer(&format!("{}{}", line("merchant-info", "Amazon.com"), delivery("FREE")));
        assert_eq!(own.seller.as_deref(), Some("Amazon.com"));
        assert_eq!(
            (own.third_party, own.store_ships, own.ships_free, own.shipping_cents),
            (Some(false), Some(true), Some(true), None)
        );

        // Another seller, shipped by Amazon.
        let shipped = amazon_offer(&format!("{}{}", line("fulfiller-info", "Amazon"), line("merchant-info", "Lambcare")));
        assert_eq!(shipped.seller.as_deref(), Some("Lambcare"));
        assert_eq!((shipped.third_party, shipped.store_ships, shipped.ships_free), (Some(true), Some(true), None));

        // Another seller that ships it itself and charges for it.
        let charged = amazon_offer(&format!("{}{}", line("merchant-info", "Infinite Finds"), delivery("$6.99")));
        assert_eq!(
            (charged.third_party, charged.store_ships, charged.ships_free, charged.shipping_cents),
            (Some(true), Some(false), Some(false), Some(699))
        );

        // A page without a buying box says nothing.
        let bare = amazon_offer("");
        assert_eq!((bare.seller, bare.third_party, bare.store_ships, bare.ships_free), (None, None, None, None));
    }

    #[test]
    fn hot_topic_page_with_a_sale_price() {
        let obs = parse("hottopic", &fixture("hottopic.html")).unwrap();
        assert_eq!(obs.title.as_deref(), Some("Funko Disney Lilo & Stitch Pop! Angel Vinyl Figure"));
        assert_eq!(obs.brand.as_deref(), Some("FUNKO"));
        assert_eq!((obs.price_cents, obs.list_price_cents), (Some(2632), Some(3290)));
        assert_eq!(obs.in_stock, Some(true));
        assert_eq!(obs.details, None, "the description only repeats the name");
    }

    #[test]
    fn details_are_one_plain_line() {
        let html = r#"<html><head><script type="application/ld+json">
            {"@type":"Product","name":"Towel Set","color":"Sage Green","size":"6-Piece",
             "description":"<p>Soft &amp; thick cotton towels.</p><ul><li>Two bath towels</li></ul>",
             "additionalProperty":[{"@type":"PropertyValue","name":"Weight","value":"800 GSM"}],
             "offers":{"price":"39.99"}}
            </script></head></html>"#;
        let obs = parse("shop.example", html).unwrap();
        assert_eq!(
            obs.details.as_deref(),
            Some("Colour: Sage Green. Size: 6-Piece. Weight: 800 GSM. Soft & thick cotton towels. Two bath towels")
        );

        // Long descriptions are cut on a word.
        let long = details_of(vec![], Some(&"word ".repeat(300))).unwrap();
        assert!(long.chars().count() <= DETAILS_MAX + 1 && long.ends_with("word\u{2026}"), "{long}");
        assert_eq!(details_of(vec![("Colour".into(), None)], Some("  ")), None);
    }

    #[test]
    fn refusal_pages_are_reported_as_blocked() {
        let denied = "<html><head><title>Access Denied</title></head><body>no</body></html>";
        assert!(matches!(parse("staples", denied), Err(ParseError::Blocked(_))));
        let px = "<html><head><title>Access to this page has been denied</title></head></html>";
        assert!(matches!(parse("harborfreight", px), Err(ParseError::Blocked(_))));
    }

    #[test]
    fn page_without_a_price_is_an_error() {
        let html = "<html><head><title>Some shop</title></head><body>hello</body></html>";
        assert!(matches!(parse("example.com", html), Err(ParseError::NoPrice)));
    }

    #[test]
    fn generic_shop_with_meta_tags() {
        let html = r#"<html><head><title>Widget</title>
            <meta property="og:title" content="Blue Widget &amp; Case">
            <meta property="product:price:amount" content="12.50">
            <meta property="og:image" content="//cdn.example.com/w.jpg"></head></html>"#;
        let o = parse("example.com", html).unwrap();
        assert_eq!(o.price_cents, Some(1250));
        assert_eq!(o.title.as_deref(), Some("Blue Widget & Case"));
        assert_eq!(o.image_url.as_deref(), Some("https://cdn.example.com/w.jpg"));
    }

    #[test]
    fn amazon_split_price() {
        let html = r##"<html><head><title>Amazon.com: Thing</title></head><body>
            <span id="productTitle"> Thing 3000 </span>
            <a id="bylineInfo">Visit the Acme Store</a>
            <div id="corePriceDisplay_desktop_feature_div">
              <span class="a-price priceToPay"><span class="a-offscreen"> </span>
                <span aria-hidden="true"><span class="a-price-symbol">$</span><span class="a-price-whole">1,189<span class="a-price-decimal">.</span></span><span class="a-price-fraction">99</span></span></span>
              <span class="a-price a-text-price basisPrice"><span class="a-offscreen">$1,299.00</span></span>
            </div>
            <div id="availability"><span> In Stock </span></div>
            <table><tr><th class="prodDetSectionEntry"> Item model number </th>   <td class="prodDetAttrValue">
                &lrm;T-3000/B </td></tr></table>
            <img id="landingImage" src="https://m.media-amazon.com/images/I/x.jpg">
            </body></html>"##;
        let o = parse("amazon", html).unwrap();
        assert_eq!(o.title.as_deref(), Some("Thing 3000"));
        assert_eq!(o.brand.as_deref(), Some("Acme"));
        assert_eq!(o.price_cents, Some(118999));
        assert_eq!(o.list_price_cents, Some(129900));
        assert_eq!(o.in_stock, Some(true));
        assert_eq!(o.model.as_deref(), Some("T-3000/B"));
    }

    #[test]
    fn amazon_unavailable_item_is_out_of_stock_not_an_error() {
        let html = r#"<html><head><title>Amazon.com: Thing</title></head><body>
            <span id="productTitle">Thing</span>
            <div id="availability"><span>Currently unavailable.</span></div></body></html>"#;
        let o = parse("amazon", html).unwrap();
        assert_eq!(o.price_cents, None);
        assert_eq!(o.in_stock, Some(false));
    }
}
