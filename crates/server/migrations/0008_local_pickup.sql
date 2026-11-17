-- Whether the chain's local store had the item to collect at the server's
-- last reading: 1 yes, 0 no, NULL when the store does not say.
ALTER TABLE products ADD COLUMN pickup INTEGER;
