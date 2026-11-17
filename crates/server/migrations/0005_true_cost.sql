-- Comparing what things really cost: who sells it, what delivery costs,
-- and how many come in one purchase.

-- Who sells the item, at stores that host other sellers (Amazon, Walmart).
ALTER TABLE products ADD COLUMN seller TEXT;
-- 1 when that seller is not the store itself. NULL when the page does not say.
ALTER TABLE products ADD COLUMN third_party INTEGER;
-- 1 when the store itself ships the item, whoever sells it. NULL when unknown.
ALTER TABLE products ADD COLUMN store_ships INTEGER;
-- 1 when the page says delivery is free, 0 when it is charged. NULL when unknown.
ALTER TABLE products ADD COLUMN ships_free INTEGER;
-- The delivery charge, when the page states one.
ALTER TABLE products ADD COLUMN shipping_cents INTEGER;

-- A membership that makes delivery free at a store (Prime, Walmart+).
-- Kept with the discounts because both are "my own terms at this store".
ALTER TABLE discount_rules ADD COLUMN free_shipping INTEGER NOT NULL DEFAULT 0;

-- Leave offers from marketplace sellers out when comparing stores.
ALTER TABLE user_settings ADD COLUMN ignore_marketplace INTEGER NOT NULL DEFAULT 0;

-- The same item sold in a different quantity (a 3-pack against a single, a
-- ream against a carton). `source_qty` is how many the tracked product
-- holds, `other_qty` how many the other store's listing holds, and
-- `qty_unit` what is being counted ("pack", "sheets"). All NULL when both
-- listings hold the same amount.
ALTER TABLE comparisons ADD COLUMN source_qty REAL;
ALTER TABLE comparisons ADD COLUMN other_qty REAL;
ALTER TABLE comparisons ADD COLUMN qty_unit TEXT;
