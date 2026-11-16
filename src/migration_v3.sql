-- Server-authenticated author of each revision; NULL for revisions saved before v3.
ALTER TABLE revisions ADD COLUMN principal TEXT CHECK(principal IS NULL OR length(principal) BETWEEN 1 AND 128);
PRAGMA user_version=3;
