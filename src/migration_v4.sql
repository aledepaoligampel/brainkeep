-- Users and project ownership. Project keys become `<owner handle>/<name>`;
-- the data step that prefixes existing projects runs in Rust with the owner handle.
CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    handle TEXT NOT NULL UNIQUE CHECK(length(handle) BETWEEN 1 AND 32 AND handle GLOB '[a-z0-9]*' AND handle NOT GLOB '*[^a-z0-9._-]*' AND handle != 'local'),
    email TEXT UNIQUE CHECK(email IS NULL OR (length(email) BETWEEN 3 AND 254 AND email = lower(email))),
    google_sub TEXT UNIQUE CHECK(google_sub IS NULL OR length(google_sub) BETWEEN 1 AND 255),
    admin INTEGER NOT NULL CHECK(admin IN (0,1)),
    disabled INTEGER NOT NULL CHECK(disabled IN (0,1)),
    created_at TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX users_single_admin ON users(admin) WHERE admin = 1;
CREATE TABLE project_members (
    project TEXT NOT NULL CHECK(length(project) BETWEEN 3 AND 161),
    user_id INTEGER NOT NULL REFERENCES users(id),
    role TEXT NOT NULL CHECK(role IN ('read','write')),
    PRIMARY KEY(project, user_id)
) STRICT;
CREATE INDEX project_members_user ON project_members(user_id, project);
-- OAuth clients registered dynamically (RFC 7591); public clients with PKCE only.
CREATE TABLE oauth_clients (
    client_id TEXT PRIMARY KEY CHECK(length(client_id) BETWEEN 16 AND 64),
    redirect_uris TEXT NOT NULL CHECK(json_valid(redirect_uris) AND length(redirect_uris) <= 4096),
    client_name TEXT NOT NULL CHECK(length(client_name) <= 128),
    created_at TEXT NOT NULL
) STRICT;
-- Issued tokens, stored only as SHA-256 hex. Refresh tokens rotate within a family;
-- reusing a rotated refresh token revokes the whole family.
CREATE TABLE oauth_tokens (
    hash TEXT PRIMARY KEY CHECK(length(hash) = 64),
    kind TEXT NOT NULL CHECK(kind IN ('access','refresh')),
    user_id INTEGER NOT NULL REFERENCES users(id),
    client_id TEXT NOT NULL REFERENCES oauth_clients(client_id),
    family TEXT NOT NULL CHECK(length(family) = 64),
    expires_at INTEGER NOT NULL,
    used INTEGER NOT NULL CHECK(used IN (0,1))
) STRICT;
CREATE INDEX oauth_tokens_family ON oauth_tokens(family);
CREATE INDEX oauth_tokens_expiry ON oauth_tokens(expires_at);
PRAGMA user_version=4;
