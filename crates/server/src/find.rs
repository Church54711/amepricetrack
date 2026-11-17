//! Finding a product to track from its name or its barcode, instead of a link.
//!
//! The words are typed into each store's own search, one store at a time, and
//! the first results are offered. The person picks the right one, and from
//! there it is tracked like a pasted link.

use crate::compare::{self, search::StoreSearch};
use crate::db::{self, Product};
use crate::retailers;
use crate::service::{user_problem, Problem};
use crate::App;
use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

/// Results offered per store.
const MAX_RESULTS: usize = 4;

/// What a person asked to find.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Query {
    /// The words to type into a store's search.
    pub text: String,
    /// True when the words are a product barcode (UPC or EAN).
    pub barcode: bool,
}

/// Tidies what a person typed or scanned. Barcodes are written the way
/// stores list them: twelve digits.
pub fn query(typed: &str) -> Result<Query, Problem> {
    let text = typed.split_whitespace().collect::<Vec<_>>().join(" ");
    let digits: String = text.chars().filter(|c| !matches!(c, ' ' | '-')).collect();
    if (8..=14).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit()) {
        // A 13 or 14-digit code with leading zeros is the same product as the 12-digit one.
        let trimmed = if digits.len() > 12 { digits.trim_start_matches('0').to_string() } else { digits };
        // An eight-digit code is a kind of its own (EAN-8) and is searched as printed.
        let text = if trimmed.len() == 8 { trimmed } else { format!("{trimmed:0>12}") };
        return Ok(Query { text, barcode: true });
    }
    match text.chars().count() {
        0 => Err(user_problem("Type a product name or a barcode number first.")),
        1..=2 => Err(user_problem("That is too short to search for.")),
        3..=120 => Ok(Query { text, barcode: false }),
        _ => Err(user_problem("That is too long to search for. Use a few words.")),
    }
}

/// True when the text is a product link (or share text containing one)
/// rather than something to search for.
pub fn is_link(text: &str) -> bool {
    static RE_SITE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[a-z0-9][a-z0-9.-]*\.[a-z]{2,}(/\S*)?$").unwrap());
    let text = text.trim();
    text.contains("://") || RE_SITE.is_match(text)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    pub title: String,
    pub url: String,
}

/// One store's answer.
#[derive(Debug, Clone, Serialize)]
pub struct StoreAnswer {
    pub store: String,
    pub store_name: String,
    pub results: Vec<Item>,
    /// Why the store could not be searched, when it could not.
    pub problem: Option<String>,
}

static RE_PRICE_TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i),?\s+(?:current |now )?price\b.*$").unwrap());
static RE_BADGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:best seller|top rated[^,]*|popular pick|overall pick|sponsored|rollback|clearance|\d[\d,.]*k?\+? bought [^,]*),\s*",
    )
    .unwrap()
});

/// A search result's name without what stores read out around it
/// ("Best seller, ..., Price $9.14").
fn tidy_name(raw: &str) -> String {
    let mut name = raw.trim();
    // Badges come before the name, sometimes more than one.
    while let Some(badge) = RE_BADGE.find(name) {
        name = &name[badge.end()..];
    }
    let name = RE_PRICE_TAIL.replace(name, "");
    // Amazon follows the name with a line of selling points after a bar.
    match name.split_once(" | ") {
        Some((before, _)) if before.chars().count() >= 15 => before.trim().to_string(),
        _ => name.trim().to_string(),
    }
}

/// The stores a person can search, in the usual order.
pub async fn stores(app: &App) -> Result<Vec<StoreSearch>, Problem> {
    Ok(compare::stores(&app.db).await?)
}

/// Searches one store and returns its first results.
pub async fn at_store(app: &App, query: &Query, store_id: &str) -> Result<StoreAnswer, Problem> {
    let all = stores(app).await?;
    let Some(store) = all.iter().find(|s| s.retailer == store_id) else {
        return Err(user_problem("That store cannot be searched."));
    };
    let address = store.address(&query.text);
    let mut answer =
        StoreAnswer { store: store.retailer.clone(), store_name: store.name.clone(), results: vec![], problem: None };
    match compare::search_page(&app.fetcher, store, &address).await {
        Ok(links) => {
            for link in links {
                let title = tidy_name(&link.description());
                // A link without a name cannot be chosen between.
                if title.is_empty() || answer.results.iter().any(|r: &Item| r.url == link.url) {
                    continue;
                }
                answer.results.push(Item { title, url: link.url });
                if answer.results.len() == MAX_RESULTS {
                    break;
                }
            }
        }
        // Some stores answer "no such page" when a search finds nothing.
        Err(crate::fetch::FetchError::Gone) => {}
        Err(e) => {
            tracing::debug!(store = store.retailer, query = query.text, "find failed: {e}");
            answer.problem = Some(e.to_string());
        }
    }
    Ok(answer)
}

/// Products this person already tracks that carry the barcode.
pub async fn already_tracked(app: &App, user_id: i64, query: &Query) -> Result<Vec<Product>, Problem> {
    if !query.barcode {
        return Ok(vec![]);
    }
    let wanted = query.text.trim_start_matches('0').to_string();
    Ok(db::watched(&app.db, user_id)
        .await?
        .into_iter()
        .map(|(product, _)| product)
        .filter(|p| p.gtin.as_deref().is_some_and(|g| g.trim_start_matches('0') == wanted))
        .collect())
}

/// The start of a find: the tidied query, the stores to ask, and matches already tracked.
pub async fn start(app: &App, user_id: i64, typed: &str) -> Result<serde_json::Value, Problem> {
    let query = query(typed)?;
    let stores: Vec<serde_json::Value> =
        stores(app).await?.iter().map(|s| serde_json::json!({ "id": s.retailer, "name": s.name })).collect();
    let tracked: Vec<serde_json::Value> = already_tracked(app, user_id, &query)
        .await?
        .iter()
        .map(|p| {
            serde_json::json!({
                "product_id": p.id,
                "title": p.display_title(),
                "store_name": retailers::display_name(&p.retailer),
            })
        })
        .collect();
    Ok(serde_json::json!({ "query": query.text, "barcode": query.barcode, "stores": stores, "tracked": tracked }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_was_typed_or_scanned() {
        let q = |text: &str| query(text).map(|q| (q.text, q.barcode)).map_err(|e| e.to_string());
        assert_eq!(q("  hammermill   copy paper "), Ok(("hammermill copy paper".into(), false)));
        // A scanned UPC, an EAN with a leading zero, and one typed with spaces are the same code.
        assert_eq!(q("195951039305"), Ok(("195951039305".into(), true)));
        assert_eq!(q("0195951039305"), Ok(("195951039305".into(), true)));
        assert_eq!(q("1959 5103 9305"), Ok(("195951039305".into(), true)));
        assert_eq!(q("12345678"), Ok(("12345678".into(), true)));
        assert_eq!(q("19595103930"), Ok(("019595103930".into(), true)), "a leading zero that was dropped");
        // A model number with letters, or a short number, is ordinary search text.
        assert_eq!(q("MXP93LL/A"), Ok(("MXP93LL/A".into(), false)));
        assert_eq!(q("105007"), Ok(("105007".into(), false)));
        assert!(q("").is_err() && q("ab").is_err() && q(&"word ".repeat(40)).is_err());
    }

    #[test]
    fn tells_links_from_search_words() {
        assert!(is_link("https://www.staples.com/paper/product_122374"));
        assert!(is_link("walmart.com/ip/19864722287"));
        assert!(is_link("Check this out! https://a.co/d/abc123"));
        assert!(!is_link("hammermill copy paper"));
        assert!(!is_link("195951039305"));
        assert!(!is_link("MXP93LL/A"));
        assert!(!is_link("8.5 x 11 paper"));
    }

    #[test]
    fn result_names_lose_the_words_stores_read_out_around_them() {
        assert_eq!(
            tidy_name("Best seller, Entronix Clear Case for iPhone 15, Blue, Price $9.14 Options from $9.14 \u{2013} $11.88"),
            "Entronix Clear Case for iPhone 15, Blue"
        );
        assert_eq!(tidy_name("JETech Case for iPhone 15 (Clear), Price $11.99"), "JETech Case for iPhone 15 (Clear)");
        assert_eq!(tidy_name("Hammermill Copy Paper, 500 Sheets"), "Hammermill Copy Paper, 500 Sheets");
        assert_eq!(
            tidy_name("Best seller, 100+ bought since yesterday, Sharpie Permanent Markers, Fine Point, Black, 12 Count"),
            "Sharpie Permanent Markers, Fine Point, Black, 12 Count"
        );
        assert_eq!(
            tidy_name("Sharpie Permanent Markers, Fine Point, Black, 12 Count | Marks on Most Surfaces, Quick-Drying Ink, Water and Fade-Resistan..."),
            "Sharpie Permanent Markers, Fine Point, Black, 12 Count"
        );
        assert_eq!(
            tidy_name("HP | Printer Paper | 8.5 x 11"),
            "HP | Printer Paper | 8.5 x 11",
            "a short start is part of the name"
        );
        assert_eq!(
            tidy_name("1K+ bought since yesterday, Duracell Coppertop AA Alkaline Batteries 8/PK"),
            "Duracell Coppertop AA Alkaline Batteries 8/PK"
        );
        assert_eq!(tidy_name("Shockproof Case, Crystal Clear, Price Now $12.99 Was $44.99"), "Shockproof Case, Crystal Clear");
    }
}
