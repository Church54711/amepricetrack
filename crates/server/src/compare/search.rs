//! Looking a product up at another store: what to type into the store's
//! search, and picking the product links out of the results page.

use super::matching::{best_model_code, Facts};
use crate::retailers::{self, FetchMode};
use scraper::{Html, Selector};
use std::sync::LazyLock;
use url::Url;

/// How to search one store. `{q}` in the address is replaced by the query.
#[derive(Debug, Clone)]
pub struct StoreSearch {
    pub retailer: String,
    pub name: String,
    pub url: String,
    /// How the results page is read. `None` for stores people added: a plain
    /// request first, then the browser.
    pub mode: Option<FetchMode>,
}

const BUILTIN: &[(&str, &str, FetchMode)] = &[
    ("staples", "https://www.staples.com/{q}/directory_{q}", FetchMode::Browser),
    ("bestbuy", "https://www.bestbuy.com/site/searchpage.jsp?st={q}", FetchMode::Browser),
    ("walmart", "https://www.walmart.com/search?q={q}", FetchMode::Http),
    ("target", "https://www.target.com/s?searchTerm={q}", FetchMode::Browser),
    ("amazon", "https://www.amazon.com/s?k={q}", FetchMode::Http),
    ("kohls", "https://www.kohls.com/search.jsp?search={q}", FetchMode::Browser),
    ("harborfreight", "https://www.harborfreight.com/search?q={q}", FetchMode::Browser),
    ("hottopic", "https://www.hottopic.com/search?q={q}", FetchMode::Http),
];

/// The stores with a dedicated reader.
pub fn builtin() -> Vec<StoreSearch> {
    BUILTIN
        .iter()
        .map(|(id, url, mode)| StoreSearch {
            retailer: id.to_string(),
            name: retailers::display_name(id),
            url: url.to_string(),
            mode: Some(*mode),
        })
        .collect()
}

impl StoreSearch {
    pub fn address(&self, query: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        self.url.replace("{q}", &encoded)
    }
}

/// Search phrases to try, most precise first: the barcode, then brand and
/// model number, then the start of the product name.
pub fn queries(source: &Facts) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |q: String| {
        let q = q.split_whitespace().collect::<Vec<_>>().join(" ");
        if q.len() >= 4 && !out.iter().any(|existing| existing.eq_ignore_ascii_case(&q)) {
            out.push(q);
        }
    };

    if let Some(gtin) = &source.gtin {
        let digits: String = gtin.chars().filter(|c| c.is_ascii_digit()).collect();
        // Stores list the 12-digit UPC; a 13 or 14-digit code with leading zeros is the same thing.
        let upc = if digits.len() > 12 { digits.trim_start_matches('0').to_string() } else { digits };
        if upc.len() >= 8 {
            add(format!("{upc:0>12}"));
        }
    }

    let brand = source.brand.as_deref().unwrap_or("").trim();
    if let Some(code) = best_model_code(source) {
        add(format!("{brand} {code}"));
    }

    // The name without bracketed codes and punctuation, cut to its first words.
    let mut name = String::new();
    let mut depth = 0;
    for ch in source.title.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth > 0 => {}
            c if c.is_alphanumeric() || c == '.' || c == '-' || c == '/' => name.push(c),
            _ => name.push(' '),
        }
    }
    let mut words: Vec<&str> = name.split_whitespace().filter(|w| w.chars().any(|c| c.is_alphanumeric())).collect();
    words.truncate(9);
    let name = words.join(" ");
    if !brand.is_empty() && !name.to_ascii_lowercase().contains(&brand.to_ascii_lowercase()) {
        add(format!("{brand} {name}"));
    } else {
        add(name);
    }
    out
}

/// A product link found on a results page.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub sku: String,
    pub url: String,
    /// The link's text, which is usually the product name. May be empty.
    pub title: String,
}

impl Candidate {
    /// Text to judge the link by before opening it: its name, helped by the
    /// words in its address when the name is short ("Blue") or missing.
    pub fn description(&self) -> String {
        if self.title.split_whitespace().count() >= 3 {
            return self.title.clone();
        }
        let slug = Url::parse(&self.url)
            .ok()
            .and_then(|u| {
                u.path_segments().and_then(|segments| {
                    segments.filter(|s| s.matches('-').count() >= 2).max_by_key(|s| s.len()).map(str::to_string)
                })
            })
            .unwrap_or_default();
        let slug = slug.trim_end_matches(".html").trim_end_matches(".jsp").trim_end_matches(".p").replace('-', " ");
        format!("{} {}", self.title, slug).trim().to_string()
    }
}

static SEL_LINK: LazyLock<Selector> = LazyLock::new(|| Selector::parse("a[href]").unwrap());
static SEL_IMG: LazyLock<Selector> = LazyLock::new(|| Selector::parse("img[alt]").unwrap());
/// Links inside the product cards most shop software draws its results as.
/// Menus and banners also link products; these are the results themselves.
static SEL_CARD_LINK: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        ".product-tile a[href], .product-card a[href], .product-item a[href], .grid-product a[href],          .product-grid-item a[href], [data-pid] a[href], [data-product-id] a[href]",
    )
    .unwrap()
});

fn tidy(text: &str) -> String {
    // Staples appends an internal picture reference to names after this marker.
    let text = text.split("~#|#~").next().unwrap_or(text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Path fragments that mark a product page at most shops.
const PRODUCT_PATHS: &[&str] =
    &["/products/", "/product/", "/p/", "/dp/", "/ip/", "/item/", "/items/", "/pd/", "/prod/", "/sku/"];
/// Path fragments of pages that are never a product.
const OTHER_PATHS: &[&str] = &[
    "/search",
    "/cart",
    "/account",
    "/login",
    "/signin",
    "/register",
    "/checkout",
    "/wishlist",
    "/help",
    "/blog",
    "/pages/",
    "/collections",
    "/category",
    "/categories",
    "/brands",
    "/stores",
    "/store-locator",
    "/gift-card",
    "/customer-service",
    "/policies",
    "/contact",
    "/about",
    "/careers",
    "/c/",
    "/b/",
];

/// For a shop without a dedicated reader: could this address be a product
/// page, and does it follow one of the usual product address shapes?
fn generic_link_kind(path: &str) -> Option<bool> {
    let lower = path.to_ascii_lowercase();
    let usual = PRODUCT_PATHS.iter().any(|p| lower.contains(p));
    if !usual && OTHER_PATHS.iter().any(|p| lower.starts_with(p)) {
        return None;
    }
    Some(usual)
}

/// Every distinct product of `retailer` linked from a page, in page order.
pub fn product_links(retailer: &str, page_url: &str, html: &str) -> Vec<Candidate> {
    let Ok(base) = Url::parse(page_url) else {
        return vec![];
    };
    let generic = retailers::by_id(retailer).is_none();
    let doc = Html::parse_document(html);
    let mut found: Vec<Candidate> = Vec::new();
    // For shops without a dedicated reader: which found links have a usual product address.
    let mut usual: Vec<bool> = Vec::new();
    // Shops on common shop software: when the page has product cards, only those count.
    let in_cards = generic || retailer == "hottopic";
    let mut links: Vec<scraper::ElementRef> = if in_cards { doc.select(&SEL_CARD_LINK).collect() } else { vec![] };
    if links.is_empty() {
        links = doc.select(&SEL_LINK).collect();
    }
    for link in links {
        let Some(href) = link.value().attr("href") else { continue };
        let Ok(absolute) = base.join(href) else { continue };
        let Ok(product) = retailers::identify(absolute.as_str()) else { continue };
        if product.retailer != retailer {
            continue;
        }
        let is_usual = if generic {
            // The page itself (a "sort by" link, say) is not a result.
            if absolute.path() == base.path() {
                continue;
            }
            match generic_link_kind(absolute.path()) {
                Some(usual) => usual,
                None => continue,
            }
        } else {
            true
        };
        // A product card has several links (picture, name, reviews). The
        // longest sensible text among them is the product name.
        let mut labels = vec![tidy(&link.text().collect::<String>())];
        for attr in ["aria-label", "title"] {
            if let Some(value) = link.value().attr(attr) {
                labels.push(tidy(value));
            }
        }
        for img in link.select(&SEL_IMG) {
            labels.push(tidy(img.value().attr("alt").unwrap_or("")));
        }
        let title = labels.into_iter().filter(|l| l.len() <= 300).max_by_key(|l| l.len()).unwrap_or_default();

        match found.iter_mut().find(|c| c.sku == product.sku) {
            Some(existing) => {
                if title.len() > existing.title.len() {
                    existing.title = title;
                }
            }
            None => {
                found.push(Candidate { sku: product.sku, url: product.url, title });
                usual.push(is_usual);
            }
        }
    }
    // When a shop uses a usual product address shape, its other links
    // (menus, brands, articles) are not results.
    if generic && usual.iter().any(|u| *u) {
        let mut keep = usual.into_iter();
        found.retain(|_| keep.next().unwrap_or(false));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_shops_keep_only_product_looking_links() {
        let html = r#"<html><body>
            <a href="/">Home</a>
            <a href="/collections/funko">Funko</a>
            <a href="/search?q=funko&sort=price">Sort by price</a>
            <a href="/products/funko-pop-stitch?variant=1">Funko Pop! Disney Stitch Vinyl Figure</a>
            <a href="/products/funko-pop-angel">Funko Pop! Disney Angel Vinyl Figure</a>
            <a href="/pages/shipping">Shipping</a>
            <a href="/blogs/news/new-arrivals">New arrivals this week</a>
            </body></html>"#;
        let found = product_links("shop.example", "https://shop.example/search?q=funko", html);
        let names: Vec<&str> = found.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(names, vec!["Funko Pop! Disney Stitch Vinyl Figure", "Funko Pop! Disney Angel Vinyl Figure"]);

        // Product cards, when the page has them, are the results; a product in the menu is not.
        let html = r#"<html><body>
            <nav><a href="/product/featured-hoodie/11111111.html">Featured Hoodie On Sale Now</a></nav>
            <div class="product-tile" data-pid="22222222"><a href="/product/stitch-figure/22222222.html">Stitch Vinyl Figure</a></div>
            </body></html>"#;
        let found = product_links("hottopic", "https://www.hottopic.com/search?q=stitch", html);
        assert_eq!(found.iter().map(|c| c.sku.as_str()).collect::<Vec<_>>(), vec!["22222222"]);

        // A shop with its own address shape: menus are dropped, the rest kept for judging by name.
        let html = r#"<html><body>
            <a href="/cart">Cart</a>
            <a href="/category/toys">Toys</a>
            <a href="/stitch-vinyl-figure-12345.html">Stitch Vinyl Figure</a>
            <a href="/toys/angel-vinyl-figure-777">Angel Vinyl Figure</a>
            </body></html>"#;
        let found = product_links("shop.example", "https://shop.example/find?text=stitch", html);
        let names: Vec<&str> = found.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(names, vec!["Stitch Vinyl Figure", "Angel Vinyl Figure"]);
    }

    #[test]
    fn queries_go_from_precise_to_loose() {
        let paper = Facts {
            title: "Hammermill Copy Plus 8.5\" x 11\" Copy Paper, 20 lbs., 92 Brightness, 5000 Sheets/Carton (105007)".into(),
            brand: Some("Hammermill".into()),
            model: Some("105007CT/27061".into()),
            gtin: Some("0010199005106".into()),
            ..Default::default()
        };
        assert_eq!(queries(&paper), vec!["010199005106", "Hammermill 105007", "Hammermill Copy Plus 8.5 x 11 Copy Paper 20"]);

        let towels = Facts {
            title: "Madison Park Signature 800GSM Towel Set".into(),
            brand: Some("Madison Park".into()),
            ..Default::default()
        };
        assert_eq!(queries(&towels), vec!["Madison Park Signature 800GSM Towel Set"]);

        let no_brand_in_name =
            Facts { title: "1/2 in. Drive Click Torque Wrench".into(), brand: Some("PITTSBURGH".into()), ..Default::default() };
        assert_eq!(queries(&no_brand_in_name), vec!["PITTSBURGH 1/2 in. Drive Click Torque Wrench"]);
    }

    #[test]
    fn search_address_is_encoded() {
        let stores = builtin();
        let walmart = stores.iter().find(|s| s.retailer == "walmart").unwrap();
        assert_eq!(walmart.address("Hammermill 105007 & more"), "https://www.walmart.com/search?q=Hammermill+105007+%26+more");
    }

    #[test]
    fn short_link_names_borrow_words_from_the_address() {
        let swatch = Candidate {
            sku: "6550083".into(),
            url: "https://www.bestbuy.com/product/apple-airpods-max-2-usb-c-blue/JJGCQYH953/sku/6550083".into(),
            title: "Blue".into(),
        };
        assert_eq!(swatch.description(), "Blue apple airpods max 2 usb c blue");
        let named = Candidate { title: "Apple AirPods Max 2 (USB-C)".into(), ..swatch.clone() };
        assert_eq!(named.description(), "Apple AirPods Max 2 (USB-C)");
        let bare = Candidate { sku: "1".into(), url: "https://www.walmart.com/ip/1".into(), title: String::new() };
        assert_eq!(bare.description(), "");
    }

    #[test]
    fn finds_each_product_once_with_its_name() {
        let html = r#"<html><body>
            <a href="/ip/Hammermill-Copy-Plus/111222333?athbdg=L1600"><img alt="Hammermill Copy Plus Paper"></a>
            <a href="/ip/Hammermill-Copy-Plus/111222333"><span>Hammermill Copy Plus Paper, 5000 Sheets~#|#~0A9D_sc7</span></a>
            <a href="/ip/Hammermill-Copy-Plus/111222333#reviews">(4.7)</a>
            <a href="https://www.walmart.com/ip/Other-Paper/444555666" aria-label="HP Printer Paper 500 Sheets"></a>
            <a href="/search?q=paper&page=2">Next</a>
            <a href="https://www.amazon.com/dp/B000000000">Elsewhere</a>
            </body></html>"#;
        let found = product_links("walmart", "https://www.walmart.com/search?q=paper", html);
        assert_eq!(
            found,
            vec![
                Candidate {
                    sku: "111222333".into(),
                    url: "https://www.walmart.com/ip/111222333".into(),
                    title: "Hammermill Copy Plus Paper, 5000 Sheets".into()
                },
                Candidate {
                    sku: "444555666".into(),
                    url: "https://www.walmart.com/ip/444555666".into(),
                    title: "HP Printer Paper 500 Sheets".into()
                },
            ]
        );
    }
}
