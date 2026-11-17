//! Finding the same product at other stores.
//!
//! Given a product someone tracks, each other store is searched (by barcode,
//! then brand and model number, then name), the likeliest results are opened,
//! and `matching` decides whether one of them is the same product. When the
//! person has an AI service connected, `advisor` helps choose which results
//! to open and settles the cases a barcode or model number does not.

pub mod advisor;
pub mod cases;
pub mod matching;
pub mod search;
pub mod tracking;

use crate::db::Db;
use crate::fetch::{FetchError, Fetcher};
use crate::retailers::{FetchMode, Observation};
use advisor::{Advisor, Judgement};
use matching::{Facts, PackDiff, Verdict, POSSIBLE, SAME};
use search::{Candidate, StoreSearch};
use std::collections::HashSet;

/// Product pages opened per store at most, so one lookup stays cheap.
const MAX_PAGES_PER_STORE: usize = 5;
const MAX_PAGES_PER_QUERY: usize = 2;
/// Lowest name-only score at which a link is worth opening.
const WORTH_OPENING: u8 = 45;
/// A page scoring at least this is the right kind of product, so its colour
/// and size options are worth a look when it is not itself the match.
const RELATED: u8 = 40;

pub struct Found {
    pub candidate: Candidate,
    pub obs: Observation,
    pub mode: FetchMode,
    /// 0 to 100; `SAME` and above is treated as the same product.
    pub score: u8,
    pub reason: String,
    /// The AI service made the call; the rules alone were not sure.
    pub by_ai: bool,
    /// Set when this is the same item sold in a different amount.
    pub pack: Option<PackDiff>,
}

pub enum StoreResult {
    /// The same product (score >= SAME) or a possible match (score >= POSSIBLE).
    Found(Box<Found>),
    /// The store was searched and has nothing that is this product. The
    /// note says why, when the closest result was looked at and ruled out.
    NotFound(Option<String>),
    /// The store could not be searched.
    Failed(String),
}

pub fn facts_of(
    title: Option<&str>,
    brand: Option<&str>,
    model: Option<&str>,
    gtin: Option<&str>,
    price_cents: Option<i64>,
) -> Facts {
    Facts {
        title: title.unwrap_or("").to_string(),
        brand: brand.map(str::to_string),
        model: model.map(str::to_string),
        gtin: gtin.map(str::to_string),
        price_cents,
        other_gtins: vec![],
        details: None,
    }
}

/// Every store that can be searched: the built-in ones, then the ones people added.
pub async fn stores(db: &Db) -> anyhow::Result<Vec<StoreSearch>> {
    let mut all = search::builtin();
    for row in crate::db::custom_stores(db).await? {
        if !all.iter().any(|s| s.retailer == row.retailer) {
            all.push(StoreSearch { retailer: row.retailer, name: row.name, url: row.search_url, mode: None });
        }
    }
    Ok(all)
}

/// A product page that was opened during a hunt.
struct Opened {
    candidate: Candidate,
    obs: Observation,
    mode: FetchMode,
    facts: Facts,
    verdict: Verdict,
    /// What the AI said about it, once asked.
    ai: Option<AiView>,
}

#[derive(Clone, PartialEq)]
enum AiView {
    /// The same product, with why and (for another pack size) the amounts.
    Same(String, Option<PackDiff>),
    Different,
    Unsure,
}

/// The search of one store in progress.
struct Hunt<'a> {
    fetcher: &'a Fetcher,
    source: &'a Facts,
    store: &'a StoreSearch,
    advisor: Option<&'a Advisor>,
    opened: Vec<Opened>,
    seen: HashSet<String>,
    /// The AI looked at a page of search results and saw nothing that could be the product.
    ai_ruled_out: bool,
}

impl Hunt<'_> {
    fn budget_left(&self) -> bool {
        self.opened.len() < MAX_PAGES_PER_STORE
    }

    fn settled(&self) -> bool {
        self.opened.iter().any(|o| o.verdict.score >= SAME || matches!(o.ai, Some(AiView::Same(..))))
    }

    /// Links worth opening, likeliest first, by name alone. Stores pad thin
    /// results with promoted or unrelated items; a link whose name has almost
    /// nothing in common with the product is dropped. (A link with no
    /// readable name gets the benefit of the doubt.)
    fn rank(&self, links: Vec<Candidate>) -> Vec<Candidate> {
        let mut scored: Vec<(u8, Candidate)> = links
            .into_iter()
            .filter(|c| !self.seen.contains(&c.sku))
            .map(|c| {
                let guess = Facts { title: c.description(), ..Default::default() };
                (matching::judge(self.source, &guess).score, c)
            })
            .filter(|(guess, c)| c.description().is_empty() || *guess >= WORTH_OPENING)
            .collect();
        scored.sort_by_key(|(guess, _)| std::cmp::Reverse(*guess));
        scored.into_iter().map(|(_, c)| c).collect()
    }

    /// Search results worth opening, likeliest first: the AI's choice when
    /// one is connected, otherwise (or if it does not answer) by name.
    async fn choose(&mut self, links: Vec<Candidate>) -> Vec<Candidate> {
        let fresh: Vec<Candidate> = links.into_iter().filter(|c| !self.seen.contains(&c.sku)).collect();
        if let Some(advisor) = self.advisor.filter(|_| !fresh.is_empty()) {
            if let Some(picks) = advisor.pick_results(self.source, &self.store.name, &fresh).await {
                tracing::debug!(store = self.store.retailer, results = fresh.len(), chosen = picks.len(), "AI chose results");
                if picks.is_empty() {
                    self.ai_ruled_out = true;
                }
                return picks.into_iter().map(|i| fresh[i].clone()).collect();
            }
        }
        self.rank(fresh)
    }

    /// Opens one product page and judges it by the rules. Returns the verdict
    /// and the product links on that page (its other colours and sizes among them).
    async fn open(&mut self, candidate: Candidate) -> Option<(Verdict, Vec<Candidate>)> {
        self.seen.insert(candidate.sku.clone());
        let store = self.store.retailer.as_str();
        let fetched = match self.fetcher.fetch(store, &candidate.sku, &candidate.url, None).await {
            Ok(f) => f,
            Err(e) => {
                tracing::debug!(store, url = candidate.url, "could not open result: {e}");
                return None;
            }
        };
        let title = fetched.obs.title.clone().unwrap_or_else(|| candidate.title.clone());
        let facts = Facts {
            other_gtins: fetched.obs.other_gtins.clone(),
            details: fetched.obs.details.clone(),
            ..facts_of(
                Some(&title),
                fetched.obs.brand.as_deref(),
                fetched.obs.model.as_deref(),
                fetched.obs.gtin.as_deref(),
                fetched.obs.price_cents,
            )
        };
        let verdict = matching::judge(self.source, &facts);
        tracing::debug!(store, title, score = verdict.score, reason = verdict.reason, "judged");
        // Its other colours and sizes: the ones the store lists by item number
        // first, then whatever product links the page carries.
        let mut options: Vec<Candidate> = fetched
            .obs
            .variants
            .iter()
            .filter_map(|v| {
                let url = crate::retailers::product_url(store, &v.sku)?;
                Some(Candidate { sku: v.sku.clone(), url, title: v.label.clone() })
            })
            .collect();
        if let Some(html) = fetched.html.as_deref() {
            for link in search::product_links(store, &candidate.url, html) {
                if !options.iter().any(|o| o.sku == link.sku) {
                    options.push(link);
                }
            }
        }
        self.opened.push(Opened { candidate, obs: fetched.obs, mode: fetched.mode, facts, verdict, ai: None });
        Some((verdict, options))
    }

    /// Asks the AI about the pages opened since it was last asked.
    async fn consult(&mut self) {
        let Some(advisor) = self.advisor else { return };
        if self.settled() {
            return;
        }
        let waiting: Vec<usize> = (0..self.opened.len()).filter(|i| self.opened[*i].ai.is_none()).collect();
        if waiting.is_empty() {
            return;
        }
        let listings: Vec<Facts> = waiting.iter().map(|i| self.opened[*i].facts.clone()).collect();
        let Some(judgement) = advisor.judge(self.source, &self.store.name, &listings).await else {
            return;
        };
        tracing::debug!(store = self.store.retailer, listings = listings.len(), ?judgement, "AI judged");
        for (position, index) in waiting.iter().enumerate() {
            self.opened[*index].ai = Some(match &judgement {
                Judgement::Same { index, why, pack } if *index == position => AiView::Same(why.clone(), pack.clone()),
                Judgement::Unsure { .. } => AiView::Unsure,
                _ => AiView::Different,
            });
        }
    }

    fn finish(self, searched_ok: bool, last_error: Option<FetchError>) -> StoreResult {
        let Hunt { opened, ai_ruled_out, source, .. } = self;
        let found = |o: Opened, score: u8, reason: String, by_ai: bool| {
            // The names say how the amounts differ when they state them; otherwise the AI's count stands.
            let stated = matching::pack_difference(&source.title, &o.facts.title);
            let pack = match &o.ai {
                Some(AiView::Same(_, counted)) if by_ai => stated.or_else(|| counted.clone()),
                _ => stated,
            };
            StoreResult::Found(Box::new(Found { candidate: o.candidate, obs: o.obs, mode: o.mode, score, reason, by_ai, pack }))
        };
        // A barcode or model number settles it.
        if let Some(i) = (0..opened.len()).filter(|i| opened[*i].verdict.score >= SAME).max_by_key(|i| opened[*i].verdict.score) {
            let o = opened.into_iter().nth(i).expect("index is in range");
            let (score, reason) = (o.verdict.score, o.verdict.reason.to_string());
            return found(o, score, reason, false);
        }
        if let Some(i) = opened.iter().position(|o| matches!(o.ai, Some(AiView::Same(..)))) {
            let o = opened.into_iter().nth(i).expect("index is in range");
            let why = match &o.ai {
                Some(AiView::Same(why, _)) if !why.is_empty() => why.clone(),
                _ => "Judged the same product".to_string(),
            };
            return found(o, SAME, why, true);
        }
        // Otherwise the closest result the AI did not rule out is offered as a possible match.
        let ruled_out = opened.iter().any(|o| o.ai == Some(AiView::Different) && o.verdict.score >= POSSIBLE);
        let best = (0..opened.len())
            .filter(|i| opened[*i].verdict.score >= POSSIBLE && opened[*i].ai != Some(AiView::Different))
            .max_by_key(|i| opened[*i].verdict.score);
        match best {
            Some(i) => {
                let o = opened.into_iter().nth(i).expect("index is in range");
                let unsure = if o.ai == Some(AiView::Unsure) { "; your AI could not tell" } else { "" };
                let (score, reason) = (o.verdict.score, format!("{}{unsure}", o.verdict.reason));
                found(o, score, reason, false)
            }
            None if ruled_out => {
                StoreResult::NotFound(Some("The closest results are different products, according to your AI".into()))
            }
            None if ai_ruled_out && searched_ok => {
                StoreResult::NotFound(Some("Nothing in the search results is this product, according to your AI".into()))
            }
            None if searched_ok => StoreResult::NotFound(None),
            None => StoreResult::Failed(last_error.map(|e| e.to_string()).unwrap_or_else(|| "nothing to search for".into())),
        }
    }
}

/// Loads a store's search results page and picks the product links out of it.
pub async fn search_page(fetcher: &Fetcher, store: &StoreSearch, address: &str) -> Result<Vec<Candidate>, FetchError> {
    let retailer = store.retailer.as_str();
    if let Some(mode) = store.mode {
        let html = fetcher.page(retailer, address, mode).await?;
        return Ok(search::product_links(retailer, address, &html));
    }
    // An added store: a plain request first. Many shops fill their results
    // in with a script, or refuse plain requests, so an empty or refused
    // page is loaded again in the browser.
    let plain = fetcher.page(retailer, address, FetchMode::Http).await;
    let links = plain.as_ref().map(|html| search::product_links(retailer, address, html)).unwrap_or_default();
    if !links.is_empty() || !fetcher.has_browser() {
        return plain.map(|_| links);
    }
    let html = fetcher.page(retailer, address, FetchMode::Browser).await?;
    Ok(search::product_links(retailer, address, &html))
}

/// Searches one store for the product described by `source`.
pub async fn find_at_store(fetcher: &Fetcher, source: &Facts, store: &StoreSearch, advisor: Option<&Advisor>) -> StoreResult {
    let mut hunt = Hunt { fetcher, source, store, advisor, opened: Vec::new(), seen: HashSet::new(), ai_ruled_out: false };
    let mut searched_ok = false;
    let mut followed_options = false;
    let mut last_error: Option<FetchError> = None;

    'queries: for query in search::queries(source) {
        if !hunt.budget_left() {
            break;
        }
        let address = store.address(&query);
        let links = match search_page(fetcher, store, &address).await {
            Ok(links) => links,
            Err(e) => {
                tracing::debug!(store = store.retailer, query, "search failed: {e}");
                last_error = Some(e);
                continue;
            }
        };
        searched_ok = true;
        let results = links.len();
        let chosen = hunt.choose(links).await;
        tracing::debug!(store = store.retailer, query, results, plausible = chosen.len(), "searched");

        for candidate in chosen.into_iter().take(MAX_PAGES_PER_QUERY) {
            if !hunt.budget_left() {
                break;
            }
            let Some((verdict, options)) = hunt.open(candidate).await else { continue };
            if verdict.score >= SAME {
                break 'queries;
            }
            // The right product in the wrong colour or size: stores link the
            // other options from that page, so try the likeliest of those.
            if verdict.score >= RELATED && !followed_options {
                followed_options = true;
                for option in hunt.rank(options).into_iter().take(2) {
                    if !hunt.budget_left() {
                        break;
                    }
                    if hunt.open(option).await.is_some_and(|(v, _)| v.score >= SAME) {
                        break 'queries;
                    }
                }
            }
        }
        // A certain answer ends the search. A merely possible one does not:
        // the next, differently worded search may find the exact product.
        hunt.consult().await;
        if hunt.settled() {
            break;
        }
    }
    hunt.finish(searched_ok, last_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(title: &str, score: u8, ai: Option<AiView>) -> Opened {
        Opened {
            candidate: Candidate { sku: title.into(), url: format!("https://www.walmart.com/ip/{title}"), title: title.into() },
            obs: Observation { title: Some(title.into()), ..Default::default() },
            mode: FetchMode::Http,
            facts: Facts { title: title.into(), ..Default::default() },
            verdict: Verdict { score, reason: if score >= SAME { "Same barcode" } else { "Similar name" } },
            ai,
        }
    }

    /// How a finished hunt with these opened pages is reported: status, title, by AI.
    async fn outcome(pages: Vec<Opened>, ai_ruled_out: bool) -> String {
        let app = crate::test_app().await;
        let source = Facts { title: "Pens 12 Pack".into(), ..Default::default() };
        let store = search::builtin().remove(2);
        let hunt = Hunt {
            fetcher: &app.fetcher,
            source: &source,
            store: &store,
            advisor: None,
            opened: pages,
            seen: HashSet::new(),
            ai_ruled_out,
        };
        match hunt.finish(true, None) {
            StoreResult::Found(f) => {
                let kind = if f.score >= SAME { "same" } else { "possible" };
                let pack = f.pack.map(|p| format!(" [{} for {}]", p.other_qty, p.source_qty)).unwrap_or_default();
                format!("{kind}: {} ({}{}){pack}", f.candidate.title, if f.by_ai { "AI: " } else { "" }, f.reason)
            }
            StoreResult::NotFound(Some(note)) => format!("not found: {note}"),
            StoreResult::NotFound(None) => "not found".into(),
            StoreResult::Failed(why) => format!("failed: {why}"),
        }
    }

    #[tokio::test]
    async fn a_barcode_beats_the_ai_and_the_ai_beats_a_similar_name() {
        let same = |why: &str| Some(AiView::Same(why.into(), None));
        // A barcode match needs nobody's opinion.
        assert_eq!(
            outcome(vec![opened("A", 70, same("Looks alike")), opened("B", 100, None)], false).await,
            "same: B (Same barcode)"
        );
        // No barcode: the AI's pick is taken, even over a closer-sounding name.
        assert_eq!(
            outcome(vec![opened("A", 80, Some(AiView::Different)), opened("B", 62, same("Same model, same colour"))], false)
                .await,
            "same: B (AI: Same model, same colour)"
        );
        // The AI was not sure, or was not asked: the closest name is only a possible match.
        assert_eq!(
            outcome(vec![opened("A", 72, Some(AiView::Unsure)), opened("B", 65, None)], false).await,
            "possible: A (Similar name; your AI could not tell)"
        );
        // The AI ruled the close ones out: nothing is offered, and the reason is given.
        assert_eq!(
            outcome(vec![opened("A", 72, Some(AiView::Different)), opened("B", 30, None)], false).await,
            "not found: The closest results are different products, according to your AI"
        );
        // One ruled out, another still possible: the other is offered.
        assert_eq!(
            outcome(vec![opened("A", 80, Some(AiView::Different)), opened("B", 66, Some(AiView::Unsure))], false).await,
            "possible: B (Similar name; your AI could not tell)"
        );
        // Nothing opened because the AI saw nothing worth opening in the results.
        assert_eq!(outcome(vec![], true).await, "not found: Nothing in the search results is this product, according to your AI");
        assert_eq!(outcome(vec![], false).await, "not found");
        // Another pack size: the names' own amounts are used, and failing that the AI's count.
        assert_eq!(outcome(vec![opened("Pens 36 Pack", 95, None)], false).await, "same: Pens 36 Pack (Same barcode) [36 for 12]");
        let counted =
            Some(AiView::Same("Same pens".into(), Some(PackDiff { source_qty: 12.0, other_qty: 24.0, unit: "count".into() })));
        assert_eq!(
            outcome(vec![opened("Two dozen pens", 62, counted)], false).await,
            "same: Two dozen pens (AI: Same pens) [24 for 12]"
        );
    }
}
