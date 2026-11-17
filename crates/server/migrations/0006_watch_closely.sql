-- How often this person wants the product checked, in minutes. NULL means
-- the server's usual pace. The product is checked as often as its most
-- impatient watcher asks.
ALTER TABLE watches ADD COLUMN check_minutes INTEGER;
