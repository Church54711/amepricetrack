//! Getting a product's price history out as a spreadsheet file, and putting
//! prices from before tracking started in: pasted as dates and prices, or
//! fetched from Keepa for Amazon products.

use crate::db::{self, PricePoint};
use crate::service::{user_problem, Problem};
use crate::App;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Date, OffsetDateTime, PrimitiveDateTime, Time};

/// One price from the past.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PastPrice {
    pub ts: i64,
    pub cents: i64,
}

/// More past prices than this are thinned out; a chart cannot show them anyway.
const MOST_POINTS: usize = 5000;

// ------------------------------------------------------------------- export

fn csv_field(text: &str) -> String {
    // A value a spreadsheet would run as a formula is made plain text.
    let text = if text.starts_with(['=', '+', '-', '@']) { format!("'{text}") } else { text.to_string() };
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

fn money(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

/// The history as a CSV file: one line per recorded change, oldest first.
pub fn to_csv(points: &[PricePoint]) -> String {
    let mut out = String::from("time,price,list_price,in_stock,kind,source\n");
    for p in points {
        let time = OffsetDateTime::from_unix_timestamp(p.ts).ok().and_then(|t| t.format(&Rfc3339).ok()).unwrap_or_default();
        let stock = match p.in_stock {
            Some(true) => "yes",
            Some(false) => "no",
            None => "",
        };
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            time,
            money(p.price_cents),
            p.list_price_cents.map(money).unwrap_or_default(),
            stock,
            if p.member { "signed-in" } else { "listed" },
            csv_field(&p.source),
        ));
    }
    out
}

/// A name for the downloaded file, made from the product's title.
pub fn file_name(title: &str) -> String {
    let mut name = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c.to_ascii_lowercase());
        } else if !name.ends_with('-') && !name.is_empty() {
            name.push('-');
        }
        if name.len() >= 50 {
            break;
        }
    }
    let name = name.trim_matches('-');
    format!("{}-prices.csv", if name.is_empty() { "product" } else { name })
}

// ------------------------------------------------------------------- pasted

/// A day or a moment written the ways people and spreadsheets write them.
fn parse_when(text: &str) -> Option<i64> {
    let text = text.trim().trim_matches('"');
    if let Ok(moment) = OffsetDateTime::parse(text, &Rfc3339) {
        return Some(moment.unix_timestamp());
    }
    // A long number is seconds (or milliseconds) since 1970.
    if text.len() >= 9 && text.chars().all(|c| c.is_ascii_digit()) {
        let n: i64 = text.parse().ok()?;
        return Some(if n > 100_000_000_000 { n / 1000 } else { n });
    }
    let with_time = [
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second]"),
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]"),
        format_description!("[year]-[month]-[day] [hour]:[minute]"),
        format_description!("[year]-[month]-[day]T[hour]:[minute]"),
    ];
    for format in with_time {
        if let Ok(moment) = PrimitiveDateTime::parse(text, format) {
            return Some(moment.assume_utc().unix_timestamp());
        }
    }
    let day_only = [
        format_description!("[year]-[month]-[day]"),
        format_description!("[month padding:none]/[day padding:none]/[year]"),
        format_description!("[year]/[month padding:none]/[day padding:none]"),
        format_description!("[month repr:short] [day padding:none], [year]"),
        format_description!("[month repr:short] [day padding:none] [year]"),
    ];
    // Only the day part of "2025-03-01 extra words" style values is tried too.
    let first_word = text.split_whitespace().next().unwrap_or(text);
    for candidate in [text, first_word] {
        for format in day_only {
            if let Ok(day) = Date::parse(candidate, format) {
                // Noon, so the day reads the same in every US time zone.
                return Some(PrimitiveDateTime::new(day, Time::from_hms(12, 0, 0).ok()?).assume_utc().unix_timestamp());
            }
        }
    }
    None
}

/// Splits one line of pasted text into its columns: tabs, semicolons or
/// commas, with "quoted, values" kept whole.
fn columns(line: &str) -> Vec<String> {
    let separator = if line.contains('\t') {
        '\t'
    } else if line.contains(';') {
        ';'
    } else {
        ','
    };
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c == separator && !quoted => out.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    out.push(current);
    out.into_iter().map(|v| v.trim().to_string()).collect()
}

/// Reads pasted lines of "date, price". Also takes this server's own export.
/// The first column that reads as a date and the first after it that reads
/// as money are used; a heading line and blank lines are skipped.
pub fn parse_pasted(text: &str, now: i64) -> Result<Vec<PastPrice>, String> {
    let mut found = Vec::new();
    let mut unread = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cells = columns(line);
        // A signed-in price in this server's own export is one person's, not the listed price.
        if cells.iter().any(|c| c == "signed-in") {
            continue;
        }
        // The date, and where the columns after it start. "Mar 1, 2025, 19.99" has a
        // comma inside its date, so two neighbouring columns are tried as one as well.
        let dated = cells.iter().enumerate().find_map(|(at, cell)| parse_when(cell).map(|ts| (ts, at + 1))).or_else(|| {
            cells
                .windows(2)
                .enumerate()
                .find_map(|(at, pair)| parse_when(&format!("{}, {}", pair[0], pair[1])).map(|ts| (ts, at + 2)))
        });
        let reading = dated.and_then(|(ts, rest)| {
            let cents = cells.iter().skip(rest).find_map(|c| crate::pricing::parse_dollars(c));
            cents.map(|cents| PastPrice { ts, cents })
        });
        match reading {
            Some(price) if price.ts < now && price.ts > 0 => found.push(price),
            Some(_) => {}
            None if number == 0 => {} // a heading
            None => unread.push(number + 1),
        }
    }
    if found.is_empty() {
        return Err("No dates with prices were found. Put one per line, such as: 2025-03-01, 19.99".into());
    }
    if unread.len() > found.len() {
        return Err(format!("Most lines could not be read (line {} is the first). Put one date and price per line.", unread[0]));
    }
    Ok(found)
}

// -------------------------------------------------------------------- Keepa

/// Keepa counts minutes from the start of 2011.
const KEEPA_EPOCH_MINUTES: i64 = 21_564_000;

/// Reads the price history out of Keepa's answer for one product: the price
/// Amazon itself charged, or the lowest new offer when Amazon never sold it.
pub fn parse_keepa(body: &str) -> Result<Vec<PastPrice>, String> {
    let json: serde_json::Value = serde_json::from_str(body).map_err(|_| "Keepa's answer could not be read.".to_string())?;
    if let Some(message) = json.pointer("/error/message").and_then(|m| m.as_str()) {
        return Err(format!("Keepa said: {message}"));
    }
    let product = json.pointer("/products/0").ok_or("Keepa does not know this product.")?;
    let series = |index: usize| -> Vec<PastPrice> {
        let Some(values) = product.pointer(&format!("/csv/{index}")).and_then(|v| v.as_array()) else {
            return vec![];
        };
        // Pairs of (time, price); a price of -1 means nothing was on offer then.
        values
            .as_chunks::<2>()
            .0
            .iter()
            .filter_map(|pair| {
                let (minutes, cents) = (pair[0].as_i64()?, pair[1].as_i64()?);
                (cents > 0).then_some(PastPrice { ts: (minutes + KEEPA_EPOCH_MINUTES) * 60, cents })
            })
            .collect()
    };
    let amazon = series(0);
    let found = if amazon.is_empty() { series(1) } else { amazon };
    if found.is_empty() {
        return Err("Keepa has no price history for this product.".into());
    }
    Ok(found)
}

/// Asks Keepa for an Amazon product's history, with the person's own key.
pub async fn from_keepa(app: &App, key: &str, asin: &str) -> Result<Vec<PastPrice>, Problem> {
    let asked = app
        .public_http
        .get("https://api.keepa.com/product")
        .query(&[("key", key), ("domain", "1"), ("asin", asin)])
        .timeout(std::time::Duration::from_secs(40))
        .send()
        .await
        .map_err(|e| user_problem(format!("Keepa could not be reached ({}).", crate::fetch::root_cause(&e))))?;
    let status = asked.status().as_u16();
    let body = asked.text().await.unwrap_or_default();
    match status {
        200 => parse_keepa(&body).map_err(user_problem),
        400 => Err(user_problem("Keepa did not accept the request. Check the key on the Settings page.")),
        402 => Err(user_problem("Keepa says this key has no paid data plan.")),
        429 => Err(user_problem("Keepa says this key has used up its allowance for now. Try again in a few minutes.")),
        other => Err(user_problem(format!("Keepa answered with an error ({other})."))),
    }
}

// ------------------------------------------------------------------- import

/// Keeps one point per change of price, oldest first, and no more than the limit.
fn tidy(mut points: Vec<PastPrice>, before: i64) -> Vec<PastPrice> {
    points.retain(|p| p.ts > 0 && p.ts < before && p.cents > 0);
    points.sort_by_key(|p| p.ts);
    points.dedup_by_key(|p| p.ts);
    let mut changes: Vec<PastPrice> = Vec::new();
    for point in points {
        if changes.last().is_none_or(|last| last.cents != point.cents) {
            changes.push(point);
        }
    }
    if changes.len() > MOST_POINTS {
        // The most recent ones matter most.
        changes.drain(..changes.len() - MOST_POINTS);
    }
    changes
}

/// Puts past prices into a product's history. They only fill the time
/// before the server's own first reading, and replace what was imported
/// before. Returns how many were kept.
///
/// The history is shared, so this is for the administrator, or for a
/// product nobody else relies on.
pub async fn import(app: &App, user_id: i64, product_id: i64, points: Vec<PastPrice>) -> Result<usize, Problem> {
    if db::watch(&app.db, user_id, product_id).await?.is_none() {
        return Err(user_problem("That product is not on your list."));
    }
    if !db::is_admin(&app.db, user_id).await? && db::others_rely_on(&app.db, product_id, user_id).await? {
        return Err(user_problem("Someone else tracks this product too, so only the administrator can add past prices to it."));
    }
    let first_own: Option<i64> =
        sqlx::query_scalar("SELECT MIN(ts) FROM price_points WHERE product_id = ? AND user_id IS NULL AND source != 'import'")
            .bind(product_id)
            .fetch_one(&app.db)
            .await?;
    let points = tidy(points, first_own.unwrap_or_else(db::now));
    if points.is_empty() {
        return Err(user_problem("All of those prices are from after tracking started here, so there was nothing to add."));
    }

    let mut tx = app.db.begin().await?;
    sqlx::query("DELETE FROM price_points WHERE product_id = ? AND source = 'import'").bind(product_id).execute(&mut *tx).await?;
    for point in &points {
        sqlx::query("INSERT INTO price_points (product_id, user_id, ts, price_cents, source) VALUES (?, NULL, ?, ?, 'import')")
            .bind(product_id)
            .bind(point.ts)
            .bind(point.cents)
            .execute(&mut *tx)
            .await?;
    }
    // The lowest price on record may now be an older one.
    sqlx::query(
        "UPDATE products SET
            lowest_price_cents = (SELECT MIN(price_cents) FROM price_points WHERE product_id = ?1 AND user_id IS NULL),
            lowest_price_at = (SELECT ts FROM price_points WHERE product_id = ?1 AND user_id IS NULL
                               ORDER BY price_cents, ts DESC LIMIT 1)
         WHERE id = ?1 AND EXISTS (SELECT 1 FROM price_points WHERE product_id = ?1 AND user_id IS NULL)",
    )
    .bind(product_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(points.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_795_000_000; // late 2026

    fn point(ts: i64, cents: i64, member: bool, source: &str) -> PricePoint {
        PricePoint { ts, price_cents: cents, list_price_cents: None, in_stock: Some(true), source: source.into(), member }
    }

    #[test]
    fn the_export_is_a_plain_spreadsheet_file() {
        let csv = to_csv(&[point(1_700_000_000, 6279, false, "server"), point(1_700_086_400, 5999, true, "device")]);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "time,price,list_price,in_stock,kind,source");
        assert_eq!(lines[1], "2023-11-14T22:13:20Z,62.79,,yes,listed,server");
        assert_eq!(lines[2], "2023-11-15T22:13:20Z,59.99,,yes,signed-in,device");
        assert_eq!(csv_field("=cmd()"), "'=cmd()");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
    }

    #[test]
    fn file_names_are_made_from_the_title() {
        assert_eq!(file_name("TRU RED 8.5\" x 11\" Copy Paper"), "tru-red-8-5-x-11-copy-paper-prices.csv");
        assert_eq!(file_name("???"), "product-prices.csv");
    }

    #[test]
    fn pasted_prices_are_read_in_the_usual_spellings() {
        let text = "Date,Price\n2025-03-01,19.99\n3/15/2025\t$1,234.50\n2025-04-01T08:30:00Z;17.49\nMar 20, 2025, 18.25\n";
        let read = parse_pasted(text, NOW).unwrap();
        assert_eq!(read.iter().map(|p| p.cents).collect::<Vec<_>>(), vec![1999, 123_450, 1749, 1825]);
        assert_eq!(read[0].ts, 1_740_830_400, "noon UTC on the day");
        assert_eq!(read[2].ts, 1_743_496_200);
    }

    #[test]
    fn this_servers_own_export_can_be_pasted_back() {
        let csv = to_csv(&[point(1_700_000_000, 6279, false, "server"), point(1_700_086_400, 5999, true, "device")]);
        let read = parse_pasted(&csv, NOW).unwrap();
        assert_eq!(read, vec![PastPrice { ts: 1_700_000_000, cents: 6279 }], "the signed-in price is left out");
    }

    #[test]
    fn text_that_is_not_prices_is_refused() {
        assert!(parse_pasted("", NOW).is_err());
        assert!(parse_pasted("hello\nworld\n", NOW).is_err());
        // Prices dated in the future are not history.
        assert!(parse_pasted("2090-01-01,5.00", NOW).is_err());
        let mostly_noise = "2025-03-01,19.99\nnoise\nmore noise\nand more\n";
        assert!(parse_pasted(mostly_noise, NOW).is_err());
    }

    #[test]
    fn keepa_history_is_read_from_amazons_own_price() {
        // Amazon's price: 49.00, then nothing on offer, then 45.00. Marketplace: 60.00.
        let body = r#"{"products":[{"asin":"B000000000","csv":[[411180,4900,500000,-1,600000,4500],[411180,6000]]}]}"#;
        let read = parse_keepa(body).unwrap();
        assert_eq!(
            read,
            vec![PastPrice { ts: 1_318_510_800, cents: 4900 }, PastPrice { ts: (600_000 + 21_564_000) * 60, cents: 4500 }]
        );
        // Never sold by Amazon itself: the lowest new offer is used.
        let third_party = r#"{"products":[{"csv":[null,[411180,6000]]}]}"#;
        assert_eq!(parse_keepa(third_party).unwrap()[0].cents, 6000);
        assert!(parse_keepa(r#"{"products":[]}"#).is_err());
        assert!(parse_keepa(r#"{"error":{"message":"invalid key"}}"#).unwrap_err().contains("invalid key"));
        assert!(parse_keepa("<html>").is_err());
    }

    #[test]
    fn imported_prices_keep_only_the_changes_before_tracking() {
        let raw = vec![
            PastPrice { ts: 300, cents: 900 },
            PastPrice { ts: 100, cents: 1000 },
            PastPrice { ts: 200, cents: 1000 },
            PastPrice { ts: 300, cents: 950 },
            PastPrice { ts: 5000, cents: 100 },
        ];
        let kept = tidy(raw, 1000);
        assert_eq!(kept.iter().map(|p| (p.ts, p.cents)).collect::<Vec<_>>(), vec![(100, 1000), (300, 900)]);
    }
}
