//! A set of listing pairs with known answers, for checking how well matching
//! works: the rules on every test run, and a person's AI model on request
//! (`amepricetrack judge-test`).

use super::advisor::{Advisor, Judgement};
use super::matching::{self, Facts, PackDiff, SAME};
use serde::Deserialize;

/// The pairs that ship with the server.
pub const BUILTIN: &str = include_str!("../../tests/matching_cases.json");

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Expect {
    /// The same product in the same amount.
    Same,
    /// Not the same product.
    Different,
    /// The same item in a different amount (see `Case::pack`).
    Pack,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Listing {
    pub title: String,
    pub brand: Option<String>,
    pub model: Option<String>,
    /// In dollars, as a store shows it.
    pub price: Option<f64>,
    pub details: Option<String>,
}

impl Listing {
    pub fn facts(&self) -> Facts {
        Facts {
            title: self.title.clone(),
            brand: self.brand.clone(),
            model: self.model.clone(),
            price_cents: self.price.map(|p| (p * 100.0).round() as i64),
            details: self.details.clone(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    pub name: String,
    pub expect: Expect,
    /// For `Expect::Pack`: how many the tracked product and the listing hold.
    pub pack: Option<(f64, f64)>,
    /// The store the listing is from, as told to the AI.
    pub store: String,
    pub tracked: Listing,
    pub listing: Listing,
}

pub fn load(json: &str) -> anyhow::Result<Vec<Case>> {
    Ok(serde_json::from_str(json)?)
}

/// What a matcher said about one pair.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Same(Option<PackDiff>),
    Different,
    Unsure,
    /// The AI could not be asked or gave an unusable answer.
    Silent,
}

impl Answer {
    pub fn describe(&self) -> String {
        match self {
            Answer::Same(None) => "same".into(),
            Answer::Same(Some(p)) => format!("same item, {} against {}", p.source_qty, p.other_qty),
            Answer::Different => "different".into(),
            Answer::Unsure => "not sure".into(),
            Answer::Silent => "no answer".into(),
        }
    }
}

/// How an answer compares with what was expected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Grade {
    Right,
    /// Called two different things the same, or got the amounts wrong: this
    /// is the mistake that sends a false price alert.
    WrongSame,
    /// Failed to recognise a match. Costs a missed comparison, nothing worse.
    Missed,
    /// Did not commit either way.
    Unsure,
}

fn same_ratio(pack: &PackDiff, expected: (f64, f64)) -> bool {
    (pack.other_qty / pack.source_qty - expected.1 / expected.0).abs() < 1e-6
}

pub fn grade(case: &Case, answer: &Answer) -> Grade {
    match (case.expect, answer) {
        (_, Answer::Unsure | Answer::Silent) => Grade::Unsure,
        (Expect::Same, Answer::Same(None)) | (Expect::Different, Answer::Different) => Grade::Right,
        (Expect::Pack, Answer::Same(Some(pack))) if case.pack.is_some_and(|expected| same_ratio(pack, expected)) => Grade::Right,
        (Expect::Same | Expect::Pack, Answer::Different) => Grade::Missed,
        _ => Grade::WrongSame,
    }
}

/// What the rules alone say: a match only when a model number settles it.
pub fn by_rules(case: &Case) -> Answer {
    let (tracked, listing) = (case.tracked.facts(), case.listing.facts());
    if matching::judge(&tracked, &listing).score >= SAME {
        Answer::Same(matching::pack_difference(&tracked.title, &listing.title))
    } else {
        Answer::Unsure
    }
}

/// What the AI says, the way a real search would use its answer.
pub async fn by_ai(advisor: &Advisor, case: &Case) -> Answer {
    let (tracked, listing) = (case.tracked.facts(), case.listing.facts());
    match advisor.judge(&tracked, &case.store, std::slice::from_ref(&listing)).await {
        // As in a real search, amounts stated in the names win over the AI's count.
        Some(Judgement::Same { pack, .. }) => Answer::Same(matching::pack_difference(&tracked.title, &listing.title).or(pack)),
        Some(Judgement::Different { .. }) => Answer::Different,
        Some(Judgement::Unsure { .. }) => Answer::Unsure,
        None => Answer::Silent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_never_call_a_known_different_pair_the_same() {
        let cases = load(BUILTIN).unwrap();
        assert!(cases.len() >= 20);
        for case in &cases {
            let answer = by_rules(case);
            assert_ne!(grade(case, &answer), Grade::WrongSame, "{}: the rules said {}", case.name, answer.describe());
        }
        // The rules do settle some of them by themselves.
        assert!(cases.iter().any(|c| by_rules(c) == Answer::Same(None) && c.expect == Expect::Same));
        assert!(cases.iter().any(|c| matches!(by_rules(c), Answer::Same(Some(_))) && c.expect == Expect::Pack));
    }

    #[test]
    fn answers_are_graded_by_what_they_would_cost() {
        let cases = load(BUILTIN).unwrap();
        let pack = cases.iter().find(|c| c.expect == Expect::Pack).unwrap();
        let (tracked, listing) = pack.pack.unwrap();
        let right = PackDiff { source_qty: tracked, other_qty: listing, unit: "count".into() };
        // The same proportion in another unit (1 ream to 10 is 500 sheets to 5000) is right too.
        let scaled = PackDiff { source_qty: tracked * 2.0, other_qty: listing * 2.0, unit: "count".into() };
        assert_eq!(grade(pack, &Answer::Same(Some(right))), Grade::Right);
        assert_eq!(grade(pack, &Answer::Same(Some(scaled))), Grade::Right);
        assert_eq!(grade(pack, &Answer::Same(None)), Grade::WrongSame, "one for one would compare the wrong prices");
        assert_eq!(grade(pack, &Answer::Different), Grade::Missed);
        assert_eq!(grade(pack, &Answer::Unsure), Grade::Unsure);

        let different = cases.iter().find(|c| c.expect == Expect::Different).unwrap();
        assert_eq!(grade(different, &Answer::Same(None)), Grade::WrongSame);
        assert_eq!(grade(different, &Answer::Different), Grade::Right);
    }
}
