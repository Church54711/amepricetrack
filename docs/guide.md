# AmePriceTrack guide

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
- **Find by name or barcode.** Type a product's name or the number under its
  barcode instead of a link (the phone app can scan the barcode), and pick
  the right result from each store's own search.
- **Compares stores for you.** Paste one link and the same product is looked
  up at every other store, matched by barcode or model number, and kept up
  to date. You are told when another store is cheaper.
- **Compares what it really costs.** Delivery charges count, marketplace
  sellers on Amazon and Walmart are named, and the same item in another pack
  size (a ream against a carton, a single against a 3-pack) is compared for
  the same amount.
- **Price match card.** When another store is cheaper, one page shows both
  listings side by side with what to check, ready to show at the register.
- **Tells you when a store stops working.** If nothing at a store could be
  read for two days you get an alert, and another when it reads again.
- **Watches closely when you ask.** A product you are waiting on can be
  checked every 30 minutes, hour or two hours instead of the usual four.
- **Backs itself up.** A copy of the database is written once a day and the
  newest 14 are kept.
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

There is a companion Android app in `android/` (see "Android app" below).

## How each store is read

| Store | How the server reads it |
| --- | --- |
| Walmart, Amazon, Hot Topic | A plain web request. |
| Target | Target's own product data service. |
| Staples, Best Buy, Kohl's, Harbor Freight | A real Chrome window. These stores answer plain requests and headless browsers with "access denied", so the server runs an ordinary Chrome on a virtual screen (Xvfb) and reads the finished page. |
| Other shops, and stores you add | A plain request first, then Chrome if that is refused. |

**Best Buy on Linux comes and goes.** Tested on Ubuntu 24.04 under WSL in
November 2026: on the 16th, after a day of heavy testing from the same
connection, Best Buy dropped every connection from Chrome on Ubuntu
(`ERR_HTTP2_PROTOCOL_ERROR`) while serving Chrome on Windows normally; on
the 17th the same Ubuntu setup read Best Buy product pages and its search
without trouble. So a refusal there looks like a temporary block of a busy
address, not a rule against Linux. If it happens, the Stores page shows it
and you get the "store stopped working" alert after two days; it has cleared
by itself so far. Staples, Kohl's, Harbor Freight and Walmart were read
correctly on Ubuntu in both tests. (Best Buy also has an official product
data service, but its terms forbid keeping what it returns for more than 72
hours, which a price history does, so the server does not use it.)

Each product is checked about every four hours, with at least eight seconds
between two requests to the same store. One you are waiting on can be
checked more often: on its page, under "Check the price", choose every 30
minutes, hour or two hours. The same product at the other stores is then
checked as often. Stores notice frequent visits, so keep this to a few
products. A server at home works best; stores
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

### What is compared

Two prices are only worth comparing when they buy the same thing, delivered.

- **Delivery.** Amazon and Walmart pages say whether delivery is free or what
  it costs. A stated charge is added to that store's price. When a page only
  says delivery is charged, the listing is marked "plus delivery" rather
  than guessed at. If a membership gives you free delivery (Prime,
  Walmart+), add it on the Discounts page with "Free delivery" ticked; it
  covers what the store ships itself. Delivery is not read at the other stores.
- **Marketplace sellers.** Amazon and Walmart also list other sellers'
  offers. The seller is shown on the product page and named in the alert.
  Settings has "Leave marketplace sellers out" for people who never want
  those offers to count as the best price.
- **Pack sizes.** When two listings are the same item in different amounts
  ("500 Sheets" against "5000 Sheets", "8 Count" against "24 Count"), the
  other store's price is worked out for the amount you track, and the page
  shows both figures. Only counts are treated this way (sheets, reams,
  packs, counts, rolls, pairs). A different size of the item itself, such as
  a 12 oz bottle against a 24 oz one, stays a different product.

### Price match card

On a product page, a store that is cheaper for the identical item has a
"Price match card" link. The card shows your store's listing and the other
store's side by side (name, price, model, barcode, item number), when the
other price was read, a button to open the live listing, and anything that
may get a match refused: a marketplace seller, an out-of-stock item, a
delivery charge, or a match only your AI vouches for. "Check both prices
again" re-reads both pages. Price-match rules differ by store and change;
the card does not know them.

### Checking the AI's matching

The server ships with a set of listing pairs whose answers are known: the
same product worded two ways, near-identical variants, other colours and
generations, bundles, refurbished items, and other pack sizes.

```sh
APT_AI_PROVIDER=claude APT_AI_KEY=... APT_AI_MODEL=claude-haiku-5-5 amepricetrack judge-test
```

It reports how many the model got right, how many it wrongly called the same
(the mistake that causes false alerts), how many matches it missed and how
many it left undecided, and exits with an error if any were wrongly called
the same. Pass a file of your own pairs as an argument; the format is in
`crates/server/tests/matching_cases.json`. The same pairs are run against
the rules alone in the test suite.

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
exclusive items, so they rarely match; a listing that does not state its
pack size in its name is taken to hold the same amount unless an AI service
says otherwise; Walmart does not state the delivery charge on orders under
its free-delivery minimum, only that there is one; a store with colour options on one page (Target) is
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
a store near you. If it is not your store, choose your store in the server's
own browser (below). The other built-in stores show one online price everywhere.

**The server's browser.** The server reads several stores with a real Chrome
of its own, and that browser keeps its cookies. The administrator can use it
by hand from the Stores page ("Open in the server's browser"): the page shows
a picture of the browser, passes on clicks and typing, and has a
press-and-hold for "prove you are human" checks. Use it to:

- sign in at a store, so the server reads the prices your account is shown;
- choose your local store at a chain that prices per store;
- get a store past a robot check it has put the server behind.

When you are done, tick what you did. "I am signed in" is shown on the
Stores page, because those prices are then that account's for everyone on
the server. "Read with this browser from now on" matters for stores that are
normally read with plain requests (Walmart, Target, Amazon, Hot Topic):
plain requests carry neither a sign-in nor a chosen store, so the server
switches to the browser for that store and falls back to plain requests if
the browser is refused. Price checks that need the browser wait while
someone is using it, and it is released 40 seconds after the page is closed.
What you type goes from your web browser to your server, so do this at home
or over HTTPS. This is the place to do sign-ins: the server is always on,
and a phone then has nothing to report.

Store sign-ins lapse. The server watches for that: it knows whether a
store's pages ask a visitor to sign in, and when two pages in a row ask
again after you signed in, the person who signed in gets an alert and the
Stores page says "Looks signed out". Until someone signs in again the
ordinary prices are recorded. This works where a signed-out page says "Sign
in" and a signed-in one does not; where the server cannot see a difference
(Harbor Freight's pages did not ask in testing), the Stores page says so
instead of pretending. It has been tested with made-up pages and on real
signed-out pages, not yet with a real account.

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
- Data: `/var/lib/amepricetrack`
- Backups: `/var/lib/amepricetrack/backups` (see "Backups" below)
- Log: `journalctl -u amepricetrack -f`

The site speaks plain HTTP on port 8787. To reach it from outside your home
network, put it behind a reverse proxy with HTTPS (Caddy, nginx) or a private
network such as Tailscale, and set `APT_SECURE_COOKIES=1` when using HTTPS.
Behind a proxy, also set `APT_PUBLIC_URL` to the address you open the site
with: forms are only accepted from the site's own address, and a proxy that
does not pass that address on would otherwise have every form refused.

**Who can reach what.** The server fetches addresses people give it. Store
addresses must be public websites; a name that leads to a machine on your
home network is refused. An AI service, ntfy topic or webhook may be on the
home network (an Ollama or ntfy of your own), but only for the
administrator, unless `APT_MEMBERS_LOCAL_NETWORK=1` extends that to
everyone with an account. Where the server reads a product itself and someone else also
tracks it, a different price reported by a phone stays that person's own,
unless the administrator's phone sent it. Where the server cannot read the
product, the phone's reading is all there is and is shared. Store pages are
fetched directly, without any proxy set in the environment.

## Backups

Everything (accounts, watchlists, price history, settings, keys) is in one
file, `amepricetrack.db` in the data folder. Once a day the server writes a
complete copy of it to the `backups` folder next to it, named by date
(`amepricetrack-2026-11-17.db`), and keeps the newest 14. If a backup fails,
the administrator gets an alert. Settings (as the administrator) shows when
the last one was made and has a "Back up now" button;
`amepricetrack backup [folder]` does the same from a terminal and is safe
while the server runs.

The copies are on the same disk as the data, which protects against a
mistake or a damaged database, not against losing the disk. Point
`APT_BACKUP_DIR` at another disk or a network folder for that. A backup
holds the same sign-ins and keys as the database, so keep it as private.

To go back to a backup: stop the server, replace `amepricetrack.db` with the
copy (and delete `amepricetrack.db-wal` and `amepricetrack.db-shm` if they
are there), and start the server again.

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
| `APT_BACKUP_KEEP` | `14` | How many daily copies of the database to keep; `0` turns backups off. |
| `APT_BACKUP_DIR` | `backups` in the data folder | Where those copies are written. |
| `APT_MEMBERS_LOCAL_NETWORK` | off | Set to `1` to let people who are not the administrator point their AI service, ntfy topic or webhook at an address on the home network. |
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

## Android app

A native Kotlin app in `android/`. It signs in to your server with the same
username and password as the website. It looks like the website by default;
each phone can instead be given a theme of its own in the app's Settings.

- **Watchlist and product pages**: prices, deal labels, price history, the
  other stores' prices, and alert settings.
- **Add by sharing**: in a store's app or a browser, choose Share, then
  "Track price". Or paste a link with the + button.
- **Find by name or barcode**: type a name with the + button, or scan a
  product's barcode, and pick the right result from each store. The scanner
  is the one built into Google Play services, so the app asks for no camera
  permission; without Play services, type the number under the barcode.
- **Price match card**: the same card as on the website, sized for showing
  at a register.
- **Notifications**: the app asks the server for new alerts every half hour
  unless you choose otherwise.
- **Instant alerts**: Settings can set up pushed alerts instead. It makes a
  long random [ntfy](https://ntfy.sh) topic, has your server push to it,
  opens the ntfy app subscribed to that topic, and sends a test. After that
  the app's own checking can be turned off: ntfy delivers the alert, and its
  "Open in app" button shows the product in AmePriceTrack. Alerts then pass
  through the ntfy server in the address (ntfy.sh unless you run your own),
  readable by anyone who knows the topic name. The Google Play build of ntfy
  gets ntfy.sh messages through Google's push service; the F-Droid build, and
  any ntfy server of your own, keeps a connection open instead.
- **Watching closely**: a product's alert settings include how often the
  server checks it.
- **Prices from this phone**: the app opens product pages in its own
  browser and sends them to the server. Open a store there once to sign in
  or pick your local store, and its prices are kept as your own. This is how
  member and employee prices, your own Walmart store, and stores the server
  cannot read get in. Store passwords and sign-ins stay on the phone.
  This is a fallback: the server does the reading, and signing in is better
  done in the server's own browser (see "Stores and your area"). The phone
  can also report by itself, which is **off unless you turn it on**. When
  on, you choose daily or twice a day, only on Wi-Fi (the default), only
  while charging, and which stores take part. A run starts with one small
  request; reads only products the server cannot read or stores you opened
  on the phone; skips anything read recently; loads pages without pictures,
  fonts or video; sends them compressed; gives up on a store after two
  failures; never runs on a low battery; and does not retry when the server
  is out of reach.
- **Battery**: alerts are checked every half hour by default (15 minutes,
  hourly or off are the other choices), one small request each time. For
  instant alerts with no checking at all, use "Instant alerts" above.
- **Assistant**: ask the connected AI about your tracked prices.

Build it with Android Studio, or from a terminal with a JDK (17 or newer)
and the Android SDK installed:

```sh
cd android
./gradlew assembleDebug        # app/build/outputs/apk/debug/app-debug.apk
./gradlew testDebugUnitTest
```

The phone has to be able to reach the server, for example on the same Wi-Fi.
The app allows plain HTTP because a home server usually has no certificate;
use HTTPS if you expose the server beyond your home network.

## Phone API

JSON under `/api/v1`, with `Authorization: Bearer <token>` on every call
except `login`.

| Call | Purpose |
| --- | --- |
| `POST login` `{username, password, device_name}` | Returns a token. |
| `GET me` | Account, theme, unread alerts, the stores (built-in and added), the server's location, and `check_interval_minutes`. |
| `GET watches` / `POST watches` `{url, target_price_cents?}` | List and add products. `url` may be the text a store app shares. |
| `GET products/{id}` | One product with price history. |
| `POST products/{id}/watch`, `DELETE products/{id}/watch` | Change alert settings (`check_minutes`, 15 to 1440, has the product checked that often; leave it out for the usual pace), stop tracking. |
| `POST products/{id}/check` | Check now. |
| `POST products/{id}/compare` | Search the other stores for this product (again). Results arrive in `other_stores` of `GET products/{id}`; `judged_by` is `ai` for matches the AI made. |
| `POST comparisons/{id}` `{same}` | Confirm or reject a possible match. |
| `POST comparisons/{id}/refresh` | Read both sides of a comparison again now. |
| `GET find?q=` | Start a find by name or barcode: the tidied query, the stores to ask, and matches already tracked. |
| `GET find?q=&store=` | One store's first results (`title`, `url`); pass a `url` to `POST watches` to track it. |
| `GET notifications?after={id}`, `POST notifications/read` | Alerts. |
| `GET collect` | Pages the server cannot read itself and wants the device to load. |
| `POST ingest` `{url, html, signed_in}` | A product page loaded on the device. The body may be sent with `Content-Encoding: gzip`. |
| `POST chat` `{message}` | Ask the assistant. |
| `GET push`, `POST push` `{ntfy_url}` | Where alerts are pushed; set or (with an empty address) clear the ntfy topic. |
| `POST push/test` | Push a test alert now. |

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
  find.rs        finding a product by name or barcode
  health.rs      noticing when a whole store stops being readable
  net.rs         keeping requests people ask for off the home network
  backup.rs      daily copies of the database
  browser.rs     driving Chrome
  scheduler.rs   the background checking loop
  pricing.rs     discounts, delivery, personal price, averages
  alerts.rs      when to alert, ntfy and webhook delivery
  ai.rs          AI services
  service.rs     actions shared by the site and the API
  web.rs, ui.rs  the website            (static/ holds the CSS and script)
  api.rs         the phone API
deploy/          Ubuntu service file and install script
android/         the Android app (Kotlin, Jetpack Compose)
```
