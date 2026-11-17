-- AI help with matching, added stores, and where the server's people shop.

-- The AI model used for matching products across stores. Empty means the
-- same model as the assistant.
ALTER TABLE user_settings ADD COLUMN ai_match_model TEXT NOT NULL DEFAULT '';

-- Who decided a match: 'rules' (barcode, model number or name) or 'ai'.
-- A match the person confirmed themselves has confirmed = 1 as before.
ALTER TABLE comparisons ADD COLUMN judged_by TEXT NOT NULL DEFAULT 'rules';

-- Colour, size, pack count and a line of description from the product page,
-- as plain text. Helps tell two similar listings apart.
ALTER TABLE products ADD COLUMN details TEXT;

-- Stores added by the people using this server, on top of the built-in ones.
CREATE TABLE stores (
    id          INTEGER PRIMARY KEY,
    -- The site name, e.g. "example.com". Products from it use this as their retailer.
    retailer    TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    -- The store's search results address, with {q} where the search words go.
    search_url  TEXT NOT NULL,
    added_by    INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at  INTEGER NOT NULL
);

-- Settings for the whole server (location, the local store per chain).
CREATE TABLE server_settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
