PRAGMA application_id = 1396854085;
PRAGMA user_version = 1;
CREATE TABLE memories (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    current_revision INTEGER NOT NULL CHECK(current_revision > 0),
    created_at TEXT NOT NULL
) STRICT;
CREATE INDEX memories_project ON memories(project, created_at, id);
CREATE TABLE revisions (
    id TEXT NOT NULL REFERENCES memories(id),
    revision INTEGER NOT NULL CHECK(revision > 0),
    content TEXT NOT NULL,
    source TEXT NOT NULL,
    agent TEXT NOT NULL,
    confirmed INTEGER NOT NULL CHECK(confirmed IN (0,1)),
    saved_at TEXT NOT NULL,
    PRIMARY KEY(id, revision)
) STRICT;
CREATE TABLE operations (
    operation_id TEXT PRIMARY KEY,
    request TEXT NOT NULL,
    response TEXT NOT NULL,
    saved_at TEXT NOT NULL
) STRICT;
CREATE VIRTUAL TABLE memory_fts USING fts5(id UNINDEXED, project UNINDEXED, content, tokenize='unicode61 remove_diacritics 2');
