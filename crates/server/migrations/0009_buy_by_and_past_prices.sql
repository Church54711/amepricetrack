-- A day the person wants to have bought the product by (noon UTC of that
-- day). Checks and alerts get more eager as it comes closer.
ALTER TABLE watches ADD COLUMN buy_by INTEGER;
-- Which reminder about that day was last sent: 0 none, 1 a few days ahead,
-- 2 the day before, 3 on the day.
ALTER TABLE watches ADD COLUMN buy_by_told INTEGER NOT NULL DEFAULT 0;

-- The person's own Keepa key, for fetching an Amazon product's past prices.
ALTER TABLE user_settings ADD COLUMN keepa_key TEXT NOT NULL DEFAULT '';
