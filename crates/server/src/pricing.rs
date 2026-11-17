//! What a product costs *for a particular person*: the public price, a
//! signed-in price their phone reported, and their personal discounts.

use crate::db::{DiscountRule, Product, DAY};
use std::collections::HashMap;

/// Formats cents as dollars: 129900 -> "$1,299.00".
pub fn dollars(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let cents = cents.abs();
    let whole = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{sign}${grouped}.{:02}", cents % 100)
}

/// Reads a price typed by a person: "19.99", "$1,200", "20".
pub fn parse_dollars(s: &str) -> Option<i64> {
    let cleaned: String = s.trim().chars().filter(|c| !matches!(c, '$' | ',' | ' ')).collect();
    if cleaned.is_empty() {
        return None;
    }
    let value: f64 = cleaned.parse().ok()?;
    (value.is_finite() && value > 0.0 && value < 10_000_000.0).then(|| (value * 100.0).round() as i64)
}

fn list_items(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split([',', '\n']).map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty())
}

fn rule_applies(rule: &DiscountRule, product: &Product) -> bool {
    if !rule.enabled || rule.retailer != product.retailer || rule.percent <= 0.0 {
        return false;
    }
    let title = product.title.as_deref().unwrap_or("").to_lowercase();
    let brand = product.brand.as_deref().unwrap_or("").to_lowercase();
    if list_items(&rule.exclude_keywords).any(|word| title.contains(&word) || brand.contains(&word)) {
        return false;
    }
    let mut brands = list_items(&rule.brands).peekable();
    if brands.peek().is_none() {
        return true;
    }
    // Store pages do not always fill in the brand, so a title that starts with it counts too.
    brands.any(|b| brand == b || title.starts_with(&format!("{b} ")))
}

/// The most generous discount that applies to this product.
pub fn best_rule<'a>(rules: &'a [DiscountRule], product: &Product) -> Option<&'a DiscountRule> {
    rules.iter().filter(|r| rule_applies(r, product)).max_by(|a, b| a.percent.total_cmp(&b.percent))
}

pub fn apply_percent(cents: i64, percent: f64) -> i64 {
    ((cents as f64) * (1.0 - percent.clamp(0.0, 100.0) / 100.0)).round() as i64
}

/// The prices that matter for one person and one product.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PriceView {
    /// Price anyone sees.
    pub public_cents: Option<i64>,
    /// Price seen while signed in at the store, if recent.
    pub member_cents: Option<i64>,
    /// Name and percent of the personal discount applied.
    pub discount_name: Option<String>,
    pub discount_percent: Option<f64>,
    /// What this person would actually pay.
    pub your_cents: Option<i64>,
}

impl PriceView {
    /// True when the personal price is lower than the public one.
    pub fn is_personal(&self) -> bool {
        matches!((self.your_cents, self.public_cents), (Some(y), Some(p)) if y < p)
            || (self.public_cents.is_none() && self.member_cents.is_some())
    }
}

pub fn view(product: &Product, member_cents: Option<i64>, rules: &[DiscountRule]) -> PriceView {
    let public = product.current_price_cents;
    let base = match (public, member_cents) {
        (Some(p), Some(m)) => Some(p.min(m)),
        (p, m) => p.or(m),
    };
    let rule = best_rule(rules, product);
    PriceView {
        public_cents: public,
        member_cents,
        discount_name: rule.map(|r| r.name.clone()),
        discount_percent: rule.map(|r| r.percent),
        your_cents: base.map(|b| rule.map_or(b, |r| apply_percent(b, r.percent))),
    }
}

/// Average public price over the last `days`, weighting each price by how
/// long it lasted. `changes` are (time, cents) in time order. Returns None
/// when the history covers less than a week, since an average of two days
/// says little about the usual price.
pub fn average_price(changes: &[(i64, i64)], now: i64, days: i64) -> Option<i64> {
    let first = changes.first()?;
    if now - first.0 < 7 * DAY {
        return None;
    }
    let start = (now - days * DAY).max(first.0);
    let mut total = 0f64;
    let mut span = 0f64;
    for (i, (ts, price)) in changes.iter().enumerate() {
        let from = (*ts).max(start);
        let until = changes.get(i + 1).map_or(now, |next| next.0);
        if until > from {
            total += (*price as f64) * (until - from) as f64;
            span += (until - from) as f64;
        }
    }
    (span > 0.0).then(|| (total / span).round() as i64)
}

/// Groups (product id, time, cents) rows by product.
pub fn group_changes(rows: Vec<(i64, i64, i64)>) -> HashMap<i64, Vec<(i64, i64)>> {
    let mut map: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
    for (product_id, ts, cents) in rows {
        map.entry(product_id).or_default().push((ts, cents));
    }
    map
}

/// How good the current price is, for ranking deals.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DealInfo {
    /// 90-day average public price, when there is enough history.
    pub usual_cents: Option<i64>,
    /// Percent your price is below the usual price (or below list price when
    /// there is not enough history yet).
    pub percent_below: Option<f64>,
    /// What `percent_below` is measured against: "average" or "list".
    pub compared_to: Option<&'static str>,
    /// The public price equals the lowest ever recorded.
    pub at_lowest: bool,
}

pub fn deal_info(product: &Product, view: &PriceView, changes: &[(i64, i64)], now: i64) -> DealInfo {
    let usual = average_price(changes, now, 90);
    let (reference, compared_to) = match (usual, product.list_price_cents) {
        (Some(avg), _) => (Some(avg), Some("average")),
        (None, Some(list)) => (Some(list), Some("list")),
        (None, None) => (None, None),
    };
    let percent_below = match (reference, view.your_cents) {
        (Some(r), Some(y)) if r > 0 => Some(((r - y) as f64 / r as f64) * 100.0),
        _ => None,
    };
    let at_lowest = match (product.current_price_cents, product.lowest_price_cents) {
        // Only meaningful once the price has moved at least once.
        (Some(now_price), Some(low)) => now_price <= low && changes.len() > 1,
        _ => false,
    };
    DealInfo { usual_cents: usual, percent_below, compared_to: compared_to.filter(|_| percent_below.is_some()), at_lowest }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product(retailer: &str, title: &str, brand: Option<&str>, price: i64) -> Product {
        Product {
            id: 1,
            retailer: retailer.into(),
            retailer_sku: "1".into(),
            url: "https://example.com/p".into(),
            title: Some(title.into()),
            brand: brand.map(Into::into),
            model: None,
            gtin: None,
            image_url: None,
            current_price_cents: Some(price),
            list_price_cents: None,
            in_stock: Some(true),
            promo_text: None,
            lowest_price_cents: Some(price),
            lowest_price_at: None,
            fetch_mode: None,
            created_at: 0,
            last_checked_at: None,
            last_success_at: None,
            next_check_at: 0,
            fail_count: 0,
            last_error: None,
            details: None,
        }
    }

    fn rule(retailer: &str, name: &str, percent: f64, brands: &str, exclude: &str) -> DiscountRule {
        DiscountRule {
            id: 0,
            user_id: 1,
            retailer: retailer.into(),
            name: name.into(),
            percent,
            brands: brands.into(),
            exclude_keywords: exclude.into(),
            enabled: true,
            created_at: 0,
        }
    }

    #[test]
    fn formats_and_parses_money() {
        assert_eq!(dollars(129900), "$1,299.00");
        assert_eq!(dollars(5), "$0.05");
        assert_eq!(dollars(100000000), "$1,000,000.00");
        assert_eq!(parse_dollars("$1,200"), Some(120000));
        assert_eq!(parse_dollars(" 19.99 "), Some(1999));
        assert_eq!(parse_dollars(""), None);
        assert_eq!(parse_dollars("free"), None);
        assert_eq!(parse_dollars("-5"), None);
    }

    #[test]
    fn staples_employee_discount_picks_the_better_brand_rule() {
        let rules = vec![
            rule("staples", "Associate 10%", 10.0, "", "gift card, postage"),
            rule("staples", "Associate 25% Staples brands", 25.0, "TRU RED, Staples", "gift card"),
        ];
        let paper = product("staples", "Hammermill Copy Plus Paper", Some("Hammermill"), 4999);
        let v = view(&paper, None, &rules);
        assert_eq!(v.your_cents, Some(4499));
        assert_eq!(v.discount_percent, Some(10.0));

        let own = product("staples", "TRU RED 8.5 x 11 Copy Paper", Some("TRU RED"), 4000);
        assert_eq!(view(&own, None, &rules).your_cents, Some(3000));

        // Brand missing from the page, but the title starts with it.
        let untagged = product("staples", "Staples Multipurpose Paper", None, 2000);
        assert_eq!(view(&untagged, None, &rules).your_cents, Some(1500));

        let card = product("staples", "Visa Gift Card $50", None, 5000);
        let v = view(&card, None, &rules);
        assert_eq!(v.your_cents, Some(5000));
        assert!(!v.is_personal());

        // A rule for one store never touches another.
        let tv = product("bestbuy", "Staples-like TV", None, 10000);
        assert_eq!(view(&tv, None, &rules).your_cents, Some(10000));
    }

    #[test]
    fn signed_in_price_stacks_with_discount() {
        let rules = vec![rule("staples", "Associate 10%", 10.0, "", "")];
        let p = product("staples", "Chair", None, 20000);
        let v = view(&p, Some(18000), &rules);
        assert_eq!(v.your_cents, Some(16200));
        assert!(v.is_personal());
        // A stale, higher signed-in price never makes things worse than public.
        assert_eq!(view(&p, Some(25000), &[]).your_cents, Some(20000));
    }

    #[test]
    fn average_weights_by_time() {
        let now = 100 * DAY;
        // $100 for 30 days, then $50 for 10 days: (100*30 + 50*10) / 40 = 87.50
        let changes = vec![(60 * DAY, 10000), (90 * DAY, 5000)];
        assert_eq!(average_price(&changes, now, 90), Some(8750));
        // Only the last 5 days: all at $50.
        assert_eq!(average_price(&changes, now, 5), Some(5000));
        // Less than a week of history: no average yet.
        assert_eq!(average_price(&[(97 * DAY, 10000)], now, 90), None);
        assert_eq!(average_price(&[], now, 90), None);
    }

    #[test]
    fn deal_is_measured_against_average_then_list() {
        let now = 100 * DAY;
        let mut p = product("walmart", "Thing", None, 5000);
        p.list_price_cents = Some(8000);
        p.lowest_price_cents = Some(5000);
        let v = view(&p, None, &[]);
        let d = deal_info(&p, &v, &[(60 * DAY, 10000), (90 * DAY, 5000)], now);
        assert_eq!(d.compared_to, Some("average"));
        assert!((d.percent_below.unwrap() - 42.857).abs() < 0.01);
        assert!(d.at_lowest);

        let d = deal_info(&p, &v, &[(99 * DAY, 5000)], now);
        assert_eq!(d.compared_to, Some("list"));
        assert!((d.percent_below.unwrap() - 37.5).abs() < 0.01);
        assert!(!d.at_lowest, "a single reading is not a record low");
    }
}
