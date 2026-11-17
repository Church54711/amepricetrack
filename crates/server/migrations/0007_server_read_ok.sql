-- How the server's own last attempt to read the product went: 1 it read it,
-- 0 it could not, NULL it has not tried. Readings from phones do not change
-- this. A phone's reading is taken as the shared price only while the server
-- is not reading the product itself, and this is how that is known.
ALTER TABLE products ADD COLUMN server_read_ok INTEGER;

-- Products already being tracked: one the server has read before and whose
-- last check did not fail counts as read by the server, so a phone cannot
-- replace its shared price in the hours before the server's next check.
UPDATE products SET server_read_ok = CASE
    WHEN fail_count > 0 THEN 0
    WHEN fetch_mode IS NOT NULL THEN 1
END;
