-- All timestamps are unix seconds (UTC). All money is integer cents.

CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT NOT NULL,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL
);

-- Browser sessions (kind = 'web') and long-lived app/device tokens (kind = 'api').
-- Only a SHA-256 of the token is stored.
CREATE TABLE sessions (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash   TEXT NOT NULL UNIQUE,
    kind         TEXT NOT NULL,
    label        TEXT,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER,
    last_used_at INTEGER
);

-- One row per retailer item, shared by every user who watches it.
CREATE TABLE products (
    id                  INTEGER PRIMARY KEY,
    retailer            TEXT NOT NULL,
    retailer_sku        TEXT NOT NULL,
    url                 TEXT NOT NULL,
    title               TEXT,
    brand               TEXT,
    model               TEXT,
    gtin                TEXT,
    image_url           TEXT,
    current_price_cents INTEGER,
    list_price_cents    INTEGER,
    in_stock            INTEGER,
    promo_text          TEXT,
    lowest_price_cents  INTEGER,
    lowest_price_at     INTEGER,
    -- 'http' or 'browser': which reader last worked for this item.
    fetch_mode          TEXT,
    created_at          INTEGER NOT NULL,
    last_checked_at     INTEGER,
    last_success_at     INTEGER,
    next_check_at       INTEGER NOT NULL,
    fail_count          INTEGER NOT NULL DEFAULT 0,
    last_error          TEXT,
    UNIQUE (retailer, retailer_sku)
);
CREATE INDEX products_next_check ON products (next_check_at);
CREATE INDEX products_model ON products (model);
CREATE INDEX products_gtin ON products (gtin);

-- Price history. A row is written when the price or stock status changes.
-- user_id NULL  = the public price anyone sees.
-- user_id set   = a price seen while that user was signed in at the retailer
--                 (member, employee or app-only price), reported by their device.
CREATE TABLE price_points (
    id               INTEGER PRIMARY KEY,
    product_id       INTEGER NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    user_id          INTEGER REFERENCES users(id) ON DELETE CASCADE,
    ts               INTEGER NOT NULL,
    price_cents      INTEGER NOT NULL,
    list_price_cents INTEGER,
    in_stock         INTEGER,
    source           TEXT NOT NULL
);
CREATE INDEX price_points_product ON price_points (product_id, ts);

CREATE TABLE watches (
    id                     INTEGER PRIMARY KEY,
    user_id                INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    product_id             INTEGER NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    target_price_cents     INTEGER,
    drop_percent           REAL,
    notify_any_drop        INTEGER NOT NULL DEFAULT 1,
    notify_back_in_stock   INTEGER NOT NULL DEFAULT 0,
    note                   TEXT,
    created_at             INTEGER NOT NULL,
    -- What this user was last told about, so the same price is not announced twice.
    last_seen_price_cents  INTEGER,
    last_seen_in_stock     INTEGER,
    last_alert_price_cents INTEGER,
    last_alert_at          INTEGER,
    UNIQUE (user_id, product_id)
);
CREATE INDEX watches_product ON watches (product_id);

-- Personal discounts applied on top of the listed price, e.g. an employee discount.
CREATE TABLE discount_rules (
    id               INTEGER PRIMARY KEY,
    user_id          INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    retailer         TEXT NOT NULL,
    name             TEXT NOT NULL,
    percent          REAL NOT NULL,
    -- Comma separated. Empty = every brand.
    brands           TEXT NOT NULL DEFAULT '',
    -- Comma separated words; an item whose title contains one is not discounted.
    exclude_keywords TEXT NOT NULL DEFAULT '',
    enabled          INTEGER NOT NULL DEFAULT 1,
    created_at       INTEGER NOT NULL
);
CREATE INDEX discount_rules_user ON discount_rules (user_id, retailer);

CREATE TABLE notifications (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    product_id  INTEGER REFERENCES products(id) ON DELETE CASCADE,
    ts          INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    title       TEXT NOT NULL,
    body        TEXT NOT NULL,
    price_cents INTEGER,
    read_at     INTEGER
);
CREATE INDEX notifications_user ON notifications (user_id, id);

CREATE TABLE user_settings (
    user_id      INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    ntfy_url     TEXT NOT NULL DEFAULT '',
    webhook_url  TEXT NOT NULL DEFAULT '',
    ai_provider  TEXT NOT NULL DEFAULT '',
    ai_base_url  TEXT NOT NULL DEFAULT '',
    ai_api_key   TEXT NOT NULL DEFAULT '',
    ai_model     TEXT NOT NULL DEFAULT ''
);

CREATE TABLE chat_messages (
    id      INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    ts      INTEGER NOT NULL,
    role    TEXT NOT NULL,
    content TEXT NOT NULL
);
CREATE INDEX chat_messages_user ON chat_messages (user_id, id);
