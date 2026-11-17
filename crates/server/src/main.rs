//! AmePriceTrack: a self-hosted price tracker.
//!
//! One program serves the website and the phone API, checks prices in the
//! background, and sends alerts. Data lives in a single SQLite file.

mod ai;
mod alerts;
mod api;
mod auth;
mod backup;
mod browser;
mod compare;
mod config;
mod db;
mod fetch;
mod find;
mod health;
mod net;
mod pricing;
mod retailers;
mod scheduler;
mod service;
mod stores;
mod ui;
mod web;

use axum::extract::DefaultBodyLimit;
use std::sync::Arc;
use tokio::sync::Notify;

pub struct AppState {
    pub db: db::Db,
    pub cfg: config::Config,
    pub fetcher: fetch::Fetcher,
    /// For AI services and alert pushes.
    pub http: reqwest::Client,
    /// The same, for people who may only reach public addresses (see `net`).
    pub public_http: reqwest::Client,
    /// Wakes the price checker early.
    pub wake: Notify,
    /// Wakes the worker that looks products up at other stores.
    pub compare_wake: Notify,
}

pub type App = Arc<AppState>;

/// Product pages sent by the phone can be a few megabytes.
const MAX_BODY_BYTES: usize = 12 * 1024 * 1024;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("APT_LOG").unwrap_or_else(|_| "info,sqlx=warn,html5ever=error".into()),
        )
        .init();

    let cfg = config::Config::from_env()?;

    // `amepricetrack compare <product link> [store]` looks the product up at
    // every other built-in store (or just the one named) and prints what it
    // finds, without touching the database. Set APT_AI_PROVIDER, APT_AI_KEY
    // and APT_AI_MODEL to try it with an AI service's help.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("compare") {
        let link = args.get(2).ok_or_else(|| anyhow::anyhow!("usage: amepricetrack compare <product link> [store]"))?;
        return compare_once(&cfg, link, args.get(3).map(String::as_str)).await;
    }
    // `amepricetrack nearby <zip>` lists the Target stores near a ZIP code.
    if args.get(1).map(String::as_str) == Some("nearby") {
        let zip =
            args.get(2).and_then(|z| stores::parse_zip(z)).ok_or_else(|| anyhow::anyhow!("usage: amepricetrack nearby <zip>"))?;
        let fetcher = fetch::Fetcher::new(&cfg)?;
        for store in fetcher.target_stores_near(&zip).await? {
            println!("Target {} | {} | {} | {:?} miles", store.id, store.name, store.place, store.miles);
        }
        return Ok(());
    }
    // `amepricetrack page <address> [file]` shows what a plain request to a store gets back
    // (and saves it to a file), for working out why a store cannot be read.
    if args.get(1).map(String::as_str) == Some("page") {
        let address = args.get(2).ok_or_else(|| anyhow::anyhow!("usage: amepricetrack page <address> [file to save to]"))?;
        let site = retailers::public_site(address)?;
        let html =
            fetch::Fetcher::new(&cfg)?.page(&retailers::site_name(&site), site.as_str(), retailers::FetchMode::Http).await?;
        println!("{} bytes", html.len());
        if let Some(file) = args.get(3) {
            std::fs::write(file, &html)?;
        }
        return Ok(());
    }
    // `amepricetrack parse <store> <file>` shows what the server reads from a saved product page.
    if args.get(1).map(String::as_str) == Some("parse") {
        let (Some(store), Some(file)) = (args.get(2), args.get(3)) else {
            anyhow::bail!("usage: amepricetrack parse <store> <saved page>");
        };
        let page = std::fs::read_to_string(file)?;
        println!("asks to sign in: {}", stores::shows_sign_in_prompt(&page));
        println!("{:#?}", retailers::parse(store, &page)?);
        return Ok(());
    }
    // `amepricetrack judge-test [file]` runs the pairs with known answers past the AI model
    // named by APT_AI_PROVIDER, APT_AI_KEY and APT_AI_MODEL, and reports how it did.
    if args.get(1).map(String::as_str) == Some("judge-test") {
        return judge_test(args.get(2).map(String::as_str)).await;
    }
    // `amepricetrack backup [folder]` writes a copy of the database now, into the backup
    // folder or the one named. It is safe to run while the server is running.
    if args.get(1).map(String::as_str) == Some("backup") {
        let dir = args.get(2).map(std::path::PathBuf::from).unwrap_or_else(|| cfg.backup_dir.clone());
        let db = db::open(&cfg.data_dir).await?;
        let written = backup::write(&db, &dir).await?;
        println!("{} ({} bytes)", written.path.display(), written.bytes);
        return Ok(());
    }
    // `amepricetrack search <store> <words...>` lists the product links a store's search returns.
    if args.get(1).map(String::as_str) == Some("search") {
        let store = args.get(2).ok_or_else(|| anyhow::anyhow!("usage: amepricetrack search <store> <words>"))?;
        return search_once(&cfg, store, &args[3..].join(" ")).await;
    }

    let db = db::open(&cfg.data_dir).await?;
    let fetcher = fetch::Fetcher::new(&cfg)?;
    let http = reqwest::Client::builder().user_agent("AmePriceTrack").build()?;
    let public_http = net::public_only(reqwest::Client::builder().user_agent("AmePriceTrack")).build()?;

    match &cfg.browser_path {
        Some(path) => tracing::info!("browser for protected stores: {}", path.display()),
        None => tracing::warn!(
            "no Chrome or Chromium found: Staples, Best Buy, Kohl's and Harbor Freight cannot be read by the server \
             (install google-chrome-stable and xvfb, or set APT_BROWSER)"
        ),
    }

    let app: App = Arc::new(AppState { db, cfg, fetcher, http, public_http, wake: Notify::new(), compare_wake: Notify::new() });
    stores::refresh_names(&app.db).await?;
    stores::apply_saved_location(&app).await?;
    stores::apply_browser_setups(&app).await.map_err(|e| anyhow::anyhow!("could not load the stores' browser settings: {e}"))?;
    tokio::spawn(scheduler::run(app.clone()));
    tokio::spawn(compare::tracking::run(app.clone()));

    let router = web::site(app.clone())
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(tower_http::compression::CompressionLayer::new());

    let listener = tokio::net::TcpListener::bind(app.cfg.bind)
        .await
        .map_err(|e| anyhow::anyhow!("could not listen on {}: {e}", app.cfg.bind))?;
    tracing::info!("AmePriceTrack is running at http://{}", app.cfg.bind);

    axum::serve(listener, router).with_graceful_shutdown(shutdown_signal()).await?;

    if let Some(browser) = app.fetcher.browser() {
        browser.shutdown().await;
    }
    Ok(())
}

async fn compare_once(cfg: &config::Config, link: &str, only: Option<&str>) -> anyhow::Result<()> {
    use compare::StoreResult;
    use pricing::dollars;

    let fetcher = fetch::Fetcher::new(cfg)?;
    let product = retailers::identify(link)?;
    println!("Reading {} ...", product.url);
    let source = fetcher.fetch(&product.retailer, &product.sku, &product.url, None).await?.obs;
    if let Some(store) = &source.local_store {
        println!("Price from local store number {} ({})", store.id, store.place);
    }
    let facts = compare::matching::Facts {
        details: source.details.clone(),
        ..compare::facts_of(
            source.title.as_deref(),
            source.brand.as_deref(),
            source.model.as_deref(),
            source.gtin.as_deref(),
            source.price_cents,
        )
    };
    let http = reqwest::Client::builder().user_agent("AmePriceTrack").build()?;
    let advisor = match ai_from_env() {
        Ok(ai) => {
            println!("With help from the AI model {}", ai.model);
            Some(compare::advisor::Advisor::new(&http, ai))
        }
        Err(_) => None,
    };
    let price = |cents: Option<i64>| cents.map(dollars).unwrap_or_else(|| "no price".into());
    println!(
        "{}: {} | {}\n  brand {:?}, model {:?}, barcode {:?}\n  searches: {:?}\n",
        retailers::display_name(&product.retailer),
        price(source.price_cents),
        facts.title,
        facts.brand,
        facts.model,
        facts.gtin,
        compare::search::queries(&facts),
    );
    if let Some(details) = &facts.details {
        println!("  details: {details}\n");
    }
    let wanted = |s: &&compare::search::StoreSearch| s.retailer != product.retailer && only.is_none_or(|o| o == s.retailer);
    for store in compare::search::builtin().iter().filter(wanted) {
        let name = &store.name;
        match compare::find_at_store(&fetcher, &facts, store, advisor.as_ref()).await {
            StoreResult::Found(found) => {
                println!(
                    "{name}: {} | {}{} ({}%)\n  {}\n  {}",
                    price(found.obs.price_cents),
                    if found.by_ai { "AI: " } else { "" },
                    found.reason,
                    found.score,
                    found.obs.title.as_deref().unwrap_or(&found.candidate.title),
                    found.candidate.url,
                );
                if let Some(pack) = &found.pack {
                    println!(
                        "  another amount: {} there, {} here",
                        compare::matching::amount_text(pack.other_qty, &pack.unit),
                        compare::matching::amount_text(pack.source_qty, &pack.unit)
                    );
                }
                if let Some(seller) = &found.obs.seller {
                    println!(
                        "  sold by {seller} (marketplace seller: {:?}), free delivery: {:?}",
                        found.obs.third_party, found.obs.ships_free
                    );
                }
            }
            StoreResult::NotFound(Some(note)) => println!("{name}: not found ({note})"),
            StoreResult::NotFound(None) => println!("{name}: not found"),
            StoreResult::Failed(why) => println!("{name}: could not search ({why})"),
        }
    }
    if let Some(browser) = fetcher.browser() {
        browser.shutdown().await;
    }
    Ok(())
}

/// The AI service described by the APT_AI_* environment variables, set up for matching.
fn ai_from_env() -> anyhow::Result<ai::AiConfig> {
    let env = |name: &str| std::env::var(name).unwrap_or_default();
    let settings = db::Settings {
        ai_provider: env("APT_AI_PROVIDER"),
        ai_api_key: env("APT_AI_KEY"),
        ai_model: env("APT_AI_MODEL"),
        ai_base_url: env("APT_AI_BASE_URL"),
        ..Default::default()
    };
    ai::AiConfig::for_matching(&settings)
}

async fn judge_test(file: Option<&str>) -> anyhow::Result<()> {
    use compare::cases::{self, Expect, Grade};

    let json = match file {
        Some(path) => std::fs::read_to_string(path)?,
        None => cases::BUILTIN.to_string(),
    };
    let all = cases::load(&json)?;
    let ai = ai_from_env().map_err(|e| anyhow::anyhow!("{e} (set APT_AI_PROVIDER, APT_AI_KEY and APT_AI_MODEL)"))?;
    println!("Asking {} about {} pairs with known answers.\n", ai.model, all.len());
    let http = reqwest::Client::builder().user_agent("AmePriceTrack").build()?;
    let advisor = compare::advisor::Advisor::new(&http, ai);

    let (mut right, mut wrong_same, mut missed, mut unsure) = (0, 0, 0, 0);
    for case in &all {
        let answer = cases::by_ai(&advisor, case).await;
        let grade = cases::grade(case, &answer);
        let mark = match grade {
            Grade::Right => {
                right += 1;
                "ok      "
            }
            Grade::WrongSame => {
                wrong_same += 1;
                "WRONG   "
            }
            Grade::Missed => {
                missed += 1;
                "missed  "
            }
            Grade::Unsure => {
                unsure += 1;
                "not sure"
            }
        };
        let expected = match (case.expect, case.pack) {
            (Expect::Same, _) => "same".to_string(),
            (Expect::Different, _) => "different".to_string(),
            (Expect::Pack, Some((tracked, listing))) => format!("same item, {tracked} against {listing}"),
            (Expect::Pack, None) => "same item, another amount".to_string(),
        };
        println!(
            "{mark}  {}\n          expected: {expected}; answered: {}; the rules alone: {}",
            case.name,
            answer.describe(),
            cases::by_rules(case).describe()
        );
    }
    println!(
        "\n{right} of {} right. {wrong_same} wrongly called the same (these would cause false alerts), \
         {missed} matches missed, {unsure} left undecided.",
        all.len()
    );
    if wrong_same > 0 {
        anyhow::bail!("{wrong_same} pairs were wrongly called the same");
    }
    Ok(())
}

async fn search_once(cfg: &config::Config, store: &str, query: &str) -> anyhow::Result<()> {
    let fetcher = fetch::Fetcher::new(cfg)?;
    let stores = compare::search::builtin();
    let search = stores.iter().find(|s| s.retailer == store).ok_or_else(|| anyhow::anyhow!("unknown store '{store}'"))?;
    let address = search.address(query);
    println!("{address}");
    let html = fetcher.page(&search.retailer, &address, search.mode.unwrap_or(retailers::FetchMode::Http)).await?;
    let links = compare::search::product_links(&search.retailer, &address, &html);
    println!("{} bytes, {} product links", html.len(), links.len());
    for link in links.iter().take(10) {
        println!("  {} | {} | {}", link.sku, link.title, link.url);
    }
    if let Some(browser) = fetcher.browser() {
        browser.shutdown().await;
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutting down");
}

/// An app with an empty in-memory database and no browser, for tests.
#[cfg(test)]
pub async fn test_app() -> App {
    let cfg = config::Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: std::env::temp_dir(),
        public_url: None,
        check_interval: std::time::Duration::from_secs(4 * 3600),
        browser_path: None,
        browser_port: 0,
        browser_args: vec![],
        target_store_id: "1375".into(),
        secure_cookies: false,
        // Tests that exercise backups name a folder of their own.
        backup_keep: 0,
        backup_dir: std::env::temp_dir().join("apt-test-backups"),
        members_local_network: false,
    };
    Arc::new(AppState {
        db: db::open_in_memory().await,
        fetcher: fetch::Fetcher::new(&cfg).unwrap(),
        cfg,
        http: reqwest::Client::new(),
        public_http: net::public_only(reqwest::Client::builder()).build().unwrap(),
        wake: Notify::new(),
        compare_wake: Notify::new(),
    })
}
