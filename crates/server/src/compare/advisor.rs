//! Asking the person's AI service for help with matching: which search
//! results are worth opening, and whether an opened listing is the same
//! product. The rules in `matching` stay in charge of anything a barcode or
//! model number settles; the AI is asked about the rest.

use super::matching::Facts;
use super::search::Candidate;
use crate::ai::{self, AiConfig, Options};
use crate::pricing::dollars;
use serde_json::{json, Value};

/// Search results shown to the AI at most.
const MAX_RESULTS: usize = 20;

pub struct Advisor {
    http: reqwest::Client,
    cfg: AiConfig,
}

/// The AI's answer about a handful of listings from one store.
#[derive(Debug, Clone, PartialEq)]
pub enum Judgement {
    /// Listing `index` is the same product.
    Same { index: usize, why: String },
    /// None of the listings is the same product.
    Different { why: String },
    /// It could not tell. `index` is the listing it leaned towards, if any.
    Unsure { index: Option<usize> },
}

const PICK_SYSTEM: &str = "\
You help a price tracker find one product at another store. You are given the product and the numbered results of \
searching that store for it. Choose the results that could be that same product, likeliest first.

Leave out anything that is clearly something else: an accessory or part for the product, a different model or \
generation, an unrelated item the store padded the results with. Keep a result that differs only in colour, size or \
pack count, because its page links to the other options. Store names for one product are worded differently, so go \
by what the item is, not by matching words. If no result could be the product, give an empty list.

Reply with JSON only, in this shape: {\"results\": [3, 1]}";

const JUDGE_SYSTEM: &str = "\
You decide whether a listing at another store is the same product a shopper is tracking, so the two prices can be \
compared fairly. A wrong \"same\" sends the shopper a false price alert, so it is the costlier mistake: say it only \
when the details support it.

It is the same only if buying either one gives the shopper the same thing: the same brand, model or edition, \
generation, size or capacity, colour, and pack count. These are not the same: an accessory or part for the product; \
a different generation or model; a bundle with extras; a multi-pack against a single item, or packs of different \
sizes; a different colour.

Product lines such as collectible figures, toys, clothing and cosmetics have many near-identical variants. When one \
name carries a variant, edition, character detail or item number that the other does not (\"with Bags\", \"Easter\", \
\"Deluxe\", \"Flocked\", \"#1568\"), they are different products unless the other details show they are the same one. \
When the tracked product's details are too thin to tell such variants apart, you are not sure.

A listing that offers several colours or sizes on one page is the same if the shopper's option is plausibly among \
them. Stores word names differently, so go by what the item is. A much higher or lower price suggests a different \
quantity or item, but is not proof on its own.

You are given the tracked product and one or more numbered listings from the other store. Reply with JSON only, in \
this shape: {\"differences\": \"Listing 1 adds shopping bags; listing 2 none\", \"match\": 2, \"sure\": true, \
\"why\": \"Same model and colour\"}
- differences: for each listing, what sets it apart from the tracked product, or \"none\". Work this out first.
- match: the number of the listing that is the same product, or 0 if none of them is.
- sure: false when the details given are not enough to tell.
- why: the deciding fact, in at most twelve words.";

fn describe(f: &Facts) -> String {
    let mut lines = vec![format!("name: {}", f.title)];
    let mut add = |label: &str, value: Option<&str>| {
        if let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) {
            lines.push(format!("{label}: {value}"));
        }
    };
    add("brand", f.brand.as_deref());
    add("model number", f.model.as_deref());
    add("barcode", f.gtin.as_deref());
    add("price", f.price_cents.map(dollars).as_deref());
    add("details", f.details.as_deref());
    if f.other_gtins.len() > 1 {
        lines.push("this listing offers several colours or sizes on one page".into());
    }
    lines.join("\n")
}

/// The JSON object in an answer, tolerating code fences or words around it.
fn json_in(answer: &str) -> Option<Value> {
    let start = answer.find('{')?;
    let end = answer.rfind('}')?;
    serde_json::from_str(answer.get(start..=end)?).ok()
}

/// Turns the AI's list of result numbers into positions in `count` results.
fn parse_picks(answer: &str, count: usize) -> Option<Vec<usize>> {
    let value = json_in(answer)?;
    let numbers = value.get("results")?.as_array()?;
    let mut picks: Vec<usize> = Vec::new();
    for n in numbers.iter().filter_map(Value::as_u64) {
        let n = n as usize;
        if (1..=count).contains(&n) && !picks.contains(&(n - 1)) {
            picks.push(n - 1);
        }
    }
    Some(picks)
}

fn parse_judgement(answer: &str, count: usize) -> Option<Judgement> {
    let value = json_in(answer)?;
    let chosen = value.get("match")?.as_u64()? as usize;
    if chosen > count {
        return None;
    }
    let sure = value.get("sure").and_then(Value::as_bool).unwrap_or(false);
    let why: String =
        value.get("why").and_then(Value::as_str).unwrap_or("").trim().trim_end_matches('.').chars().take(90).collect();
    Some(match (chosen, sure) {
        (0, true) => Judgement::Different { why },
        (0, false) => Judgement::Unsure { index: None },
        (n, true) => Judgement::Same { index: n - 1, why },
        (n, false) => Judgement::Unsure { index: Some(n - 1) },
    })
}

impl Advisor {
    pub fn new(http: &reqwest::Client, cfg: AiConfig) -> Self {
        Advisor { http: http.clone(), cfg }
    }

    /// `quick` is for choices that are cheap to get wrong (which results to
    /// open); judging gets the model's normal effort.
    async fn ask(&self, system: &str, question: String, schema: Value, quick: bool) -> Option<String> {
        let options = Options { json_schema: Some(schema), quick };
        match ai::complete_with(&self.http, &self.cfg, system, &[("user".to_string(), question)], &options).await {
            Ok(answer) => Some(answer),
            Err(e) => {
                tracing::warn!("AI matching help failed: {e}");
                None
            }
        }
    }

    /// Which of a store's search results could be the product: positions in
    /// `results`, likeliest first. Only the first `MAX_RESULTS` are looked at.
    /// `None` when the AI could not be asked or did not answer usably.
    pub async fn pick_results(&self, source: &Facts, store: &str, results: &[Candidate]) -> Option<Vec<usize>> {
        let shown = &results[..results.len().min(MAX_RESULTS)];
        let list: Vec<String> = shown.iter().enumerate().map(|(i, c)| format!("{}. {}", i + 1, c.description())).collect();
        let question = format!("The product\n{}\n\nSearch results at {store}\n{}", describe(source), list.join("\n"));
        let schema = json!({
            "type": "object",
            "properties": { "results": { "type": "array", "items": { "type": "integer" } } },
            "required": ["results"],
            "additionalProperties": false
        });
        let answer = self.ask(PICK_SYSTEM, question, schema, true).await?;
        let picks = parse_picks(&answer, shown.len());
        if picks.is_none() {
            tracing::warn!("AI gave an unusable answer when choosing search results: {answer}");
        }
        picks
    }

    /// Whether one of `listings` (opened pages at one store) is the same product.
    pub async fn judge(&self, source: &Facts, store: &str, listings: &[Facts]) -> Option<Judgement> {
        let list: Vec<String> = listings.iter().enumerate().map(|(i, f)| format!("Listing {}\n{}", i + 1, describe(f))).collect();
        let question = format!("The tracked product\n{}\n\nAt {store}\n\n{}", describe(source), list.join("\n\n"));
        let schema = json!({
            "type": "object",
            "properties": {
                "differences": { "type": "string" },
                "match": { "type": "integer" },
                "sure": { "type": "boolean" },
                "why": { "type": "string" }
            },
            "required": ["differences", "match", "sure", "why"],
            "additionalProperties": false
        });
        let answer = self.ask(JUDGE_SYSTEM, question, schema, false).await?;
        let judgement = parse_judgement(&answer, listings.len());
        if judgement.is_none() {
            tracing::warn!("AI gave an unusable answer when judging listings: {answer}");
        }
        judgement
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_chosen_results() {
        assert_eq!(parse_picks(r#"{"results": [3, 1]}"#, 5), Some(vec![2, 0]));
        assert_eq!(parse_picks("```json\n{\"results\": []}\n```", 5), Some(vec![]));
        // Numbers outside the list and repeats are dropped.
        assert_eq!(parse_picks(r#"Sure: {"results": [2, 9, 0, 2]}"#, 3), Some(vec![1]));
        assert_eq!(parse_picks("none of them", 3), None);
        assert_eq!(parse_picks(r#"{"picks": [1]}"#, 3), None);
    }

    #[test]
    fn reads_the_judgement() {
        assert_eq!(
            parse_judgement(r#"{"match": 2, "sure": true, "why": "Same model and colour."}"#, 3),
            Some(Judgement::Same { index: 1, why: "Same model and colour".into() })
        );
        assert_eq!(
            parse_judgement(r#"{"match": 0, "sure": true, "why": "A three-pack"}"#, 1),
            Some(Judgement::Different { why: "A three-pack".into() })
        );
        assert_eq!(parse_judgement(r#"{"match": 1, "sure": false, "why": ""}"#, 1), Some(Judgement::Unsure { index: Some(0) }));
        assert_eq!(parse_judgement(r#"{"match": 0, "sure": false, "why": ""}"#, 1), Some(Judgement::Unsure { index: None }));
        // A listing number that was never shown is not an answer.
        assert_eq!(parse_judgement(r#"{"match": 4, "sure": true, "why": "x"}"#, 2), None);
        assert_eq!(parse_judgement("SAME", 2), None);
    }

    /// A stand-in Claude service that always answers with `text`. Returns
    /// its address and what it was asked.
    async fn canned(text: &'static str) -> (String, std::sync::Arc<std::sync::Mutex<Vec<Value>>>) {
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = asked.clone();
        let handler = move |axum::Json(body): axum::Json<Value>| {
            recorder.lock().unwrap().push(body);
            async move { axum::Json(json!({ "content": [{ "type": "text", "text": text }], "stop_reason": "end_turn" })) }
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, axum::Router::new().fallback(handler)).await.unwrap() });
        (address, asked)
    }

    fn advisor(address: String) -> Advisor {
        let cfg =
            AiConfig { kind: ai::Kind::Anthropic, base_url: address, api_key: "k".into(), model: "claude-haiku-5-5".into() };
        Advisor::new(&reqwest::Client::new(), cfg)
    }

    #[tokio::test]
    async fn asks_the_service_about_listings_and_results() {
        let source = Facts { title: "AirPods Max 2 Blue".into(), brand: Some("Apple".into()), ..Default::default() };
        let listing =
            Facts { title: "Apple AirPods Max (2nd gen) - Blue".into(), price_cents: Some(44900), ..Default::default() };

        let (address, asked) = canned(r#"{"match": 1, "sure": true, "why": "Same headphones, same colour"}"#).await;
        let judgement = advisor(address).judge(&source, "Walmart", &[listing]).await;
        assert_eq!(judgement, Some(Judgement::Same { index: 0, why: "Same headphones, same colour".into() }));
        let question = asked.lock().unwrap()[0]["messages"][0]["content"].as_str().unwrap().to_string();
        assert!(question.contains("The tracked product\nname: AirPods Max 2 Blue\nbrand: Apple"), "{question}");
        assert!(
            question.contains("At Walmart\n\nListing 1\nname: Apple AirPods Max (2nd gen) - Blue\nprice: $449.00"),
            "{question}"
        );
        assert_eq!(asked.lock().unwrap()[0]["model"], "claude-haiku-5-5");

        let results: Vec<Candidate> = ["AirPods Max case", "Apple AirPods Max 2 Blue", "Sony WH-1000XM6"]
            .iter()
            .enumerate()
            .map(|(i, title)| Candidate {
                sku: i.to_string(),
                url: format!("https://www.walmart.com/ip/{i}"),
                title: title.to_string(),
            })
            .collect();
        let (address, asked) = canned(r#"{"results": [2]}"#).await;
        assert_eq!(advisor(address).pick_results(&source, "Walmart", &results).await, Some(vec![1]));
        let question = asked.lock().unwrap()[0]["messages"][0]["content"].as_str().unwrap().to_string();
        assert!(
            question.ends_with("Search results at Walmart\n1. AirPods Max case\n2. Apple AirPods Max 2 Blue\n3. Sony WH-1000XM6"),
            "{question}"
        );

        // An answer that is not what was asked for counts as no answer.
        let (address, _) = canned("I think the second one.").await;
        assert_eq!(advisor(address).pick_results(&source, "Walmart", &results).await, None);
    }

    #[test]
    fn a_listing_is_described_with_what_is_known() {
        let facts = Facts {
            title: "AirPods Max 2".into(),
            brand: Some("Apple".into()),
            price_cents: Some(44900),
            details: Some("Colour: Blue".into()),
            other_gtins: vec!["1".into(), "2".into()],
            ..Default::default()
        };
        assert_eq!(
            describe(&facts),
            "name: AirPods Max 2\nbrand: Apple\nprice: $449.00\ndetails: Colour: Blue\n\
             this listing offers several colours or sizes on one page"
        );
    }
}
