-- Per-person appearance: a theme id (see ui::THEMES) and an optional custom
-- accent colour as #rrggbb ('' = the theme's own accent).
ALTER TABLE user_settings ADD COLUMN theme TEXT NOT NULL DEFAULT 'oled-gold';
ALTER TABLE user_settings ADD COLUMN accent TEXT NOT NULL DEFAULT '';
