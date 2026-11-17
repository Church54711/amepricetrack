//! Decides whether two store listings are the same product.
//!
//! Evidence, strongest first: the same barcode, the same manufacturer model
//! number, then a similar name. Only the first two are trusted on their own;
//! a similar name is reported as a "possible" match for a person (or the AI
//! assistant) to confirm.

use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

/// A score at or above this is treated as the same product.
pub const SAME: u8 = 90;
/// A score at or above this (and below `SAME`) is shown as a possible match.
pub const POSSIBLE: u8 = 60;

/// What is known about a listing.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub title: String,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub gtin: Option<String>,
    /// Current price, when known. Used only as a sanity check.
    pub price_cents: Option<i64>,
    /// Barcodes of the listing's other options (colours, sizes).
    pub other_gtins: Vec<String>,
    /// Colour, size and a line of description, when the page gave them.
    /// Only the AI reads this; the rules below go by name and codes.
    pub details: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verdict {
    /// 0 to 100.
    pub score: u8,
    pub reason: &'static str,
}

static RE_PARENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(([^()]{3,40})\)").unwrap());
static RE_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9][A-Za-z0-9./\-]*").unwrap());
/// A plain measurement such as 64GB, 1080P or 20LBS: not a model number.
static RE_MEASURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d+(\.\d+)?(GB|TB|MB|KB|MM|CM|IN|INCH|FT|OZ|LB|LBS|PK|PACK|CT|W|V|MAH|AH|HZ|P|K|QT|GAL|ML|L|PC|PCS|MP|X|TH|ST|ND|RD|GSM|PSI|RPM|AMP|A|D|G)$").unwrap()
});

fn squash(text: &str) -> String {
    text.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase()
}

fn has_digit(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_digit())
}

fn has_letter(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_alphabetic())
}

/// Barcodes compared as digits without leading zeros, so a 12-digit UPC
/// equals the same code written as a 13 or 14-digit GTIN.
fn barcode(text: &str) -> Option<String> {
    let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
    let trimmed = digits.trim_start_matches('0');
    (trimmed.len() >= 7).then(|| trimmed.to_string())
}

/// Model-number-like codes for a listing: the model field, anything in
/// parentheses in the name, and words in the name that mix letters and digits.
pub fn model_codes(facts: &Facts) -> Vec<String> {
    let mut codes: Vec<String> = Vec::new();
    let mut add = |raw: &str, from_free_text: bool| {
        let code = squash(raw);
        if code.len() < 4 || code.len() > 24 || !has_digit(&code) || RE_MEASURE.is_match(&code) {
            return;
        }
        // A bare number in running text ("5000 Sheets") is usually a quantity.
        if from_free_text && (!has_letter(&code) || code.len() < 5) {
            return;
        }
        if !codes.contains(&code) {
            codes.push(code);
        }
    };
    if let Some(model) = &facts.model {
        add(model, false);
        for part in model.split(['/', ',', ' ', '|']) {
            add(part, false);
        }
    }
    for cap in RE_PARENS.captures_iter(&facts.title) {
        let inside = cap[1].trim();
        if !inside.contains(' ') {
            add(inside, false);
        }
    }
    for word in RE_WORD.find_iter(&facts.title) {
        add(word.as_str(), true);
    }
    codes
}

/// The code most worth typing into a store's search box, if there is one.
pub fn best_model_code(facts: &Facts) -> Option<String> {
    // Codes in parentheses in the name are the manufacturer's own; prefer those.
    for cap in RE_PARENS.captures_iter(&facts.title) {
        let inside = cap[1].trim();
        let code = squash(inside);
        if !inside.contains(' ') && code.len() >= 4 && has_digit(&code) && !RE_MEASURE.is_match(&code) {
            return Some(inside.to_string());
        }
    }
    let model = facts.model.as_deref()?;
    model
        .split(['/', ',', ' ', '|'])
        .map(str::trim)
        .find(|part| {
            let code = squash(part);
            code.len() >= 4 && has_digit(&code) && !RE_MEASURE.is_match(&code)
        })
        .map(str::to_string)
}

const FILLER: &[&str] =
    &["the", "and", "with", "for", "from", "new", "pack", "set", "inch", "in", "of", "by", "to", "a", "an", "x", "each", "per"];

fn name_words(title: &str, brand: Option<&str>) -> HashSet<String> {
    let brand_words: HashSet<String> =
        brand.unwrap_or("").split(|c: char| !c.is_ascii_alphanumeric()).map(|w| w.to_ascii_lowercase()).collect();
    title
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
        .map(|w| w.trim_matches('.').to_ascii_lowercase())
        .filter(|w| !w.is_empty() && (w.len() >= 2 || has_digit(w)))
        .filter(|w| !FILLER.contains(&w.as_str()) && !brand_words.contains(w))
        .collect()
}

/// 0.0 to 1.0: how much two product names share.
pub fn name_similarity(a: &Facts, b: &Facts) -> f64 {
    similarity_ignoring(a, b, &[])
}

/// Name similarity with the given model codes left out of both names.
fn similarity_ignoring(a: &Facts, b: &Facts, codes: &[String]) -> f64 {
    let words = |f: &Facts| -> HashSet<String> {
        name_words(&f.title, f.brand.as_deref()).into_iter().filter(|w| !codes.contains(&squash(w))).collect()
    };
    let (wa, wb) = (words(a), words(b));
    if wa.is_empty() || wb.is_empty() {
        return 0.0;
    }
    let shared = wa.intersection(&wb).count() as f64;
    let contained = shared / wa.len().min(wb.len()) as f64;
    let jaccard = shared / wa.union(&wb).count() as f64;
    0.65 * contained + 0.35 * jaccard
}

/// Numbers in a name (sizes, counts, capacities). Two listings of one product
/// rarely disagree on these; a 500-sheet ream is not a 5000-sheet carton.
fn numbers(title: &str) -> HashSet<String> {
    static RE_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+(?:\.\d+)?").unwrap());
    RE_NUMBER.find_iter(&title.replace(',', "")).map(|m| m.as_str().trim_end_matches(".0").to_string()).collect()
}

/// Sizes and counts stated with a unit: "5000 Sheets", "64GB", "2-Pack".
/// Returned per unit, so 500 sheets can be told apart from 5000 sheets.
fn quantities(title: &str) -> std::collections::HashMap<&'static str, HashSet<String>> {
    static RE_QUANTITY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(\d+(?:\.\d+)?)[\s-]*(sheets?|reams?|packs?|pk|count|ct|pieces?|pcs?|oz|ounces?|lbs?|pounds?|gb|tb|gal|gallons?|qt|quarts?|ml|ft|feet|rolls?|pairs?)\b")
            .unwrap()
    });
    let mut found: std::collections::HashMap<&'static str, HashSet<String>> = std::collections::HashMap::new();
    for cap in RE_QUANTITY.captures_iter(&title.replace(',', "")) {
        let unit = match cap[2].to_ascii_lowercase().trim_end_matches('s') {
            "sheet" => "sheets",
            "ream" => "reams",
            "pack" | "pk" => "pack",
            "count" | "ct" | "piece" | "pc" => "count",
            "oz" | "ounce" => "oz",
            "lb" | "pound" => "lb",
            "gb" => "gb",
            "tb" => "tb",
            "gal" | "gallon" => "gal",
            "qt" | "quart" => "qt",
            "ml" => "ml",
            "ft" | "feet" => "ft",
            "roll" => "rolls",
            "pair" => "pairs",
            _ => continue,
        };
        found.entry(unit).or_default().insert(cap[1].trim_end_matches(".0").to_string());
    }
    found
}

/// True when both names state a size or count in the same unit and disagree.
fn quantity_clash(a: &str, b: &str) -> bool {
    let (qa, qb) = (quantities(a), quantities(b));
    qa.iter().any(|(unit, numbers)| qb.get(unit).is_some_and(|other| numbers.is_disjoint(other)))
}

const COLOURS: &[&str] = &[
    "black",
    "white",
    "blue",
    "red",
    "green",
    "silver",
    "gold",
    "gray",
    "pink",
    "purple",
    "orange",
    "yellow",
    "brown",
    "beige",
    "ivory",
    "navy",
    "teal",
    "starlight",
    "midnight",
    "graphite",
    "canary",
    "cream",
    "tan",
    "bronze",
    "burgundy",
    "turquoise",
    "lavender",
    "charcoal",
    "titanium",
    "rose",
    "coral",
    "olive",
    "maroon",
    "mint",
    "sage",
];

fn colours(title: &str) -> HashSet<&'static str> {
    title
        .split(|c: char| !c.is_ascii_alphabetic())
        .map(|w| w.to_ascii_lowercase())
        .filter_map(|w| {
            let w = if w == "grey" { "gray".to_string() } else { w };
            COLOURS.iter().copied().find(|c| *c == w)
        })
        .collect()
}

/// True when both names say a colour and have none in common.
fn colour_clash(a: &str, b: &str) -> bool {
    let (ca, cb) = (colours(a), colours(b));
    !ca.is_empty() && !cb.is_empty() && ca.is_disjoint(&cb)
}

#[derive(PartialEq)]
enum BrandCheck {
    Same,
    Unknown,
    Different,
}

fn compare_brands(a: &Facts, b: &Facts) -> BrandCheck {
    let known = |f: &Facts| f.brand.as_deref().map(squash).filter(|s| s.len() >= 2);
    match (known(a), known(b)) {
        (Some(x), Some(y)) if x.contains(&y) || y.contains(&x) => BrandCheck::Same,
        // Stores write brands differently ("HP" vs "HP Inc."); the other
        // listing's name mentioning the brand also counts.
        (Some(x), Some(y)) => {
            if squash(&b.title).contains(&x) || squash(&a.title).contains(&y) {
                BrandCheck::Same
            } else {
                BrandCheck::Different
            }
        }
        (Some(x), None) => {
            if squash(&b.title).contains(&x) {
                BrandCheck::Same
            } else {
                BrandCheck::Unknown
            }
        }
        (None, Some(y)) => {
            if squash(&a.title).contains(&y) {
                BrandCheck::Same
            } else {
                BrandCheck::Unknown
            }
        }
        (None, None) => BrandCheck::Unknown,
    }
}

/// How likely `candidate` is the same product as `source`.
pub fn judge(source: &Facts, candidate: &Facts) -> Verdict {
    let barcodes = (source.gtin.as_deref().and_then(barcode), candidate.gtin.as_deref().and_then(barcode));
    if let (Some(a), Some(b)) = &barcodes {
        if a == b {
            return Verdict { score: 100, reason: "Same barcode" };
        }
    }
    // The barcode may belong to one of the other listing's colour or size options.
    let option_of = |code: &Option<String>, other: &Facts| {
        code.as_ref().is_some_and(|c| other.other_gtins.iter().filter_map(|g| barcode(g)).any(|g| &g == c))
    };
    if option_of(&barcodes.0, candidate) || option_of(&barcodes.1, source) {
        return Verdict { score: 96, reason: "Same barcode (one of this listing's options)" };
    }
    let barcodes_differ = matches!(&barcodes, (Some(a), Some(b)) if a != b);
    let brands = compare_brands(source, candidate);

    let (codes_a, codes_b) = (model_codes(source), model_codes(candidate));
    let exact = codes_a.iter().any(|a| codes_b.contains(a));
    // "105007" on one site and "105007CT" on another.
    let partial = !exact
        && codes_a.iter().any(|a| {
            codes_b.iter().any(|b| {
                let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
                short.len() >= 5 && long.starts_with(short.as_str()) && long.len() - short.len() <= 3
            })
        });
    // The other listing's name mentions the model code without spacing.
    let mentioned = !exact && !partial && {
        let (name_a, name_b) = (squash(&source.title), squash(&candidate.title));
        codes_a.iter().any(|c| c.len() >= 6 && has_letter(c) && name_b.contains(c.as_str()))
            || codes_b.iter().any(|c| c.len() >= 6 && has_letter(c) && name_a.contains(c.as_str()))
    };

    let similarity = name_similarity(source, candidate);
    let number_clash = {
        let (na, nb) = (numbers(&source.title), numbers(&candidate.title));
        !na.is_empty() && !nb.is_empty() && na.is_disjoint(&nb)
    };

    let mut verdict = if exact {
        Verdict { score: 92, reason: "Same model number" }
    } else if partial || mentioned {
        Verdict { score: 90, reason: "Same model number" }
    } else {
        let mut score = 30.0 + 55.0 * similarity;
        if number_clash {
            score -= 15.0;
        }
        Verdict { score: score.clamp(0.0, 85.0) as u8, reason: "Similar name" }
    };

    // Accessories quote the model they fit ("charger for DCD771C2"). So a
    // shared code is only trusted when the rest of the names also overlap
    // and the prices are in the same range.
    if verdict.score >= SAME {
        let all_codes: Vec<String> = codes_a.iter().chain(codes_b.iter()).cloned().collect();
        if similarity_ignoring(source, candidate, &all_codes) < 0.2 {
            verdict = Verdict { score: 75, reason: "Same model number, different name" };
        } else if let (Some(a), Some(b)) = (source.price_cents, candidate.price_cents) {
            let ratio = b as f64 / a.max(1) as f64;
            if !(0.4..=2.5).contains(&ratio) {
                verdict = Verdict { score: 75, reason: "Same model number, very different price" };
            }
        }
    }
    // One model number often covers several pack sizes (a ream and a carton).
    if quantity_clash(&source.title, &candidate.title) {
        if verdict.score >= SAME {
            verdict = Verdict { score: 78, reason: "Same model number, different size or quantity" };
        } else {
            verdict.score = verdict.score.saturating_sub(15);
        }
    }
    // The same model in another colour is a different thing to buy.
    if colour_clash(&source.title, &candidate.title) {
        if verdict.score >= SAME {
            verdict = Verdict { score: 78, reason: "Same model number, different colour" };
        } else {
            verdict.score = verdict.score.saturating_sub(20);
            verdict.reason = "Similar name, different colour";
        }
    }
    if brands == BrandCheck::Different {
        verdict.score = verdict.score.min(45);
        verdict.reason = "Different brand";
    }
    // Two different barcodes usually mean a different pack size or colour.
    if barcodes_differ && verdict.score >= SAME {
        verdict = Verdict { score: 80, reason: "Same model number, different barcode" };
    } else if barcodes_differ {
        verdict.score = verdict.score.min(70);
    }
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(title: &str, brand: Option<&str>, model: Option<&str>, gtin: Option<&str>) -> Facts {
        Facts {
            title: title.into(),
            brand: brand.map(Into::into),
            model: model.map(Into::into),
            gtin: gtin.map(Into::into),
            ..Default::default()
        }
    }

    #[test]
    fn same_barcode_wins_regardless_of_name_or_padding() {
        let a = facts("AirPods Max 2 - Blue", Some("Apple"), Some("MHWM4AM/A"), Some("195951039305"));
        let b = facts("Apple AirPods Max (2nd gen) Over-Ear Headphones, Blue", Some("Apple"), None, Some("0195951039305"));
        assert_eq!(judge(&a, &b), Verdict { score: 100, reason: "Same barcode" });
    }

    #[test]
    fn model_number_matches_across_store_spellings() {
        // Staples writes the part number as "105007CT/27061"; others as "105007".
        let staples = facts(
            "Hammermill Copy Plus 8.5\" x 11\" Copy Paper, 20 lbs., 92 Brightness, 5000 Sheets/Carton (105007)",
            Some("Hammermill"),
            Some("105007CT/27061"),
            None,
        );
        let walmart = facts(
            "Hammermill Copy Plus Paper, 8.5 x 11, 20 lb, 92 Bright, 5000 Sheets, 10 Reams (105007)",
            Some("Hammermill"),
            Some("105007"),
            None,
        );
        let verdict = judge(&staples, &walmart);
        assert!(verdict.score >= SAME, "{verdict:?}");
        assert_eq!(verdict.reason, "Same model number");

        let bestbuy = facts("Apple - AirPods 4 with Active Noise Cancellation - White", Some("Apple"), Some("MXP93LL/A"), None);
        let amazon =
            facts("Apple AirPods 4 Wireless Earbuds with Active Noise Cancellation MXP93LL/A", Some("Apple"), None, None);
        assert!(judge(&bestbuy, &amazon).score >= SAME);
    }

    #[test]
    fn same_model_with_a_different_barcode_needs_confirming() {
        // Two colours of one model: the names barely differ, the barcodes do.
        let white = facts("Sony WH-1000XM5 Wireless Headphones, Silver", Some("Sony"), Some("WH1000XM5/S"), Some("027242923232"));
        let black = facts("Sony WH-1000XM5 Wireless Headphones, Black", Some("Sony"), Some("WH1000XM5/B"), Some("027242923225"));
        let verdict = judge(&white, &black);
        assert!(verdict.score < SAME && verdict.score >= POSSIBLE, "{verdict:?}");
        assert_eq!(verdict.reason, "Same model number, different colour");

        // Same colour, two barcodes (a regional or packaging variant): still worth a look.
        let other_pack =
            facts("Sony WH-1000XM5 Wireless Headphones, Silver", Some("Sony"), Some("WH1000XM5/S"), Some("027242999999"));
        let verdict = judge(&white, &other_pack);
        assert_eq!(verdict, Verdict { score: 80, reason: "Same model number, different barcode" });

        let blue = facts("AirPods Max 2 - Blue", Some("Apple"), None, None);
        let starlight = facts("Apple - AirPods Max 2 (USB-C) - Starlight", Some("Apple"), None, None);
        assert!(judge(&blue, &starlight).score < POSSIBLE);
        let also_blue = facts("Apple - AirPods Max 2 (USB-C) - Blue", Some("Apple"), None, None);
        assert!(judge(&blue, &also_blue).score >= POSSIBLE);
    }

    #[test]
    fn similar_names_are_only_a_possible_match() {
        let a = facts("1/2 in. Drive, 10 to 150 ft-lb Click Torque Wrench", Some("PITTSBURGH"), None, None);
        let b = facts("Pittsburgh 1/2 in. Drive Click Torque Wrench 10-150 ft-lb", None, None, None);
        let verdict = judge(&a, &b);
        assert!(verdict.score >= POSSIBLE && verdict.score < SAME, "{verdict:?}");
        assert_eq!(verdict.reason, "Similar name");
    }

    #[test]
    fn different_products_score_low() {
        let paper = facts("Hammermill Copy Plus Paper, 8.5 x 11, 5000 Sheets (105007)", Some("Hammermill"), Some("105007"), None);
        let other_brand = facts("HP Printer Paper, 8.5 x 11, 5000 Sheets", Some("HP"), None, None);
        let verdict = judge(&paper, &other_brand);
        assert!(verdict.score < POSSIBLE, "{verdict:?}");
        assert_eq!(verdict.reason, "Different brand");

        let unrelated = facts("Stainless Steel Water Bottle 32 oz", Some("Hammermill"), None, None);
        assert!(judge(&paper, &unrelated).score < POSSIBLE);

        // Sizes and counts are not model numbers.
        let tv_a = facts("Samsung 65\" Class 4K Smart TV 120HZ", Some("Samsung"), None, None);
        let tv_b = facts("Samsung Soundbar 120HZ compatible 300W", Some("Samsung"), None, None);
        assert!(judge(&tv_a, &tv_b).score < POSSIBLE);
    }

    #[test]
    fn a_shared_code_with_unrelated_names_is_not_trusted() {
        let a = facts("Cordless Drill Kit (DCD771C2)", Some("DeWalt"), None, None);
        let b = facts("Replacement battery charger DCD771C2 compatible", None, None, None);
        let verdict = judge(&a, &b);
        assert!(verdict.score < SAME, "{verdict:?}");
    }

    #[test]
    fn a_shared_code_at_a_very_different_price_is_not_trusted() {
        let earbuds = Facts {
            price_cents: Some(15999),
            ..facts("Apple AirPods 4 with Active Noise Cancellation MXP93LL/A", Some("Apple"), None, None)
        };
        let same = Facts {
            price_cents: Some(16900),
            ..facts("Apple - AirPods 4 with Active Noise Cancellation - White", Some("Apple"), Some("MXP93LL/A"), None)
        };
        assert!(judge(&earbuds, &same).score >= SAME);
        let case = Facts {
            price_cents: Some(899),
            ..facts("Case for Apple AirPods 4 with Active Noise Cancellation MXP93LL/A", None, None, None)
        };
        let verdict = judge(&earbuds, &case);
        assert_eq!(verdict, Verdict { score: 75, reason: "Same model number, very different price" });
    }

    #[test]
    fn barcode_of_one_colour_matches_a_listing_covering_all_colours() {
        let blue = facts("AirPods Max 2 - Blue", Some("Apple"), None, Some("195951039305"));
        let all_colours = Facts {
            other_gtins: vec!["195951039299".into(), "195951039305".into()],
            ..facts("Apple AirPods Max 2", Some("Apple"), None, None)
        };
        assert_eq!(judge(&blue, &all_colours).score, 96);
        let other_product =
            Facts { other_gtins: vec!["111111111111".into()], ..facts("Apple AirPods Max 2", Some("Apple"), None, None) };
        assert!(judge(&blue, &other_product).score < SAME);
    }

    #[test]
    fn same_code_in_a_different_pack_size_needs_confirming() {
        let carton = facts(
            "Hammermill Copy Plus Copy Paper, 20 lbs., 5000 Sheets/Carton (105007)",
            Some("Hammermill"),
            Some("105007CT"),
            None,
        );
        let ream =
            facts("Hammermill Copy Plus Copy Paper, 20 lbs., 500 Sheets/Ream (105007)", Some("Hammermill"), Some("105007"), None);
        assert_eq!(judge(&carton, &ream), Verdict { score: 78, reason: "Same model number, different size or quantity" });
        let same_carton = facts("Hammermill Copy Plus Paper, 20lb, 5,000 Sheets / Case (105007)", Some("Hammermill"), None, None);
        assert!(judge(&carton, &same_carton).score >= SAME);
        assert!(!quantity_clash("SSD 512GB", "SSD 1TB 512 GB cache"));
        assert!(quantity_clash("Towels 2-Pack", "Towels, 6 Pack"));
        assert!(!quantity_clash("Towels 2-Pack", "Towels"), "nothing to disagree with");
    }

    #[test]
    fn codes_found_in_names_and_model_fields() {
        let f = facts("Sony WH-1000XM5 Headphones, 30 Hours, 5000 mAh (WH1000XM5/B)", Some("Sony"), Some("WH1000XM5/B"), None);
        let codes = model_codes(&f);
        assert!(codes.contains(&"WH1000XM5".to_string()), "{codes:?}");
        assert!(!codes.contains(&"5000".to_string()), "quantities are not codes: {codes:?}");
        assert_eq!(best_model_code(&f).as_deref(), Some("WH1000XM5/B"));

        let paper = facts("Copy Paper (105007)", None, Some("105007CT/27061"), None);
        assert_eq!(best_model_code(&paper).as_deref(), Some("105007"));
        let plain = facts("Bath Towel Set", None, None, None);
        assert_eq!(best_model_code(&plain), None);
    }
}
