CREATE TABLE revision_metadata (
    id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    title TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('note','decision','preference','discovery','progress')),
    PRIMARY KEY(id, revision),
    FOREIGN KEY(id, revision) REFERENCES revisions(id, revision)
) STRICT;
INSERT INTO revision_metadata SELECT id, revision, '', 'note' FROM revisions;
CREATE TABLE topics (
    project TEXT NOT NULL,
    topic TEXT NOT NULL,
    id TEXT NOT NULL UNIQUE REFERENCES memories(id),
    PRIMARY KEY(project, topic)
) STRICT;
PRAGMA user_version=2;
-- A stable numeric FTS rowid avoids scanning the unindexed textual id on every write.
DELETE FROM memory_fts;
INSERT INTO memory_fts(rowid,id,project,content)
    SELECT m.rowid,m.id,m.project,r.content FROM memories m
    JOIN revisions r ON r.id=m.id AND r.revision=m.current_revision;
