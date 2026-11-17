-- Looking a tracked product up at the other stores.
--
-- One row per person, tracked product and other store. It starts as
-- 'pending' (waiting to be searched) and ends as:
--   'found'     the same product was found (product_id points at it)
--   'possible'  a similar product was found; the person can confirm or reject it
--   'none'      the store was searched and has nothing like it
--   'failed'    the store could not be searched (reason says why)
--   'rejected'  the person said the suggested product is not the same
CREATE TABLE comparisons (
    id                  INTEGER PRIMARY KEY,
    user_id             INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source_product_id   INTEGER NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    retailer            TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'pending',
    product_id          INTEGER REFERENCES products(id) ON DELETE SET NULL,
    -- 0 to 100: how sure the match is.
    score               INTEGER,
    -- Why it matched ("Same barcode"), or why the search failed.
    reason              TEXT,
    -- The person confirmed a possible match themselves.
    confirmed           INTEGER NOT NULL DEFAULT 0,
    -- The other store's price at the time of the last "cheaper elsewhere"
    -- alert, so the same price is not announced twice.
    last_notified_cents INTEGER,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    UNIQUE (user_id, source_product_id, retailer)
);
CREATE INDEX comparisons_status ON comparisons (status, updated_at);
CREATE INDEX comparisons_product ON comparisons (product_id);

-- Look every newly added product up at the other stores automatically.
ALTER TABLE user_settings ADD COLUMN auto_compare INTEGER NOT NULL DEFAULT 1;
