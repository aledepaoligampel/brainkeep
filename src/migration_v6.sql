-- Archive and purge from the panel. An archived memory stays intact but leaves every
-- search, listing and recall until restored. A purge deletes the memory and keeps only
-- a content-free record. `project_changes` counts archive, restore and purge per
-- project so listing snapshots change even though no revision is added.
ALTER TABLE memories ADD COLUMN archived_at TEXT CHECK(archived_at IS NULL OR length(archived_at) BETWEEN 20 AND 32);
CREATE TABLE project_changes (
    project TEXT PRIMARY KEY,
    changes INTEGER NOT NULL CHECK(changes > 0)
) STRICT;
CREATE TABLE purges (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    purged_by TEXT NOT NULL,
    purged_at TEXT NOT NULL
) STRICT;
PRAGMA user_version=6;
