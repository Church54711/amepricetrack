<img src="assets/icon.svg" width="64" height="64" alt="">

# AmePriceTrack

A self-hosted price tracker. Paste a product link, and the server keeps
checking its price, records the history, and alerts you when it drops or
reaches your target. Prices can include your own discounts (for example a
Staples employee discount), and a built-in assistant answers questions about
what you track using whichever AI service you connect.

One Rust program serves the website and the phone API and does the
checking. Data lives in a single SQLite file.

## What it does

- **Tracks prices** at Staples, Best Buy, Walmart, Target, Amazon, Kohl's,
  Harbor Freight and Hot Topic, plus most other shops that publish standard
  product data. Any such shop can be added as a store of its own.
- **Alerts** on a target price, a drop of at least N percent, any drop, or
  back in stock. Alerts appear on the site and in the phone API, and can be
  pushed through [ntfy](https://ntfy.sh) or a webhook (Discord works as is).
- **Personal discounts.** A discount rule ("10% off everything at Staples,
  25% off these brands, except gift cards") is applied to the listed price.
  Targets and alerts use that personal price.
- **Signed-in prices.** A phone signed in at a store can send the page it
  sees to the server; when that price differs from the public one it is kept
  as that person's own price.
- **Deals page** ranking tracked items by how far below their usual price
  they are (90-day average, or the store's regular price while history is short).
- **Compares stores for you.** Paste one link and the same product is looked
  up at every other store, matched by barcode or model number, and kept up
  to date. You are told when another store is cheaper.
- **AI assistant** that sees your watchlist, and helps with matching: it
  picks the right search results and judges listings that no barcode
  settles. Works with Claude, OpenAI, DeepSeek, Gemini, OpenRouter, Ollama,
  LM Studio, or any OpenAI-compatible address.
- **Local prices.** With your ZIP code, Target prices come from the store
  nearest you.
- **Themes.** OLED Black & Gold by default, plus Midnight Blue, Light and
  "match my device", each with an optional custom accent colour.
- **Several people** can share one server; watchlists, discounts and
  settings are per person.

Not built yet: the Android app (the API it needs is in place).

## How each store is read

| Store | How the server reads it |
| --- | --- |
| Walmart, Amazon, Hot Topic | A plain web request. |
| Target | Target's own product data service. |
| Staples, Best Buy, Kohl's, Harbor Freight | A real Chrome window. These stores answer plain requests and headless browsers with "access denied", so the server runs an ordinary Chrome on a virtual screen (Xvfb) and reads the finished page. |
| Other shops, and stores you add | A plain request first, then Chrome if that is refused. |

**Best Buy on Linux is a known gap.** In testing (November 2026, Ubuntu 24.04
under WSL), Best Buy served Chrome on Windows normally but dropped every
connection from Chrome on Ubuntu (`ERR_HTTP2_PROTOCOL_ERROR`), whatever the
browser called itself. On a Linux server, expect Best Buy products to show
"Could not read price" until a device reports them through the phone API.
Staples, Kohl's, Harbor Freight and Walmart were read correctly on Ubuntu in
the same test; Target and Amazon use plain requests and were only run from
Windows.

Each product is checked about every four hours, with at least eight seconds
between two requests to the same store. A server at home works best; stores
are stricter with data-centre addresses. Automated reading is against most
stores' terms of use, so keep it to personal use at this gentle pace.

## Comparing stores

When you add a product, each other store is searched for it: first by
barcode, then by brand and model number, then by name. The likeliest results
are opened and judged:

- **Same product**: the barcode or the manufacturer's model number matches.
  These are trusted, tracked, and count towards "cheaper elsewhere" alerts.
- **Possibly the same**: only the name is similar, or the model matches but
  the colour, pack size or price does not. You confirm or reject these on the
  product page.
- **Not found**, or **could not search** with the reason.

With an AI service connected (Settings), it helps in two places. It reads
each store's search results and chooses which ones to open, which finds
products whose names are worded differently from store to store. And when no
barcode or model number settles a listing, it is shown the details of both
products (name, brand, model, price, colour, size, description) and answers
same, different or not sure. A match it makes is marked **Judged by AI** on
the product page and in the alert, until you confirm it yourself. A barcode
match never involves the AI.

This takes a handful of short requests per store, once per product, so a
small, inexpensive model is the right choice. Settings has a separate
"Model for matching products" so the assistant can keep a larger one; with
Claude, `claude-haiku-5-5` works well.

Your own discounts apply per store, so a Staples price after the employee
discount is compared with the other stores' prices fairly. Looking a product
up costs a few page loads per store, once; after that only the matches are
re-checked on the normal schedule. Turn automatic lookups off under Settings
if you would rather press "Compare across stores" per product.

To try a link without adding it:

```sh
amepricetrack compare 'https://www.walmart.com/ip/19864722287'          # every other store
amepricetrack compare 'https://www.walmart.com/ip/19864722287' bestbuy  # just one
```

Set `APT_AI_PROVIDER` (for example `claude`), `APT_AI_KEY` and `APT_AI_MODEL`
in the environment to try that command with the AI's help.

Known limits: Kohl's, Harbor Freight and Hot Topic mostly sell their own or
exclusive items, so they rarely match; without an AI service, Amazon and
Walmart marketplace listings of multi-packs can show up as "possibly the
same" at an odd price; a store with colour options on one page (Target) is
matched as a whole listing, not per colour; and the AI can be wrong, most
often when a product page says too little to tell near-identical variants
apart.

## Stores and your area

The Stores page lists every store the server knows and lets you add more.

**Adding a store.** Type the shop's web address. The server reads its front
page to learn its name and how its search works, and from then on looks the
things you track up there too. If it cannot work the search out, search the
shop for the word `pricetrack` and paste the address of the results page.
This works for shops that publish standard product data, which most do;
prices that need a sign-in are not read. Removing a store keeps the products
you track there.

**Your area.** The administrator can set a ZIP code. Target charges
different prices from store to store, so its prices are then read from the
store nearest that ZIP code. Only the ZIP code is stored, and it is sent
only to Target's store finder.

Walmart also prices per store, but it chooses the store itself from the
internet connection the request comes from, and (as tested in November 2026)
ignores a store named by a plain request. The Stores page shows which store
Walmart used for the server's readings. On a server at home that is normally
a store near you. If it is not your store, a phone can report the price your
own Walmart shows through `POST /api/v1/ingest`. The other built-in stores
show one online price everywhere.

## Install on Ubuntu

Needs Ubuntu 22.04 or newer and [Rust](https://rustup.rs).

```sh
sudo apt install build-essential
cargo build --release
sudo deploy/install.sh
```

The script installs Xvfb and Google Chrome, creates an `amepricetrack`
system user, installs the program as a service that starts on boot, and
prints the address to open. The first visitor creates the administrator
account. To update later: `git pull`, build again, run the script again.

- Settings: `/etc/amepricetrack.env` (see `deploy/amepricetrack.env.example`)
- Data: `/var/lib/amepricetrack` (back up `amepricetrack.db`)
- Log: `journalctl -u amepricetrack -f`

The site speaks plain HTTP on port 8787. To reach it from outside your home
network, put it behind a reverse proxy with HTTPS (Caddy, nginx) or a private
network such as Tailscale, and set `APT_SECURE_COOKIES=1` when using HTTPS.

## Run it for development

```sh
cargo run                 # http://localhost:8787, data in ./data
cargo test
```

Works on Windows and macOS too; there Chrome opens as a normal window when
a protected store is checked.

## Settings

All optional, set as environment variables.

| Variable | Default | Meaning |
| --- | --- | --- |
| `APT_BIND` | `0.0.0.0:8787` | Address and port to listen on. |
| `APT_DATA_DIR` | `./data` | Folder for the database and Chrome's profile. |
| `APT_PUBLIC_URL` | none | How people reach the server; used for links in alerts. |
| `APT_CHECK_INTERVAL_MINUTES` | `240` | Minutes between checks of each product. |
| `APT_BROWSER` | found automatically | Path to Chrome or Chromium, or `off`. |
| `APT_BROWSER_PORT` | `9377` | Local port for Chrome's control channel. |
| `APT_BROWSER_ARGS` | none | Extra Chrome arguments, space separated. |
| `APT_TARGET_STORE_ID` | `1375` | Target store whose prices are used until a ZIP code is set on the Stores page. |
| `APT_SECURE_COOKIES` | off | Set to `1` when served over HTTPS. |
| `APT_LOG` | `info,sqlx=warn` | Log detail. |
| `APT_DEBUG_PAGES` | off | Folder where the last unreadable page per store is saved. |

## Employee and member prices

Two ways to see what *you* would pay:

1. **Discount rules** (Discounts page). The Staples preset adds the associate
   discount: 10% off everything and 25% off Staples' own brands, skipping
   gift cards and postage. Staples applies this discount at checkout rather
   than on the product page and excludes some promotions, so it is a close
   estimate. No store password is involved.
2. **Signed-in prices** (`POST /api/v1/ingest`). A device that is signed in
   at a store sends the product page it loaded; the server reads the price
   from it. Store passwords and cookies never leave the device. This is what
   the phone app will use for member, employee and app-only prices.

## Phone API

JSON under `/api/v1`, with `Authorization: Bearer <token>` on every call
except `login`.

| Call | Purpose |
| --- | --- |
| `POST login` `{username, password, device_name}` | Returns a token. |
| `GET me` | Account, theme, unread alerts, the stores (built-in and added), and the server's location. |
| `GET watches` / `POST watches` `{url, target_price_cents?}` | List and add products. `url` may be the text a store app shares. |
| `GET products/{id}` | One product with price history. |
| `POST products/{id}/watch`, `DELETE products/{id}/watch` | Change alert settings, stop tracking. |
| `POST products/{id}/check` | Check now. |
| `POST products/{id}/compare` | Search the other stores for this product (again). Results arrive in `other_stores` of `GET products/{id}`; `judged_by` is `ai` for matches the AI made. |
| `POST comparisons/{id}` `{same}` | Confirm or reject a possible match. |
| `GET notifications?after={id}`, `POST notifications/read` | Alerts. |
| `GET collect` | Pages the server cannot read itself and wants the device to load. |
| `POST ingest` `{url, html, signed_in}` | A product page loaded on the device. |
| `POST chat` `{message}` | Ask the assistant. |

## Layout

```
crates/server/src
  main.rs        startup
  config.rs      environment settings
  db.rs          SQLite rows and queries       (migrations/ holds the schema)
  retailers/     store list, link recognition, page readers
  fetch.rs       plain requests, per-store pacing, choosing a reader
  compare/       finding the same product at other stores (search, matching, AI help, tracking)
  stores.rs      stores people add, and the server's location
  browser.rs     driving Chrome
  scheduler.rs   the background checking loop
  pricing.rs     discounts, personal price, averages
  alerts.rs      when to alert, ntfy and webhook delivery
  ai.rs          AI services
  service.rs     actions shared by the site and the API
  web.rs, ui.rs  the website            (static/ holds the CSS and script)
  api.rs         the phone API
deploy/          Ubuntu service file and install script
```
