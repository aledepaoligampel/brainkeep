-- Personal tokens issued from the panel: one per user, stored only as SHA-256 hex.
-- They authenticate /mcp and panel sign-in without auth.json or Google. Disabling the
-- user deletes the row, so re-enabling never revives an old token. `used` records
-- whether the token was ever presented (shown as "signed in" to the admin).
CREATE TABLE personal_tokens (
    user_id INTEGER PRIMARY KEY REFERENCES users(id),
    hash TEXT NOT NULL UNIQUE CHECK(length(hash) = 64),
    created_at TEXT NOT NULL,
    used INTEGER NOT NULL CHECK(used IN (0,1))
) STRICT;
PRAGMA user_version=7;
