-- Conventions for every migration:
--   * STRICT tables, INTEGER primary keys (document_chunks.id doubles as the sqlite-vec rowid).
--   * Timestamps are ISO-8601 UTC with milliseconds: strftime('%Y-%m-%dT%H:%M:%fZ', 'now').
--   * Editable entities carry `version`; an AFTER UPDATE trigger bumps it and `updated_at` unless
--     the statement already changed `version` (optimistic concurrency: SET version = version + 1 WHERE version = ?).

CREATE TABLE app_settings (
    key        TEXT PRIMARY KEY NOT NULL CHECK (length(key) > 0),
    value      TEXT NOT NULL CHECK (json_valid(value)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    version    INTEGER NOT NULL DEFAULT 1
) STRICT, WITHOUT ROWID;

CREATE TRIGGER app_settings_touch AFTER UPDATE ON app_settings
FOR EACH ROW WHEN NEW.version = OLD.version
BEGIN
    UPDATE app_settings SET version = OLD.version + 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE key = NEW.key;
END;
