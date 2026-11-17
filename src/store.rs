use crate::{
    access::{self, Access},
    files, security,
};
use fs2::FileExt;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, String>;
const DATABASE: &str = "memory.sqlite";
const APP_ID: i64 = 1396854085;
const DB_ERROR: &str = "storage failure; operation not confirmed, retry with the same operation_id";
const INVALID: &str = "invalid arguments; see tool schema";
pub(crate) const SELECT: &str = "SELECT m.id,m.project,r.revision,r.content,r.source,r.agent,r.saved_at,CASE WHEN r.revision!=m.current_revision THEN 'replaced' WHEN r.confirmed=1 THEN 'confirmed' ELSE 'proposed' END  ,a.title,a.kind,t.topic,r.principal FROM memories m JOIN revisions r ON r.id=m.id JOIN revision_metadata a ON a.id=r.id AND a.revision=r.revision LEFT JOIN topics t ON t.id=m.id";

/// Internal project key: `<owner handle>/<name>`. Callers use names up to 128 bytes.
const PROJECT_KEY_MAX: usize = 161;
/// Owner of a fresh installation when no handle is given.
const DEFAULT_OWNER: &str = "owner";
/// Newest schema this build reads and writes.
const SCHEMA_VERSION: i64 = 7;
/// Prefix of personal tokens issued from the panel (`sbp_` + 64 hex).
pub const PERSONAL_PREFIX: &str = "sbp_";
/// Panel administration answers for sessions that are not the admin: like a missing view.
const ADMIN_VIEWS: [&str; 6] = ["users", "invite", "disable", "enable", "token", "delete"];
/// Principal prefix of panel sessions (`panel:<session hash>`); `:` is invalid in
/// credential names and OAuth principals start with `oauth-`.
pub const PANEL_PRINCIPAL: &str = "panel:";
/// `correct` against a revision that is no longer current (or a missing memory).
pub const REVISION_CONFLICT: &str =
    "revision conflict or memory not found; recall before correcting";
/// `remember` with the topic of an archived memory: only the panel can bring it back.
/// Request kept in the receipts of a purged memory: no content, refuses retries.
const PURGED_RECEIPT: &str = r#"{"purged":true}"#;
/// Purge of a memory that is still active: it must be archived first.
pub const MUST_ARCHIVE: &str = "archive the memory before purging it";
pub const TOPIC_ARCHIVED: &str =
    "topic belongs to an archived memory; ask the user to restore it in the panel";
/// Fields the panel may change through `panel.correct`; the rest is fixed by the panel.
const PANEL_EDIT_FIELDS: [&str; 9] = [
    "project",
    "id",
    "expected_revision",
    "operation_id",
    "content",
    "title",
    "kind",
    "topic",
    "confirmed",
];
/// Pseudo OAuth client owning panel sessions, so disabling a user deletes them with
/// the rest of their tokens and re-inviting never revives them. It has no redirect
/// URIs, so it can never complete an authorization, and its rows never act as bearers.
const PANEL_CLIENT: &str = "brainkeep-panel-sessions";
const PANEL_TTL: i64 = 30 * 86400;
pub const TOKEN_TOO_SOON: &str = "token was just issued; try again in a minute";
pub const ADMIN_NO_PERSONAL: &str = "the admin has no personal token";
pub const USER_HAS_DATA: &str = "user has memories or edits; disable instead";
/// SQL: whether user `u` left anything behind: rows under their project keys (memories,
/// members of their shared projects, archive counters, purge records), revisions they wrote
/// or purges they made. Any of these would be orphaned, or inherited by a re-invited handle.
const HAS_DATA: &str = "(EXISTS(SELECT 1 FROM memories m WHERE substr(m.project,1,length(u.handle)+1)=u.handle||'/') \
     OR EXISTS(SELECT 1 FROM project_members pm WHERE substr(pm.project,1,length(u.handle)+1)=u.handle||'/') \
     OR EXISTS(SELECT 1 FROM project_changes c WHERE substr(c.project,1,length(u.handle)+1)=u.handle||'/') \
     OR EXISTS(SELECT 1 FROM purges g WHERE substr(g.project,1,length(u.handle)+1)=u.handle||'/' OR g.purged_by=u.handle) \
     OR EXISTS(SELECT 1 FROM revisions r WHERE r.principal=u.handle))";
pub const TOKEN_SESSION: &str = "sign in with Google to issue a new token";
/// A signed-in user may replace their own personal token at most this often.
const SELF_ISSUE_INTERVAL: Duration = Duration::from_secs(60);

pub struct Store {
    db: Connection,
    _lock: File,
    owner: String,
    /// Changes since the last `take_changes` (live panel): (internal key, membership).
    /// Membership changes alter who sees what; `*` means any project may be affected.
    changed: Vec<(String, bool)>,
    /// Last self-issued personal token per handle (process lifetime; see `my_token`).
    self_issued: std::collections::HashMap<String, std::time::Instant>,
}
impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        Self::open_with_owner(dir, None)
    }
    /// Opens the store. `owner` names the admin user when migrating data to v4;
    /// a migrated database rejects a different owner instead of renaming it.
    pub fn open_with_owner(dir: &Path, owner: Option<&str>) -> Result<Self> {
        if owner.is_some_and(|h| !access::valid_handle(h)) {
            return Err("invalid owner handle: 1-32 of [a-z0-9._-], starting alphanumeric".into());
        }
        files::private_dir(dir)?;
        let lock = files::private_file(&dir.join("store.lock"))?;
        lock.try_lock_exclusive()
            .map_err(|_| "Brainkeep is already running for this data directory")?;
        let path = dir.join(DATABASE);
        let fresh = !path.try_exists().map_err(|_| DB_ERROR)?;
        let _file = files::private_file(&path)?;
        for suffix in ["-wal", "-shm"] {
            let p = dir.join(format!("{DATABASE}{suffix}"));
            if fs::symlink_metadata(&p).is_ok() {
                files::check_file(&p)?;
            }
        }
        let mut db = Connection::open(&path).map_err(|_| DB_ERROR)?;
        db.busy_timeout(Duration::from_secs(3))
            .map_err(|_| DB_ERROR)?;
        db.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA temp_store=FILE")
            .map_err(|_| DB_ERROR)?;
        // Reject foreign/unsupported databases before changing their journal mode.
        if !fresh {
            validate_database(&db)?;
        }
        db.execute_batch(
            // secure_delete zeroes deleted content, so a purge leaves nothing in free pages.
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON;",
        )
        .map_err(|_| DB_ERROR)?;
        if fresh {
            db.execute_batch(&format!(
                "BEGIN IMMEDIATE; {} COMMIT;",
                include_str!("schema.sql")
            ))
            .map_err(|_| DB_ERROR)?;
        }
        for (from, migration) in [
            (1, include_str!("migration_v2.sql")),
            (2, include_str!("migration_v3.sql")),
            (3, include_str!("migration_v4.sql")),
            (4, include_str!("migration_v5.sql")),
            (5, include_str!("migration_v6.sql")),
            (6, include_str!("migration_v7.sql")),
        ] {
            if schema_version(&db)? != from {
                continue;
            }
            // Existing memories must be assigned to an explicitly named owner.
            let handle = match (from, owner) {
                (3, Some(h)) => h,
                (3, None) => {
                    let used: i64 = db
                        .query_row("SELECT count(*) FROM memories", [], |r| r.get(0))
                        .map_err(|_| DB_ERROR)?;
                    if used != 0 {
                        return Err(
                            "schema v4 migration requires --owner <handle> for existing memories"
                                .into(),
                        );
                    }
                    DEFAULT_OWNER
                }
                _ => "",
            };
            if !fresh {
                snapshot(&db, dir, from + 1)?;
            }
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| DB_ERROR)?;
            tx.execute_batch(migration).map_err(|_| DB_ERROR)?;
            if from == 3 {
                tx.execute(
                    "INSERT INTO users(handle,admin,disabled,created_at) VALUES(?1,1,0,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
                    [handle],
                )
                .map_err(|_| DB_ERROR)?;
                for table in ["memories", "topics", "memory_fts"] {
                    tx.execute(
                        &format!("UPDATE {table} SET project=?1||'/'||project"),
                        [handle],
                    )
                    .map_err(|_| DB_ERROR)?;
                }
            }
            validate_database(&tx)?;
            tx.commit().map_err(|_| DB_ERROR)?;
        }
        validate_database(&db)?;
        let stored: String = db
            .query_row("SELECT handle FROM users WHERE admin=1", [], |r| r.get(0))
            .map_err(|_| DB_ERROR)?;
        if owner.is_some_and(|h| h != stored) {
            return Err("owner handle differs from the migrated database owner".into());
        }
        Ok(Self {
            db,
            _lock: lock,
            owner: stored,
            changed: Vec::new(),
            self_issued: std::collections::HashMap::new(),
        })
    }
    /// Internal key for a caller-facing project name in the caller's namespace.
    fn project_key(me: &str, name: &str) -> String {
        format!("{me}/{name}")
    }
    /// Handle of the enabled user a credential acts for; legacy credentials act for the
    /// admin. Unknown or disabled users are rejected on every request, without restart.
    fn caller(&self, access: &Access) -> Result<String> {
        let Some(handle) = access.user() else {
            return Ok(self.owner.clone());
        };
        self.db
            .query_row(
                "SELECT handle FROM users WHERE handle=?1 AND disabled=0",
                [handle],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| DB_ERROR)?
            .ok_or_else(|| access::INACTIVE.into())
    }
    /// Resolves a caller-facing name: the caller's own project wins; otherwise
    /// `<owner>/<name>` reaches a project shared with the caller by an enabled owner.
    /// Anything else is the caller's own (possibly new) project, so unshared projects of
    /// other users look exactly like missing ones.
    fn resolve(&self, me: &str, typed: &str, write: bool) -> Result<String> {
        let own = Self::project_key(me, typed);
        if !typed.contains('/') {
            return Ok(own);
        }
        let exists: Option<i64> = self
            .db
            .query_row(
                "SELECT 1 FROM memories WHERE project=?1 LIMIT 1",
                [&own],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| DB_ERROR)?;
        if exists.is_some() {
            return Ok(own);
        }
        let role: Option<String> = self
            .db
            .query_row(
                "SELECT pm.role FROM project_members pm JOIN users m ON m.id=pm.user_id \
                 JOIN users o ON substr(pm.project,1,length(o.handle)+1)=o.handle||'/' \
                 WHERE pm.project=?1 AND m.handle=?2 AND o.handle!=?2 AND o.disabled=0",
                params![typed, me],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| DB_ERROR)?;
        match role.as_deref() {
            Some("write") => Ok(typed.to_owned()),
            Some(_) if !write => Ok(typed.to_owned()),
            Some(_) => Err(access::DENIED.into()),
            // Writing to `<other user>/name` never silently creates a look-alike own
            // project that would later shadow a share; the answer is the same whether it
            // is missing, unshared or read-only. Other slashed names stay own projects.
            None if write && self.other_user(me, typed)? => Err(access::DENIED.into()),
            None => Ok(own),
        }
    }
    /// Archives or restores (owner or write member), or purges (owner only, archived
    /// memories only) one memory. A purge deletes every revision, its metadata, topic,
    /// index entry and idempotency receipts, then compacts the index and the WAL so the
    /// content does not linger in free pages; only a content-free record remains.
    fn manage(&mut self, view: &str, args: &Value, me: &str) -> Result<Value> {
        let purge = view == "purge";
        allowed(
            args,
            if purge {
                &["project", "id", "confirm"]
            } else {
                &["project", "id", "archived"]
            },
        )?;
        let typed = text(args, "project", 128)?;
        let id = text(args, "id", 128)?;
        let key = self.resolve(me, &typed, true)?;
        let archive = if purge {
            if !key.starts_with(&format!("{me}/")) {
                return Err(access::DENIED.into());
            }
            if !matches!(text(args, "confirm", 16)?.as_str(), "purgar" | "purge") {
                return Err(INVALID.into());
            }
            true
        } else {
            match args.get("archived") {
                Some(Value::Bool(b)) => *b,
                _ => return Err(INVALID.into()),
            }
        };
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| DB_ERROR)?;
        let archived: bool = tx
            .query_row(
                "SELECT archived_at IS NOT NULL FROM memories WHERE id=?1 AND project=?2",
                params![id, key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| DB_ERROR)?
            .ok_or(crate::panel_query::NOT_FOUND)?;
        if purge && !archived {
            return Err(MUST_ARCHIVE.into());
        }
        if !purge && archived == archive {
            return Ok(json!({"id":id,"project":key,"archived":archive}));
        }
        // Listing snapshots add revisions and this counter; a purge removes revisions,
        // so it counts them here to keep every snapshot moving forward.
        let mut changes = 1i64;
        if purge {
            changes += tx
                .query_row("SELECT count(*) FROM revisions WHERE id=?1", [&id], |r| {
                    r.get::<_, i64>(0)
                })
                .map_err(|_| DB_ERROR)?;
            let rowid = "(SELECT rowid FROM memories WHERE id=?1)";
            for sql in [
                format!("DELETE FROM memory_fts WHERE rowid={rowid}"),
                "DELETE FROM topics WHERE id=?1".into(),
                "DELETE FROM revision_metadata WHERE id=?1".into(),
                "DELETE FROM revisions WHERE id=?1".into(),
                // Receipts keep their operation_id without content, so retrying an
                // old remember is refused instead of recreating the purged memory.
                "UPDATE operations SET request=?2,response='{}' WHERE \
                 CASE WHEN json_valid(response) THEN json_extract(response,'$.id') END=?1 \
                 OR CASE WHEN json_valid(request) THEN json_extract(request,'$.arguments.id') END=?1"
                    .into(),
                "DELETE FROM memories WHERE id=?1".into(),
            ] {
                if sql.contains("?2") {
                    tx.execute(&sql, params![id, PURGED_RECEIPT])
                } else {
                    tx.execute(&sql, [&id])
                }
                .map_err(|_| DB_ERROR)?;
            }
            tx.execute(
                "INSERT INTO purges VALUES(?1,?2,?3,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
                params![id, key, me],
            )
            .map_err(|_| DB_ERROR)?;
        } else {
            tx.execute(
                "UPDATE memories SET archived_at=CASE WHEN ?3 THEN strftime('%Y-%m-%dT%H:%M:%fZ','now') END \
                 WHERE id=?1 AND project=?2",
                params![id, key, archive],
            )
            .map_err(|_| DB_ERROR)?;
        }
        tx.execute(
            "INSERT INTO project_changes VALUES(?1,?2) ON CONFLICT(project) DO UPDATE SET changes=changes+?2",
            params![key, changes],
        )
        .map_err(|_| DB_ERROR)?;
        tx.commit().map_err(|_| DB_ERROR)?;
        self.changed.push((key.clone(), false));
        if purge {
            // Merge FTS segments so deleted tokens leave the index, then move the WAL
            // into the (secure_delete) database file and truncate it. The purge is
            // already committed, so a failure here does not undo or misreport it; the
            // next purge or checkpoint retries the compaction.
            let _ = self
                .db
                .execute_batch("INSERT INTO memory_fts(memory_fts) VALUES('optimize')");
            let _ = self
                .db
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                    r.get::<_, i64>(0)
                });
            return Ok(json!({"id":id,"project":key,"purged":true}));
        }
        Ok(json!({"id":id,"project":key,"archived":archive}))
    }
    /// Whether `me` may write the project with internal key `key`: own projects, or
    /// projects of an enabled owner shared with `me` as `write`.
    fn writable(&self, me: &str, key: &str) -> Result<bool> {
        if key.starts_with(&format!("{me}/")) {
            return Ok(true);
        }
        self.db
            .query_row(
                "SELECT 1 FROM project_members pm JOIN users m ON m.id=pm.user_id \
                 JOIN users o ON substr(pm.project,1,length(o.handle)+1)=o.handle||'/' \
                 WHERE pm.project=?1 AND m.handle=?2 AND o.handle!=?2 AND o.disabled=0 \
                 AND pm.role='write'",
                params![key, me],
                |_| Ok(()),
            )
            .optional()
            .map(|r| r.is_some())
            .map_err(|_| DB_ERROR.into())
    }
    /// Whether the first segment of a typed project names another existing user.
    fn other_user(&self, me: &str, typed: &str) -> Result<bool> {
        let first = typed.split('/').next().unwrap_or("");
        if first == me {
            return Ok(false);
        }
        self.db
            .query_row("SELECT 1 FROM users WHERE handle=?1", [first], |_| Ok(()))
            .optional()
            .map(|r| r.is_some())
            .map_err(|_| DB_ERROR.into())
    }
    /// Handle of a live panel session of an enabled user, matching the signed cookie.
    fn panel_user(&self, session: &str, handle: Option<&str>) -> Result<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        self.db
            .query_row(
                "SELECT u.handle FROM oauth_tokens t JOIN users u ON u.id=t.user_id \
                 WHERE t.hash=?1 AND t.client_id=?2 AND t.kind='access' AND t.expires_at>=?3 AND u.disabled=0",
                params![session, PANEL_CLIENT, now],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| DB_ERROR)?
            .filter(|h| Some(h.as_str()) == handle)
            .ok_or_else(|| access::INACTIVE.into())
    }
    /// Internal keys of every project the user may read: own projects plus projects
    /// shared with them by an enabled owner, as a JSON array for `json_each`.
    fn visible(&self, me: &str) -> Result<String> {
        let mut stmt = self
            .db
            .prepare(
                "SELECT DISTINCT project FROM memories WHERE project>=?1||'/' AND project<?1||'0' \
                 UNION SELECT pm.project FROM project_members pm JOIN users m ON m.id=pm.user_id \
                 JOIN users o ON substr(pm.project,1,length(o.handle)+1)=o.handle||'/' \
                 WHERE m.handle=?1 AND o.handle!=?1 AND o.disabled=0 ORDER BY 1",
            )
            .map_err(|_| DB_ERROR)?;
        let keys = stmt
            .query_map([me], |r| r.get::<_, String>(0))
            .map_err(|_| DB_ERROR)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| DB_ERROR)?;
        Ok(json!(keys).to_string())
    }
    /// Own projects (with members) and projects shared with the caller.
    fn projects(&self, args: &Value, access: &Access, me: &str) -> Result<Value> {
        allowed(args, &[])?;
        let prefix = format!("{me}/");
        let mut out = Vec::new();
        let mut own = self
            .db
            .prepare("SELECT DISTINCT project FROM memories WHERE substr(project,1,length(?1))=?1 ORDER BY project LIMIT 200")
            .map_err(|_| DB_ERROR)?;
        let keys = own
            .query_map([&prefix], |r| r.get::<_, String>(0))
            .map_err(|_| DB_ERROR)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| DB_ERROR)?;
        let mut members = self
            .db
            .prepare("SELECT u.email,u.handle,pm.role FROM project_members pm JOIN users u ON u.id=pm.user_id WHERE pm.project=?1 ORDER BY u.handle")
            .map_err(|_| DB_ERROR)?;
        for key in keys {
            let short = &key[prefix.len()..];
            if !access.can_read(short) {
                continue;
            }
            let list = members
                .query_map([&key], |r| {
                    Ok(json!({"email":r.get::<_,Option<String>>(0)?,"handle":r.get::<_,String>(1)?,"role":r.get::<_,String>(2)?}))
                })
                .map_err(|_| DB_ERROR)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| DB_ERROR)?;
            let mut item = json!({"project":short,"role":"owner"});
            if !list.is_empty() {
                item["members"] = json!(list);
            }
            out.push(item);
        }
        let mut shared = self
            .db
            .prepare(
                "SELECT pm.project,pm.role FROM project_members pm JOIN users m ON m.id=pm.user_id \
                 JOIN users o ON substr(pm.project,1,length(o.handle)+1)=o.handle||'/' \
                 WHERE m.handle=?1 AND o.handle!=?1 AND o.disabled=0 ORDER BY pm.project LIMIT 200",
            )
            .map_err(|_| DB_ERROR)?;
        let rows = shared
            .query_map([me], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|_| DB_ERROR)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| DB_ERROR)?;
        for (project, role) in rows {
            if access.can_read(&project) {
                out.push(json!({"project":project,"role":role}));
            }
        }
        Ok(json!({"projects":out}))
    }
    /// Owner-only sharing of an existing own project with an enabled user, by email.
    fn share(&mut self, name: &str, args: &Value, me: &str) -> Result<Value> {
        let sharing = name == "share";
        if sharing {
            allowed(args, &["project", "email", "role"])?;
        } else {
            allowed(args, &["project", "email"])?;
        }
        let short = text(args, "project", 128)?;
        let email = text(args, "email", 254)?.to_lowercase();
        let key = Self::project_key(me, &short);
        let exists: Option<i64> = self
            .db
            .query_row(
                "SELECT 1 FROM memories WHERE project=?1 LIMIT 1",
                [&key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| DB_ERROR)?;
        if exists.is_none() {
            return Err("project not found among your own projects".into());
        }
        if sharing {
            let role = text(args, "role", 8)?;
            if role != "read" && role != "write" {
                return Err(INVALID.into());
            }
            let user: Option<i64> = self
                .db
                .query_row(
                    "SELECT id FROM users WHERE email=?1 AND disabled=0 AND handle!=?2",
                    params![email, me],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|_| DB_ERROR)?;
            let user = user.ok_or("no enabled user with that email")?;
            self.db
                .execute(
                    "INSERT INTO project_members(project,user_id,role) VALUES(?1,?2,?3) ON CONFLICT(project,user_id) DO UPDATE SET role=excluded.role",
                    params![key, user, role],
                )
                .map_err(|_| DB_ERROR)?;
            self.changed.push((key, true));
            Ok(json!({"project":short,"email":email,"role":role}))
        } else {
            let removed = self
                .db
                .execute(
                    "DELETE FROM project_members WHERE project=?1 AND user_id=(SELECT id FROM users WHERE email=?2)",
                    params![key, email],
                )
                .map_err(|_| DB_ERROR)?;
            if removed != 1 {
                return Err("project is not shared with that email".into());
            }
            self.changed.push((key, true));
            Ok(json!({"project":short,"email":email,"removed":true}))
        }
    }
    /// Shows the caller's own projects by their short name in every `project` field.
    fn present(me: &str, value: &mut Value) {
        let prefix = format!("{me}/");
        match value {
            Value::Object(map) => {
                for (k, v) in map.iter_mut() {
                    match v {
                        Value::String(p) if k == "project" => {
                            if let Some(short) = p.strip_prefix(&prefix) {
                                *p = short.to_owned();
                            }
                        }
                        // Panel facets list project names as plain strings.
                        Value::Array(names) if k == "projects" => {
                            for name in names.iter_mut() {
                                if let Value::String(p) = name
                                    && let Some(short) = p.strip_prefix(&prefix)
                                {
                                    *p = short.to_owned();
                                }
                            }
                        }
                        _ => Self::present(me, v),
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| Self::present(me, v)),
            _ => {}
        }
    }
    /// Local owner call with full access, recorded under the admin user.
    pub fn call(&mut self, name: &str, args: Value) -> Result<Value> {
        self.call_as(name, args, &Access::local())
    }
    pub fn call_as(&mut self, name: &str, mut args: Value, access: &Access) -> Result<Value> {
        // Read-only panel queries never persist input, so they skip the write-side secret scan.
        // Each panel session sees only its user's projects and those shared with them,
        // named as in the tools; the read-key session (local identity) is the admin.
        if let Some(view) = name.strip_prefix("panel.") {
            // Only the panel layer reaches these: the local identity (read key) or a
            // `panel:` principal, which no auth.json name or OAuth client can produce.
            if !(access.is_local()
                || access.principal().starts_with(PANEL_PRINCIPAL) && access.reads_everything())
            {
                return Err(access::DENIED.into());
            }
            let me = match access.principal().strip_prefix(PANEL_PRINCIPAL) {
                _ if access.is_local() => self.owner.clone(),
                Some(session) => self.panel_user(session, access.user())?,
                None => return Err(access::DENIED.into()),
            };
            // Google profile of a signed-in user; the read key has none.
            if view == "profile" {
                allowed(&args, &[])?;
                if access.is_local() {
                    return Ok(json!({"name":null,"picture":null,"email":null,"admin":false}));
                }
                let (name, picture, email): (Option<String>, Option<String>, Option<String>) = self
                    .db
                    .query_row(
                        "SELECT name,picture,email FROM users WHERE handle=?1",
                        [&me],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .map_err(|_| DB_ERROR)?;
                return Ok(
                    json!({"name":name,"picture":picture,"email":email,"admin":me == self.owner}),
                );
            }
            // A signed-in user's own personal token (never the read key): replaces the
            // previous one, so agents and panel sessions that used it must be updated.
            if view == "my_token" {
                if access.is_local() {
                    return Err(access::DENIED.into());
                }
                allowed(&args, &[])?;
                if me == self.owner {
                    return Err(ADMIN_NO_PERSONAL.into());
                }
                // A session opened with the personal token belongs to its family and would end
                // with it, before the new token could be copied.
                let session = access
                    .principal()
                    .strip_prefix(PANEL_PRINCIPAL)
                    .unwrap_or("");
                let by_token: bool = self
                    .db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM oauth_tokens t JOIN personal_tokens p ON p.hash=t.family \
                         JOIN users u ON u.id=p.user_id WHERE t.hash=?1 AND u.handle=?2)",
                        params![session, me],
                        |r| r.get(0),
                    )
                    .map_err(|_| DB_ERROR)?;
                if by_token {
                    return Err(TOKEN_SESSION.into());
                }
                if self
                    .self_issued
                    .get(&me)
                    .is_some_and(|t| t.elapsed() < SELF_ISSUE_INTERVAL)
                {
                    return Err(TOKEN_TOO_SOON.into());
                }
                let token = self.reissue(&me, access::INACTIVE)?;
                self.self_issued
                    .insert(me.clone(), std::time::Instant::now());
                self.changed.push(("*".into(), true));
                return Ok(json!({"handle":me,"token":token}));
            }
            // User administration: only a signed-in admin session (Google or admin key).
            // The read key stays read-only; anyone else sees no such views.
            if ADMIN_VIEWS.contains(&view) {
                if access.is_local() {
                    return Err(access::DENIED.into());
                }
                if me != self.owner {
                    return Err(crate::panel_query::NOT_FOUND.into());
                }
                let result = self.administer(view, &args)?;
                // Token changes end sessions too: wake the streams to re-check now.
                if view != "users" {
                    self.changed.push(("*".into(), true));
                }
                return Ok(result);
            }
            // Sharing from the panel: the owner of a project, like the share/unshare tools.
            if matches!(view, "sharing" | "share" | "unshare") {
                if access.is_local() {
                    return Err(access::DENIED.into());
                }
                return self.panel_share(view, &args, &me);
            }
            // Edits from a signed-in user (never the read key) go through `correct` with
            // the user's own rights: owner or write member, enforced by `resolve`.
            if view == "correct" {
                if access.is_local() {
                    return Err(access::DENIED.into());
                }
                let fields = args.as_object().ok_or(INVALID)?;
                if fields
                    .keys()
                    .any(|k| !PANEL_EDIT_FIELDS.contains(&k.as_str()))
                {
                    return Err(INVALID.into());
                }
                let mut edit = args.clone();
                // An empty title keeps the current one (memories may have none).
                if edit
                    .get("title")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.trim().is_empty())
                {
                    edit.as_object_mut().ok_or(INVALID)?.remove("title");
                }
                edit["source"] = json!("panel");
                edit["agent"] = json!("panel");
                // `call_as` already shows project names as this user sees them.
                return self.call_as("correct", edit, access);
            }
            // Archive, restore and purge exist only here: agents have no such tools.
            if view == "archive" || view == "purge" {
                if access.is_local() {
                    return Err(access::DENIED.into());
                }
                let mut result = self.manage(view, &args, &me)?;
                Self::present(&me, &mut result);
                return Ok(result);
            }
            let visible = self.visible(&me)?;
            // Internal keys the session may see; only the live-updates stream asks.
            if view == "visible" {
                allowed(&args, &[])?;
                let keys: Value = serde_json::from_str(&visible).map_err(|_| DB_ERROR)?;
                return Ok(json!({"me":me,"keys":keys}));
            }
            if args.get("project").is_some_and(|p| !p.is_null()) {
                let short = text(&args, "project", 128)
                    .map_err(|_| crate::panel_query::INVALID.to_string())?;
                args["project"] = json!(self.resolve(&me, &short, false)?);
            }
            let mut result = crate::panel_query::run(&self.db, view, &args, &visible)?;
            if view == "memory" {
                let project = args["project"].as_str().unwrap_or_default();
                let writable = !access.is_local() && self.writable(&me, project)?;
                let owner = project.starts_with(&format!("{me}/"));
                result["can_edit"] = json!(writable && result["archived_at"].is_null());
                result["can_archive"] = json!(writable);
                result["can_purge"] =
                    json!(!access.is_local() && owner && !result["archived_at"].is_null());
            }
            Self::present(&me, &mut result);
            return Ok(result);
        }
        if name == "backup" && !access.is_local() {
            return Err("unknown tool".into());
        }
        if let Some(action) = name.strip_prefix("users.") {
            if !access.is_local() {
                return Err("unknown tool".into());
            }
            let result = self.users(action, &args);
            // Enabling or disabling a user changes what other sessions can see.
            if result.is_ok() && action != "list" {
                self.changed.push(("*".into(), true));
            }
            return result;
        }
        // Internal OAuth persistence, called by the HTTP layer with the local identity.
        if let Some(action) = name.strip_prefix("oauth.") {
            if !access.is_local() {
                return Err("unknown tool".into());
            }
            return self.oauth(action, &args);
        }
        let me = self.caller(access)?;
        if name == "auth.check" {
            return Ok(json!({"user":me}));
        }
        validate_arguments(&args)?;
        // Deny before any lookup so the answer is the same whether the project exists or not.
        if let Some(project) = args.get("project").and_then(Value::as_str) {
            let permitted = match name {
                "remember" | "correct" | "share" | "unshare" => access.can_write(project),
                _ => access.can_read(project),
            };
            if !permitted {
                return Err(access::DENIED.into());
            }
        }
        let compact = optional_bool(&args, "compact")?;
        args.as_object_mut().ok_or(INVALID)?.remove("compact");
        match name {
            "projects" => return self.projects(&args, access, &me),
            "share" | "unshare" => return self.share(name, &args, &me),
            _ => {}
        }
        let paging = args.clone();
        // Receipts keep the caller-facing arguments so retries across v4 still match.
        let typed = args.clone();
        if args.get("project").is_some() {
            let typed = text(&args, "project", 128)?;
            let key = self.resolve(&me, &typed, matches!(name, "remember" | "correct"))?;
            args["project"] = json!(key);
        }
        let result = match name {
            "remember" | "correct" => self.write(name, args, &typed, compact, access, &me),
            "recall" => {
                allowed(&args, &["project", "id", "revision", "offset"])?;
                if !compact && (args.get("revision").is_some() || args.get("offset").is_some()) {
                    return Err("revision and offset require compact mode".into());
                }
                let project = text(&args, "project", PROJECT_KEY_MAX)?;
                let id = text(&args, "id", 128)?;
                if args.get("revision").is_some() {
                    let revision = number(&args, "revision", 1, 1, i64::MAX)?;
                    self.db
                        .query_row(
                            &format!("{SELECT} WHERE m.project=?1 AND m.id=?2 AND r.revision=?3 AND m.archived_at IS NULL"),
                            params![project, id, revision],
                            row,
                        )
                        .optional()
                        .map_err(|_| DB_ERROR)?
                        .ok_or_else(|| "memory revision not found in this project".into())
                } else {
                    if number(&args, "offset", 0, 0, i64::MAX)? != 0 {
                        return Err("continuation requires revision".into());
                    }
                    self.recall(&project, &id)
                }
            }
            "history" => self.history(&args),
            "guide" => {
                allowed(&args, &[])?;
                Ok(json!({
                    "guide": include_str!("guide.md").trim(),
                    "principal": me,
                    "read": access.read_patterns(),
                    "write": access.write_patterns(),
                }))
            }
            "search" | "context" => self.search(name, &args),
            "status" => {
                allowed(&args, &[])?;
                let (count, last) = self.status(access, &me)?;
                Ok(
                    json!({"connected":true,"memories":count,"last_confirmed_save":last,"schema_version":SCHEMA_VERSION}),
                )
            }
            "backup" => {
                allowed(&args, &["destination"])?;
                self.backup(Path::new(&text(&args, "destination", 4096)?))?;
                Ok(json!({"backed_up":true}))
            }
            _ => Err("unknown tool".into()),
        }
        .map(|mut v| {
            Self::present(&me, &mut v);
            v
        })?;
        if compact {
            crate::compact::render(name, result, &paging)
        } else {
            Ok(result)
        }
    }
    /// Counts only the caller's projects this credential can read, with the latest
    /// revision time among them; other users' activity is never revealed.
    fn status(&self, access: &Access, me: &str) -> Result<(i64, Option<String>)> {
        let prefix = format!("{me}/");
        let mut stmt = self
            .db
            .prepare("SELECT m.project,count(DISTINCT m.id),max(r.saved_at) FROM memories m JOIN revisions r ON r.id=m.id WHERE substr(m.project,1,length(?1))=?1 AND m.archived_at IS NULL GROUP BY m.project")
            .map_err(|_| DB_ERROR)?;
        let rows = stmt
            .query_map([&prefix], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|_| DB_ERROR)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| DB_ERROR)?;
        let (mut count, mut last) = (0, None::<String>);
        for (project, n, saved) in rows {
            // Credential patterns name caller-facing projects of the caller's namespace.
            if project
                .strip_prefix(&prefix)
                .is_some_and(|short| access.can_read(short))
            {
                count += n;
                if last.as_ref().is_none_or(|l| saved > *l) {
                    last = Some(saved);
                }
            }
        }
        Ok((count, last))
    }
    /// OAuth clients and tokens. Callers pass only SHA-256 hashes of secrets and the
    /// current time; lifetimes are fixed here.
    fn oauth(&mut self, action: &str, args: &Value) -> Result<Value> {
        const ACCESS_TTL: i64 = 3600;
        const REFRESH_TTL: i64 = 30 * 86400;
        const GRANT: &str = "invalid_grant";
        let hash = |key: &str| -> Result<String> {
            text(args, key, 64)
                .ok()
                .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or_else(|| INVALID.to_string())
        };
        let now = || number(args, "now", -1, 0, i64::MAX / 2);
        match action {
            "register" => {
                allowed(args, &["client_id", "redirect_uris", "client_name"])?;
                let client = text(args, "client_id", 64)?;
                let name = args
                    .get("client_name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .chars()
                    .take(128)
                    .collect::<String>();
                let uris = args
                    .get("redirect_uris")
                    .and_then(Value::as_array)
                    .filter(|u| (1..=5).contains(&u.len()) && u.iter().all(Value::is_string))
                    .ok_or(INVALID)?;
                let count: i64 = self
                    .db
                    .query_row("SELECT count(*) FROM oauth_clients", [], |r| r.get(0))
                    .map_err(|_| DB_ERROR)?;
                if count >= 1000 {
                    // Make room by dropping the oldest client that never obtained a
                    // live token and is older than an hour (abandoned registrations).
                    self.db
                        .execute(
                            "DELETE FROM oauth_clients WHERE client_id=(SELECT c.client_id FROM oauth_clients c \
                             WHERE c.created_at<strftime('%Y-%m-%dT%H:%M:%fZ','now','-1 hour') \
                             AND NOT EXISTS(SELECT 1 FROM oauth_tokens t WHERE t.client_id=c.client_id) \
                             ORDER BY c.created_at LIMIT 1)",
                            [],
                        )
                        .map_err(|_| DB_ERROR)?;
                    let count: i64 = self
                        .db
                        .query_row("SELECT count(*) FROM oauth_clients", [], |r| r.get(0))
                        .map_err(|_| DB_ERROR)?;
                    if count >= 1000 {
                        return Err("client registration limit reached".into());
                    }
                }
                self.db
                    .execute(
                        "INSERT INTO oauth_clients VALUES(?1,?2,?3,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
                        params![client, Value::Array(uris.clone()).to_string(), name],
                    )
                    .map_err(|_| "client already registered or invalid")?;
                Ok(json!({"client_id":client}))
            }
            "client" => {
                allowed(args, &["client_id"])?;
                let client = text(args, "client_id", 64)?;
                let uris: String = self
                    .db
                    .query_row(
                        "SELECT redirect_uris FROM oauth_clients WHERE client_id=?1",
                        [&client],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|_| DB_ERROR)?
                    .ok_or("unknown client")?;
                Ok(
                    json!({"redirect_uris":serde_json::from_str::<Value>(&uris).map_err(|_| DB_ERROR)?}),
                )
            }
            "login" => {
                // Only invited, enabled emails sign in; the first login binds the Google
                // subject so another account claiming the same email is rejected.
                allowed(args, &["email", "sub", "name", "picture"])?;
                let email = text(args, "email", 254)?.to_lowercase();
                let sub = text(args, "sub", 255)?;
                // Profile shown by the panel; anything unexpected is dropped, not rejected.
                let name = args
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|n| !n.is_empty() && n.chars().count() <= 128)
                    .filter(|n| !n.chars().any(|c| c.is_control() || invisible(c)));
                let picture = args
                    .get("picture")
                    .and_then(Value::as_str)
                    .filter(|p| valid_picture(p));
                let tx = self
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| DB_ERROR)?;
                let found: Option<(i64, String, Option<String>)> = tx
                    .query_row(
                        "SELECT id,handle,google_sub FROM users WHERE email=?1 AND disabled=0",
                        [&email],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()
                    .map_err(|_| DB_ERROR)?;
                let (id, handle, bound) = found.ok_or("email not invited")?;
                match bound {
                    Some(b) if b != sub => return Err("email not invited".into()),
                    Some(_) => {}
                    None => {
                        tx.execute(
                            "UPDATE users SET google_sub=?1 WHERE id=?2",
                            params![sub, id],
                        )
                        .map_err(|_| "email not invited")?;
                    }
                }
                tx.execute(
                    "UPDATE users SET name=?1,picture=?2 WHERE id=?3",
                    params![name, picture, id],
                )
                .map_err(|_| DB_ERROR)?;
                tx.commit().map_err(|_| DB_ERROR)?;
                Ok(json!({"user_id":id,"handle":handle}))
            }
            "issue" => {
                allowed(
                    args,
                    &["user_id", "client_id", "family", "access", "refresh", "now"],
                )?;
                let user = number(args, "user_id", 0, 1, i64::MAX)?;
                let client = text(args, "client_id", 64)?;
                let (family, access, refresh, now) =
                    (hash("family")?, hash("access")?, hash("refresh")?, now()?);
                let tx = self
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| DB_ERROR)?;
                let enabled: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM users WHERE id=?1 AND disabled=0",
                        [user],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|_| DB_ERROR)?;
                if enabled.is_none() {
                    return Err(GRANT.into());
                }
                tx.execute("DELETE FROM oauth_tokens WHERE expires_at<?1", [now])
                    .map_err(|_| DB_ERROR)?;
                for (h, kind, ttl) in [
                    (&access, "access", ACCESS_TTL),
                    (&refresh, "refresh", REFRESH_TTL),
                ] {
                    tx.execute(
                        "INSERT INTO oauth_tokens VALUES(?1,?2,?3,?4,?5,?6,0)",
                        params![h, kind, user, client, family, now + ttl],
                    )
                    .map_err(|_| GRANT)?;
                }
                tx.commit().map_err(|_| DB_ERROR)?;
                Ok(json!({"expires_in":ACCESS_TTL}))
            }
            "refresh" => {
                allowed(
                    args,
                    &["refresh", "client_id", "access", "new_refresh", "now"],
                )?;
                let client = text(args, "client_id", 64)?;
                let (old, access, refresh, now) = (
                    hash("refresh")?,
                    hash("access")?,
                    hash("new_refresh")?,
                    now()?,
                );
                let tx = self
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| DB_ERROR)?;
                let row: Option<(i64, String, String, i64, bool)> = tx
                    .query_row(
                        "SELECT user_id,client_id,family,expires_at,used FROM oauth_tokens WHERE hash=?1 AND kind='refresh'",
                        [&old],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                    )
                    .optional()
                    .map_err(|_| DB_ERROR)?;
                let Some((user, owner, family, expires, used)) = row else {
                    return Err(GRANT.into());
                };
                if owner != client {
                    return Err(GRANT.into());
                }
                if used {
                    // Replay of a rotated token: assume theft and end the whole session.
                    tx.execute("DELETE FROM oauth_tokens WHERE family=?1", [&family])
                        .map_err(|_| DB_ERROR)?;
                    tx.commit().map_err(|_| DB_ERROR)?;
                    return Err(GRANT.into());
                }
                let enabled: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM users WHERE id=?1 AND disabled=0",
                        [user],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|_| DB_ERROR)?;
                if expires < now || enabled.is_none() {
                    return Err(GRANT.into());
                }
                tx.execute("UPDATE oauth_tokens SET used=1 WHERE hash=?1", [&old])
                    .map_err(|_| DB_ERROR)?;
                for (h, kind, ttl) in [
                    (&access, "access", ACCESS_TTL),
                    (&refresh, "refresh", REFRESH_TTL),
                ] {
                    tx.execute(
                        "INSERT INTO oauth_tokens VALUES(?1,?2,?3,?4,?5,?6,0)",
                        params![h, kind, user, client, family, now + ttl],
                    )
                    .map_err(|_| GRANT)?;
                }
                tx.commit().map_err(|_| DB_ERROR)?;
                Ok(json!({"expires_in":ACCESS_TTL}))
            }
            "revoke" => {
                allowed(args, &["family"])?;
                let family = hash("family")?;
                self.db
                    .execute("DELETE FROM oauth_tokens WHERE family=?1", [&family])
                    .map_err(|_| DB_ERROR)?;
                Ok(json!({"revoked":true}))
            }
            "panel_session" => {
                allowed(args, &["user_id", "hash", "now"])?;
                let user = number(args, "user_id", 0, 1, i64::MAX)?;
                let (session, now) = (hash("hash")?, now()?);
                self.open_panel_session(user, &session, &session, now)?;
                Ok(json!({"expires_at":now + PANEL_TTL}))
            }
            // Panel sign-in with the admin key (no `token`) or a personal token (its
            // SHA-256). A session opened with a personal token joins that token's family,
            // so regenerating the token also ends it.
            "panel_login" => {
                allowed(args, &["hash", "now", "token"])?;
                let (session, now) = (hash("hash")?, now()?);
                let token = match args.get("token") {
                    Some(_) => Some(hash("token")?),
                    None => None,
                };
                let found: Option<(i64, String)> = match &token {
                    None => self
                        .db
                        .query_row("SELECT id,handle FROM users WHERE admin=1", [], |r| {
                            Ok((r.get(0)?, r.get(1)?))
                        })
                        .optional(),
                    Some(t) => self
                        .db
                        .query_row(
                            "SELECT u.id,u.handle FROM personal_tokens p JOIN users u ON u.id=p.user_id \
                             WHERE p.hash=?1 AND u.disabled=0",
                            [t],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional(),
                }
                .map_err(|_| DB_ERROR)?;
                let (user, handle) = found.ok_or(access::INACTIVE)?;
                let family = token.clone().unwrap_or_else(|| session.clone());
                self.open_panel_session(user, &session, &family, now)?;
                if let Some(t) = &token {
                    self.db
                        .execute(
                            "UPDATE personal_tokens SET used=1 WHERE hash=?1 AND used=0",
                            [t],
                        )
                        .map_err(|_| DB_ERROR)?;
                }
                Ok(json!({"handle":handle,"expires_at":now + PANEL_TTL}))
            }
            // Bearer personal token on /mcp (its SHA-256): the enabled user it belongs to.
            "personal" => {
                allowed(args, &["hash"])?;
                let token = hash("hash")?;
                let handle: String = self
                    .db
                    .query_row(
                        "SELECT u.handle FROM personal_tokens p JOIN users u ON u.id=p.user_id \
                         WHERE p.hash=?1 AND u.disabled=0",
                        [&token],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|_| DB_ERROR)?
                    .ok_or(access::INACTIVE)?;
                self.db
                    .execute(
                        "UPDATE personal_tokens SET used=1 WHERE hash=?1 AND used=0",
                        [&token],
                    )
                    .map_err(|_| DB_ERROR)?;
                Ok(json!({"handle":handle}))
            }
            "panel_end" => {
                allowed(args, &["hash"])?;
                self.db
                    .execute(
                        "DELETE FROM oauth_tokens WHERE hash=?1 AND client_id=?2",
                        params![hash("hash")?, PANEL_CLIENT],
                    )
                    .map_err(|_| DB_ERROR)?;
                Ok(json!({"ended":true}))
            }
            "access" => {
                allowed(args, &["hash", "now"])?;
                let (token, now) = (hash("hash")?, now()?);
                self.db
                    .query_row(
                        "SELECT u.handle,t.client_id FROM oauth_tokens t JOIN users u ON u.id=t.user_id \
                         WHERE t.hash=?1 AND t.kind='access' AND t.expires_at>=?2 AND u.disabled=0 AND t.client_id!=?3",
                        params![token, now, PANEL_CLIENT],
                        |r| Ok(json!({"handle":r.get::<_,String>(0)?,"client_id":r.get::<_,String>(1)?})),
                    )
                    .optional()
                    .map_err(|_| DB_ERROR)?
                    .ok_or_else(|| access::INACTIVE.into())
            }
            _ => Err("unknown tool".into()),
        }
    }
    /// Local administration of invited users. Invitation enables an email for a handle.
    fn users(&mut self, action: &str, args: &Value) -> Result<Value> {
        match action {
            "list" => {
                allowed(args, &[])?;
                let mut stmt = self
                    .db
                    .prepare("SELECT handle,email,admin,disabled,created_at FROM users ORDER BY admin DESC,handle")
                    .map_err(|_| DB_ERROR)?;
                let users = stmt
                    .query_map([], |r| {
                        Ok(json!({"handle":r.get::<_,String>(0)?,"email":r.get::<_,Option<String>>(1)?,"admin":r.get::<_,bool>(2)?,"disabled":r.get::<_,bool>(3)?,"created_at":r.get::<_,String>(4)?}))
                    })
                    .map_err(|_| DB_ERROR)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|_| DB_ERROR)?;
                Ok(json!({"users":users}))
            }
            "invite" => {
                allowed(args, &["email", "handle"])?;
                let handle = text(args, "handle", 32)?;
                let email = text(args, "email", 254)?.to_lowercase();
                let valid_email = email.split('@').count() == 2
                    && email.split('@').all(|part| !part.is_empty())
                    && !email.chars().any(|c| c.is_whitespace() || c.is_control());
                if !access::valid_handle(&handle) || !valid_email {
                    return Err(INVALID.into());
                }
                let tx = self
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| DB_ERROR)?;
                let by_email: Option<String> = tx
                    .query_row("SELECT handle FROM users WHERE email=?1", [&email], |r| {
                        r.get(0)
                    })
                    .optional()
                    .map_err(|_| DB_ERROR)?;
                let by_handle: Option<Option<String>> = tx
                    .query_row("SELECT email FROM users WHERE handle=?1", [&handle], |r| {
                        r.get(0)
                    })
                    .optional()
                    .map_err(|_| DB_ERROR)?;
                if by_email.as_ref().is_some_and(|h| *h != handle) {
                    return Err("email already belongs to another user".into());
                }
                match by_handle {
                    Some(Some(existing)) if existing != email => {
                        return Err("handle already belongs to another email".into());
                    }
                    Some(_) => tx.execute(
                        "UPDATE users SET email=?1,disabled=0 WHERE handle=?2",
                        params![email, handle],
                    ),
                    None => tx.execute(
                        "INSERT INTO users(handle,email,admin,disabled,created_at) VALUES(?1,?2,0,0,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
                        params![handle, email],
                    ),
                }
                .map_err(|_| DB_ERROR)?;
                tx.commit().map_err(|_| DB_ERROR)?;
                Ok(json!({"handle":handle,"email":email,"disabled":false}))
            }
            "disable" => {
                allowed(args, &["handle"])?;
                let handle = text(args, "handle", 32)?;
                let tx = self
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| DB_ERROR)?;
                let changed = tx
                    .execute(
                        "UPDATE users SET disabled=1,name=NULL,picture=NULL WHERE handle=?1 AND admin=0",
                        [&handle],
                    )
                    .map_err(|_| DB_ERROR)?;
                if changed != 1 {
                    return Err("user not found or is the admin".into());
                }
                // Re-inviting or re-enabling must not revive old sessions or tokens.
                for table in ["oauth_tokens", "personal_tokens"] {
                    tx.execute(
                        &format!(
                            "DELETE FROM {table} WHERE user_id=(SELECT id FROM users WHERE handle=?1)"
                        ),
                        [&handle],
                    )
                    .map_err(|_| DB_ERROR)?;
                }
                tx.commit().map_err(|_| DB_ERROR)?;
                Ok(json!({"handle":handle,"disabled":true}))
            }
            _ => Err("unknown tool".into()),
        }
    }
    /// Members of one of the caller's own projects (`sharing`) and changes to them. Users
    /// are named by handle; emails stay with the admin. Projects of others, shared or
    /// not, and own projects without memories all answer as missing.
    fn panel_share(&mut self, view: &str, args: &Value, me: &str) -> Result<Value> {
        const NO_USER: &str = "no enabled user with that handle";
        const NOT_SHARED: &str = "project is not shared with that user";
        const BAD_ROLE: &str = "role must be read or write";
        let missing = || crate::panel_query::NOT_FOUND.to_string();
        match view {
            "sharing" => allowed(args, &["project"])?,
            "share" => allowed(args, &["project", "handle", "role"])?,
            _ => allowed(args, &["project", "handle"])?,
        }
        let short = text(args, "project", 128).map_err(|_| missing())?;
        if short.contains('/') {
            return Err(missing());
        }
        let key = Self::project_key(me, &short);
        let exists: Option<i64> = self
            .db
            .query_row(
                "SELECT 1 FROM memories WHERE project=?1 LIMIT 1",
                [&key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| DB_ERROR)?;
        if exists.is_none() {
            return Err(missing());
        }
        if view == "sharing" {
            let people = |sql: &str, param: &str, with_role: bool| -> Result<Vec<Value>> {
                let mut stmt = self.db.prepare(sql).map_err(|_| DB_ERROR)?;
                stmt.query_map([param], |r| {
                    let mut item = json!({"handle":r.get::<_,String>(0)?,"name":r.get::<_,Option<String>>(1)?,
                        "picture":r.get::<_,Option<String>>(2)?,"disabled":r.get::<_,bool>(3)?});
                    if with_role {
                        item["role"] = json!(r.get::<_, String>(4)?);
                    }
                    Ok(item)
                })
                .map_err(|_| DB_ERROR)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| DB_ERROR.to_string())
            };
            let members = people(
                "SELECT u.handle,u.name,u.picture,u.disabled,pm.role FROM project_members pm \
                 JOIN users u ON u.id=pm.user_id WHERE pm.project=?1 ORDER BY u.handle",
                &key,
                true,
            )?;
            let candidates = people(
                "SELECT handle,name,picture,disabled FROM users WHERE disabled=0 AND handle!=?1 ORDER BY handle",
                me,
                false,
            )?;
            return Ok(json!({"project":short,"members":members,"people":candidates}));
        }
        let handle = text(args, "handle", 32).map_err(|_| NO_USER.to_string())?;
        if view == "share" {
            let role = text(args, "role", 8).map_err(|_| BAD_ROLE.to_string())?;
            if role != "read" && role != "write" {
                return Err(BAD_ROLE.into());
            }
            let user: i64 = self
                .db
                .query_row(
                    "SELECT id FROM users WHERE handle=?1 AND disabled=0 AND handle!=?2",
                    params![handle, me],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|_| DB_ERROR)?
                .ok_or(NO_USER)?;
            self.db
                .execute(
                    "INSERT INTO project_members(project,user_id,role) VALUES(?1,?2,?3) ON CONFLICT(project,user_id) DO UPDATE SET role=excluded.role",
                    params![key, user, role],
                )
                .map_err(|_| DB_ERROR)?;
            self.changed.push((key, true));
            return Ok(json!({"project":short,"handle":handle,"role":role}));
        }
        let removed = self
            .db
            .execute(
                "DELETE FROM project_members WHERE project=?1 AND user_id=(SELECT id FROM users WHERE handle=?2)",
                params![key, handle],
            )
            .map_err(|_| DB_ERROR)?;
        if removed != 1 {
            return Err(NOT_SHARED.into());
        }
        self.changed.push((key, true));
        Ok(json!({"project":short,"handle":handle,"removed":true}))
    }
    /// Records a panel session (SHA-256 of its cookie nonce) for an enabled user.
    /// `family` groups sessions that end together (a personal token's sessions).
    fn open_panel_session(
        &mut self,
        user: i64,
        session: &str,
        family: &str,
        now: i64,
    ) -> Result<()> {
        const GRANT: &str = "invalid_grant";
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| DB_ERROR)?;
        tx.execute("DELETE FROM oauth_tokens WHERE expires_at<?1", [now])
            .map_err(|_| DB_ERROR)?;
        tx.execute(
            "INSERT OR IGNORE INTO oauth_clients VALUES(?1,'[]','Brainkeep panel',strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            [PANEL_CLIENT],
        )
        .map_err(|_| DB_ERROR)?;
        let inserted = tx
            .execute(
                "INSERT INTO oauth_tokens SELECT ?1,'access',id,?2,?3,?4,0 FROM users WHERE id=?5 AND disabled=0",
                params![session, PANEL_CLIENT, family, now + PANEL_TTL, user],
            )
            .map_err(|_| GRANT)?;
        if inserted != 1 {
            return Err(GRANT.into());
        }
        tx.commit().map_err(|_| DB_ERROR.to_string())
    }
    /// User administration from the panel (admin session only, checked by the caller).
    /// Same rules as the console; invite, enable and token also issue a personal token,
    /// returned once and stored only as its SHA-256.
    fn administer(&mut self, view: &str, args: &Value) -> Result<Value> {
        const INVALID_USER: &str = "invalid email or handle";
        const NOT_ENABLED: &str = "user not found, disabled or is the admin";
        const NOT_DISABLED: &str = "user not found, not disabled or is the admin";
        const ADMIN_USER: &str = "email or handle belongs to the admin";
        const ENABLED: &str = "user already exists and is enabled";
        match view {
            "users" => {
                allowed(args, &[])?;
                let mut stmt = self
                    .db
                    .prepare(
                        &format!(
                            "SELECT u.handle,u.email,u.name,u.picture,u.admin,u.disabled,u.created_at,\
                             p.user_id IS NOT NULL,u.google_sub IS NOT NULL OR coalesce(p.used,0)=1,{HAS_DATA} \
                             FROM users u LEFT JOIN personal_tokens p ON p.user_id=u.id ORDER BY u.admin DESC,u.handle"
                        ),
                    )
                    .map_err(|_| DB_ERROR)?;
                let users = stmt
                    .query_map([], |r| {
                        Ok(json!({"handle":r.get::<_,String>(0)?,"email":r.get::<_,Option<String>>(1)?,
                            "name":r.get::<_,Option<String>>(2)?,"picture":r.get::<_,Option<String>>(3)?,
                            "admin":r.get::<_,bool>(4)?,"disabled":r.get::<_,bool>(5)?,
                            "created_at":r.get::<_,String>(6)?,"has_token":r.get::<_,bool>(7)?,
                            "signed_in":r.get::<_,bool>(8)?,"has_data":r.get::<_,bool>(9)?}))
                    })
                    .map_err(|_| DB_ERROR)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|_| DB_ERROR)?;
                Ok(json!({"users":users}))
            }
            "invite" => {
                allowed(args, &["email", "handle"])?;
                // Checked before writing: the console's invite updates an existing
                // handle in place, which must never reach the admin, and re-inviting an
                // enabled user would silently replace their token.
                let handle = text(args, "handle", 32).map_err(|_| INVALID_USER.to_string())?;
                let email = text(args, "email", 254)
                    .map_err(|_| INVALID_USER.to_string())?
                    .to_lowercase();
                let existing: Option<(bool, bool)> = self
                    .db
                    .query_row(
                        "SELECT max(admin),min(disabled) FROM users WHERE handle=?1 OR email=?2 HAVING count(*)>0",
                        params![handle, email],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(|_| DB_ERROR)?;
                match existing {
                    Some((true, _)) => return Err(ADMIN_USER.into()),
                    Some((false, false)) => {
                        let same: bool = self
                            .db
                            .query_row(
                                "SELECT count(*)=1 FROM users WHERE handle=?1 AND email=?2",
                                params![handle, email],
                                |r| r.get(0),
                            )
                            .map_err(|_| DB_ERROR)?;
                        if same {
                            return Err(ENABLED.into());
                        }
                    }
                    _ => {}
                }
                let mut result = self.users("invite", args).map_err(|e| {
                    if e == INVALID {
                        INVALID_USER.to_string()
                    } else {
                        e
                    }
                })?;
                let handle = result["handle"].as_str().unwrap_or_default().to_owned();
                result["token"] = json!(self.reissue(&handle, NOT_ENABLED)?);
                Ok(result)
            }
            "disable" => {
                allowed(args, &["handle"])?;
                self.users("disable", args)
            }
            // Only users who never stored anything: their rows go, nothing else changes.
            // Users with data are disabled instead, so history keeps its authors.
            "delete" => {
                const NOT_FOUND: &str = "user not found or is the admin";
                allowed(args, &["handle"])?;
                let handle = text(args, "handle", 32).map_err(|_| NOT_FOUND.to_string())?;
                let tx = self
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| DB_ERROR)?;
                let found: Option<(i64, bool)> = tx
                    .query_row(
                        &format!(
                            "SELECT u.id,{HAS_DATA} FROM users u WHERE u.handle=?1 AND u.admin=0"
                        ),
                        [&handle],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(|_| DB_ERROR)?;
                let Some((id, has_data)) = found else {
                    return Err(NOT_FOUND.into());
                };
                if has_data {
                    return Err(USER_HAS_DATA.into());
                }
                for table in ["oauth_tokens", "personal_tokens", "project_members"] {
                    tx.execute(&format!("DELETE FROM {table} WHERE user_id=?1"), [id])
                        .map_err(|_| DB_ERROR)?;
                }
                tx.execute("DELETE FROM users WHERE id=?1", [id])
                    .map_err(|_| DB_ERROR)?;
                tx.commit().map_err(|_| DB_ERROR)?;
                Ok(json!({"handle":handle,"deleted":true}))
            }
            "enable" => {
                allowed(args, &["handle"])?;
                let handle = text(args, "handle", 32).map_err(|_| NOT_DISABLED.to_string())?;
                let tx = self
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| DB_ERROR)?;
                let changed = tx
                    .execute(
                        "UPDATE users SET disabled=0 WHERE handle=?1 AND admin=0 AND disabled=1",
                        [&handle],
                    )
                    .map_err(|_| DB_ERROR)?;
                if changed != 1 {
                    return Err(NOT_DISABLED.into());
                }
                let token = Self::issue_personal(&tx, &handle)?;
                tx.commit().map_err(|_| DB_ERROR)?;
                Ok(json!({"handle":handle,"disabled":false,"token":token}))
            }
            "token" => {
                allowed(args, &["handle"])?;
                let handle = text(args, "handle", 32).map_err(|_| NOT_ENABLED.to_string())?;
                let token = self.reissue(&handle, NOT_ENABLED)?;
                Ok(json!({"handle":handle,"token":token}))
            }
            _ => Err("unknown tool".into()),
        }
    }
    /// New personal token for an enabled, non-admin user; the previous one stops working
    /// and so do the panel sessions opened with it.
    fn reissue(&mut self, handle: &str, missing: &str) -> Result<String> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| DB_ERROR)?;
        let found: Option<i64> = tx
            .query_row(
                "SELECT id FROM users WHERE handle=?1 AND admin=0 AND disabled=0",
                [handle],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| DB_ERROR)?;
        if found.is_none() {
            return Err(missing.into());
        }
        let token = Self::issue_personal(&tx, handle)?;
        tx.commit().map_err(|_| DB_ERROR)?;
        Ok(token)
    }
    fn issue_personal(tx: &rusqlite::Transaction, handle: &str) -> Result<String> {
        use sha2::{Digest, Sha256};
        let mut raw = [0u8; 32];
        getrandom::fill(&mut raw).map_err(|_| DB_ERROR)?;
        let token = format!(
            "{PERSONAL_PREFIX}{}",
            raw.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        let digest: String = Sha256::digest(token.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let old: Option<String> = tx
            .query_row(
                "SELECT p.hash FROM personal_tokens p JOIN users u ON u.id=p.user_id WHERE u.handle=?1",
                [handle],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| DB_ERROR)?;
        if let Some(old) = old {
            tx.execute("DELETE FROM oauth_tokens WHERE family=?1", [&old])
                .map_err(|_| DB_ERROR)?;
        }
        tx.execute(
            "INSERT INTO personal_tokens(user_id,hash,created_at,used) \
             SELECT id,?2,strftime('%Y-%m-%dT%H:%M:%fZ','now'),0 FROM users WHERE handle=?1 \
             ON CONFLICT(user_id) DO UPDATE SET hash=excluded.hash,created_at=excluded.created_at,used=0",
            params![handle, digest],
        )
        .map_err(|_| DB_ERROR)?;
        Ok(token)
    }
    fn recall(&self, project: &str, id: &str) -> Result<Value> {
        self.db
            .query_row(
                &format!(
                    "{SELECT} WHERE m.project=?1 AND m.id=?2 AND r.revision=m.current_revision AND m.archived_at IS NULL"
                ),
                params![project, id],
                row,
            )
            .optional()
            .map_err(|_| DB_ERROR)?
            .ok_or_else(|| "memory not found in this project".into())
    }
    fn write(
        &mut self,
        name: &str,
        args: Value,
        typed: &Value,
        compact: bool,
        access: &Access,
        me: &str,
    ) -> Result<Value> {
        let mut fields = vec![
            "project",
            "operation_id",
            "content",
            "source",
            "agent",
            "confirmed",
            "secret",
            "title",
            "kind",
            "topic",
        ];
        if name == "correct" {
            fields.extend(["id", "expected_revision"]);
        }
        allowed(&args, &fields)?;
        let project = text(&args, "project", PROJECT_KEY_MAX)?;
        let op = text(&args, "operation_id", 128)?;
        // Remote receipts live in a per-principal namespace: another credential can neither
        // detect nor pre-claim an operation_id. Local ids keep the legacy global form.
        let legacy_op = op.clone();
        let op = if access.is_local() {
            op
        } else {
            format!("{}\u{0}{op}", access.principal())
        };
        let content = text(&args, "content", 16384)?;
        let source = text(&args, "source", 1024)?;
        let agent = text(&args, "agent", 128)?;
        let confirmed = optional_bool(&args, "confirmed")?;
        let mut id = if name == "correct" {
            text(&args, "id", 128)?
        } else {
            Uuid::new_v4().to_string()
        };
        let expected = if name == "correct" {
            args["expected_revision"]
                .as_i64()
                .filter(|n| *n > 0 && *n < i64::MAX)
                .ok_or(INVALID)?
        } else {
            0
        };
        // Canonicalize optional defaults before checking retry equality.
        let mut normalized = typed.clone();
        normalized.as_object_mut().ok_or(INVALID)?.remove("secret");
        normalized["confirmed"] = json!(confirmed);
        // Remote receipts are bound to their principal so another credential cannot
        // replay them; local receipts keep the pre-v3 form for legacy retries.
        let mut request = json!({"tool":name,"arguments":normalized});
        if !access.is_local() {
            request["principal"] = json!(access.principal());
        }
        let request = request.to_string();
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| DB_ERROR)?;
        let receipt = |key: &str| -> Result<Option<(String, String)>> {
            tx.query_row(
                "SELECT request,response FROM operations WHERE operation_id=?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| DB_ERROR.into())
        };
        let prior = receipt(&op)?;
        let legacy_request = json!({"tool":name,"arguments":normalized}).to_string();
        // A retry does not bring back the content of a memory archived since.
        let replay = |response: &str| -> Result<Value> {
            let response: Value = serde_json::from_str(response).map_err(|_| DB_ERROR)?;
            let archived: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM memories WHERE id=?1 AND archived_at IS NOT NULL",
                    [response["id"].as_str().unwrap_or_default()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|_| DB_ERROR)?;
            if archived.is_some() {
                return Err("memory not found in this project".into());
            }
            Ok(response)
        };
        // A remote retry that began before v3 may hold a global receipt. Accept it only
        // on an exact argument match, ignoring mismatches so it never becomes an oracle.
        if prior.is_none()
            && !access.is_local()
            && let Some((original, response)) = receipt(&legacy_op)?
        {
            if original == legacy_request {
                return replay(&response);
            }
            // A purged memory's global receipt refuses the retry instead of recreating it.
            if original == PURGED_RECEIPT {
                return Err("operation_id already used with different arguments".into());
            }
        }
        if let Some((original, response)) = prior {
            if original != request {
                return Err("operation_id already used with different arguments".into());
            }
            return replay(&response);
        }
        let existing = if name == "correct" {
            tx.query_row(
                &format!(
                    "{SELECT} WHERE m.id=?1 AND m.project=?2 AND r.revision=m.current_revision AND m.archived_at IS NULL"
                ),
                params![id, project],
                row,
            )
            .optional()
            .map_err(|_| DB_ERROR)?
        } else if args.get("topic").is_some() {
            let topic = text(&args, "topic", 128)?;
            let archived: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM topics t JOIN memories m ON m.id=t.id \
                     WHERE t.project=?1 AND t.topic=?2 AND m.archived_at IS NOT NULL",
                    params![project, topic],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|_| DB_ERROR)?;
            if archived.is_some() {
                return Err(TOPIC_ARCHIVED.into());
            }
            tx.query_row(
                &format!(
                    "{SELECT} WHERE t.project=?1 AND t.topic=?2 AND r.revision=m.current_revision"
                ),
                params![project, topic],
                row,
            )
            .optional()
            .map_err(|_| DB_ERROR)?
        } else {
            None
        };
        if name == "correct"
            && existing.as_ref().and_then(|v| v["revision"].as_i64()) != Some(expected)
        {
            return Err(REVISION_CONFLICT.into());
        }
        let title = match args.get("title") {
            Some(_) => text(&args, "title", 160)?,
            None => existing
                .as_ref()
                .and_then(|v| v["title"].as_str())
                .unwrap_or("")
                .to_owned(),
        };
        let kind = match args.get("kind") {
            Some(_) => text(&args, "kind", 16)?,
            None => existing
                .as_ref()
                .and_then(|v| v["kind"].as_str())
                .unwrap_or("note")
                .to_owned(),
        };
        if !["note", "decision", "preference", "discovery", "progress"].contains(&kind.as_str()) {
            return Err(INVALID.into());
        }
        let topic = match args.get("topic") {
            Some(_) => Some(text(&args, "topic", 128)?),
            None => existing
                .as_ref()
                .and_then(|v| v["topic"].as_str())
                .map(str::to_owned),
        };
        if let Some(ref old) = existing {
            id = old["id"].as_str().ok_or(DB_ERROR)?.to_owned();
            if old["topic"].as_str().is_some() && old["topic"].as_str() != topic.as_deref() {
                return Err("topic is stable; create a separate memory for a new subject".into());
            }
        }
        // Same content/state/metadata is a no-op even when a different agent repeats it.
        // Preserve the original source; changing provenance alone can be meaningful and
        // uses correct, while remember by topic cannot silently overwrite it.
        let deduplicate =
            compact || topic.is_some() || args.get("title").is_some() || args.get("kind").is_some();
        let unchanged = deduplicate
            && existing.as_ref().is_some_and(|old| {
                old["content"] == content
                    && old["title"] == title
                    && old["kind"] == kind
                    && old["topic"].as_str() == topic.as_deref()
                    && (old["state"] == "confirmed") == confirmed
                    && (name == "remember" || (old["source"] == source && old["agent"] == agent))
            });
        if existing.is_some() && name == "remember" && !unchanged {
            return Err("topic already exists; search by topic then recall and correct with expected_revision".into());
        }
        let saved_at: String = tx
            .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
                r.get(0)
            })
            .map_err(|_| DB_ERROR)?;
        let response = if unchanged {
            existing.ok_or(DB_ERROR)?
        } else {
            let revision = expected + 1;
            if name == "remember" {
                tx.execute(
                    "INSERT INTO memories(id,project,current_revision,created_at) VALUES (?1,?2,1,?3)",
                    params![id, project, saved_at],
                )
                .map_err(|_| DB_ERROR)?;
            } else {
                tx.execute("UPDATE memories SET current_revision=?1 WHERE id=?2 AND project=?3 AND current_revision=?4",params![revision,id,project,expected]).map_err(|_|DB_ERROR)?;
            }
            tx.execute(
                "INSERT INTO revisions(id,revision,content,source,agent,confirmed,saved_at,principal) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![id, revision, content, source, agent, confirmed, saved_at, me],
            )
            .map_err(|_| DB_ERROR)?;
            tx.execute(
                "INSERT INTO revision_metadata VALUES (?1,?2,?3,?4)",
                params![id, revision, title, kind],
            )
            .map_err(|_| DB_ERROR)?;
            if let Some(ref topic) = topic {
                tx.execute("INSERT INTO topics(project,topic,id) VALUES (?1,?2,?3) ON CONFLICT(id) DO NOTHING",params![project,topic,id])
                    .map_err(|_| "topic already exists; recall before correcting")?;
            }
            tx.execute(
                "DELETE FROM memory_fts WHERE rowid=(SELECT rowid FROM memories WHERE id=?1)",
                [&id],
            )
            .map_err(|_| DB_ERROR)?;
            let indexed = if title.is_empty() {
                content.clone()
            } else {
                format!("{title} {content}")
            };
            tx.execute(
                "INSERT INTO memory_fts(rowid,id,project,content) VALUES ((SELECT rowid FROM memories WHERE id=?1),?1,?2,?3)",
                params![id, project, indexed],
            )
            .map_err(|_| DB_ERROR)?;
            json!({"id":id,"project":project,"revision":revision,"content":content,"source":source,"agent":agent,"saved_at":saved_at,"state":if confirmed {"confirmed"} else {"proposed"},"source_verified":false,"title":title,"kind":kind,"topic":topic,"principal":me})
        };
        tx.execute(
            "INSERT INTO operations VALUES (?1,?2,?3,?4)",
            params![op, request, response.to_string(), saved_at],
        )
        .map_err(|_| DB_ERROR)?;
        tx.commit().map_err(|_| DB_ERROR)?;
        self.changed.push((project.to_string(), false));
        Ok(response)
    }
    /// Projects whose memories or members changed since the previous call.
    pub fn take_changes(&mut self) -> Vec<(String, bool)> {
        let mut changed = std::mem::take(&mut self.changed);
        changed.sort();
        changed.dedup();
        changed
    }
    fn history(&self, args: &Value) -> Result<Value> {
        allowed(args, &["project", "id", "after_revision", "limit"])?;
        let project = text(args, "project", PROJECT_KEY_MAX)?;
        let id = text(args, "id", 128)?;
        self.recall(&project, &id)?;
        let after = number(args, "after_revision", 0, 0, i64::MAX)?;
        let limit = number(args, "limit", 10, 1, 20)?;
        let sql = format!(
            "{SELECT} WHERE m.project=?1 AND m.id=?2 AND r.revision>?3 ORDER BY r.revision LIMIT ?4"
        );
        let mut stmt = self.db.prepare(&sql).map_err(|_| DB_ERROR)?;
        let mut items = stmt
            .query_map(params![project, id, after, limit + 1], row)
            .map_err(|_| DB_ERROR)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| DB_ERROR)?;
        let more = items.len() > limit as usize;
        items.truncate(limit as usize);
        let next = if more {
            items.last().map(|v| v["revision"].clone())
        } else {
            None
        };
        Ok(json!({"items":items,"next_after_revision":next,"trust":"data_only"}))
    }
    fn search(&self, name: &str, args: &Value) -> Result<Value> {
        let mut fields = vec!["project", "limit", "kind", "topic", "offset", "snapshot"];
        if name == "search" {
            fields.push("query");
        }
        allowed(args, &fields)?;
        let project = text(args, "project", PROJECT_KEY_MAX)?;
        let limit = number(args, "limit", 5, 1, 20)?;
        let offset = number(args, "offset", 0, 0, 1_000_000)?;
        // Per-project version: revisions are append-only, so their count changes with every
        // effective write here and reveals nothing about other projects.
        let snapshot: i64 = self
            .db
            .query_row(
                "SELECT (SELECT count(*) FROM revisions r JOIN memories m ON m.id=r.id WHERE m.project=?1) \
                 + COALESCE((SELECT changes FROM project_changes WHERE project=?1),0)",
                [&project],
                |r| r.get(0),
            )
            .map_err(|_| DB_ERROR)?;
        if offset > 0 && args.get("snapshot").is_none() {
            return Err("continuation requires snapshot; restart listing".into());
        }
        if args.get("snapshot").is_some() && number(args, "snapshot", 0, 0, i64::MAX)? != snapshot {
            return Err("memory changed; restart listing without offset and snapshot".into());
        }
        let kind = if args.get("kind").is_some() {
            Some(text(args, "kind", 16)?)
        } else {
            None
        };
        if kind.as_ref().is_some_and(|k| {
            !["note", "decision", "preference", "discovery", "progress"].contains(&k.as_str())
        }) {
            return Err(INVALID.into());
        }
        let topic = if args.get("topic").is_some() {
            Some(text(args, "topic", 128)?)
        } else {
            None
        };
        let query = if name == "search" {
            Some(
                text(args, "query", 512)?
                    .split_whitespace()
                    .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
                    .collect::<Vec<_>>()
                    .join(" AND "),
            )
        } else {
            None
        };
        let (join, filter, order) = if query.is_some() {
            (
                " JOIN memory_fts f ON f.id=m.id",
                " AND memory_fts MATCH ?6",
                "rank,m.id",
            )
        } else {
            ("", "", "r.saved_at DESC,m.id")
        };
        let sql = format!(
            "{SELECT}{join} WHERE m.project=?1 AND r.revision=m.current_revision AND m.archived_at IS NULL AND (?2 IS NULL OR a.kind=?2) AND (?3 IS NULL OR t.topic=?3){filter} ORDER BY {order} LIMIT ?4 OFFSET ?5"
        );
        let mut stmt = self.db.prepare(&sql).map_err(|_| DB_ERROR)?;
        let mut items = if let Some(query) = query {
            stmt.query_map(params![project, kind, topic, limit + 1, offset, query], row)
        } else {
            stmt.query_map(params![project, kind, topic, limit + 1, offset], row)
        }
        .map_err(|_| DB_ERROR)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| DB_ERROR)?;
        let more = items.len() > limit as usize;
        items.truncate(limit as usize);
        for item in &mut items {
            let full = item["content"].as_str().unwrap_or("");
            let preview: String = full.chars().take(512).collect();
            let truncated = preview.len() < full.len();
            item["content"] = json!(preview);
            item["truncated"] = json!(truncated);
        }
        Ok(
            json!({"items":items,"more_available":more,"trust":"data_only","expand_with":"recall","snapshot":snapshot}),
        )
    }
    pub fn backup(&self, destination: &Path) -> Result<()> {
        let file = files::new_file(destination)?;
        let result: Result<()> = (|| {
            let mut dst = Connection::open(destination).map_err(|_| DB_ERROR)?;
            let backup = rusqlite::backup::Backup::new(&self.db, &mut dst).map_err(|_| DB_ERROR)?;
            backup
                .run_to_completion(128, Duration::from_millis(5), None)
                .map_err(|_| DB_ERROR)?;
            drop(backup);
            validate_database(&dst)?;
            drop(dst);
            file.sync_all().map_err(|_| DB_ERROR)?;
            sync_parent(destination)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(destination);
        }
        result
    }
    pub fn restore(source: &Path, destination: &Path) -> Result<()> {
        files::check_file(source)?;
        let src = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "invalid backup")?;
        src.execute_batch("PRAGMA trusted_schema=OFF")
            .map_err(|_| "invalid backup")?;
        validate_database(&src)?;
        // Reserve a new directory atomically; never overwrite an existing installation.
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(destination)
            .map_err(|_| "restore destination must be a new directory with an existing parent")?;
        let result: Result<()> = (|| {
            let file = files::new_file(&destination.join(DATABASE))?;
            let mut dst = Connection::open(destination.join(DATABASE)).map_err(|_| DB_ERROR)?;
            let backup = rusqlite::backup::Backup::new(&src, &mut dst).map_err(|_| DB_ERROR)?;
            backup
                .run_to_completion(128, Duration::from_millis(5), None)
                .map_err(|_| DB_ERROR)?;
            drop(backup);
            validate_database(&dst)?;
            drop(dst);
            file.sync_all().map_err(|_| DB_ERROR)?;
            sync_parent(&destination.join(DATABASE))?;
            sync_parent(destination)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(destination);
        }
        result
    }
}
/// Consistent pre-migration copy; never overwrites an earlier snapshot.
fn snapshot(db: &Connection, dir: &Path, target: i64) -> Result<()> {
    let mut path = dir.join(format!("pre-v{target}.sqlite"));
    if std::fs::symlink_metadata(&path).is_ok() {
        path = dir.join(format!("pre-v{target}-{}.sqlite", Uuid::new_v4()));
    }
    let file = files::new_file(&path)?;
    let mut dst = Connection::open(&path).map_err(|_| DB_ERROR)?;
    let backup = rusqlite::backup::Backup::new(db, &mut dst).map_err(|_| DB_ERROR)?;
    backup
        .run_to_completion(128, Duration::from_millis(5), None)
        .map_err(|_| DB_ERROR)?;
    drop(backup);
    validate_database(&dst)?;
    drop(dst);
    file.sync_all().map_err(|_| DB_ERROR)?;
    sync_parent(&path)
}
fn sync_parent(path: &Path) -> Result<()> {
    File::open(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )
    .and_then(|f| f.sync_all())
    .map_err(|_| DB_ERROR.into())
}
pub(crate) fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(
        json!({"id":r.get::<_,String>(0)?,"project":r.get::<_,String>(1)?,"revision":r.get::<_,i64>(2)?,"content":r.get::<_,String>(3)?,"source":r.get::<_,String>(4)?,"agent":r.get::<_,String>(5)?,"saved_at":r.get::<_,String>(6)?,"state":r.get::<_,String>(7)?,"source_verified":false,"title":r.get::<_,String>(8)?,"kind":r.get::<_,String>(9)?,"topic":r.get::<_,Option<String>>(10)?,"principal":r.get::<_,Option<String>>(11)?}),
    )
}
/// Zero-width and bidirectional formatting characters (Unicode Cf) that would
/// change how a display name looks.
fn invisible(c: char) -> bool {
    matches!(c, '\u{AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}
/// Google profile photo: https on Google's image host only, URL-safe characters, bounded.
/// Mirrors the `users.picture` CHECK and the panel CSP `img-src`.
fn valid_picture(url: &str) -> bool {
    url.len() <= 2048
        && url.starts_with("https://lh3.googleusercontent.com/")
        && url
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._~:/?#@!$&()*+,;=%-".contains(&b))
}
fn schema_version(db: &Connection) -> Result<i64> {
    db.query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|_| "invalid database".into())
}
fn validate_database(db: &Connection) -> Result<()> {
    // Integrity comparisons sort the corpus, including on backup/restore connections.
    // Explicitly allow disk spill on every connection before validation allocates pages.
    db.execute_batch("PRAGMA temp_store=FILE")
        .map_err(|_| "cannot configure database validation")?;
    let version = schema_version(db)?;
    if !(1..=SCHEMA_VERSION).contains(&version) {
        return Err("invalid or unsupported Brainkeep database".into());
    }
    // SQLite's integrity_check does not detect missing application tables or
    // changed constraints. Compare against the complete supported schema before
    // accepting a database or creating a restore destination.
    let expected = Connection::open_in_memory().map_err(|_| "cannot validate schema")?;
    expected
        .execute_batch(include_str!("schema.sql"))
        .map_err(|_| "cannot validate schema")?;
    if version >= 2 {
        expected
            .execute_batch(include_str!("migration_v2.sql"))
            .map_err(|_| "cannot validate schema")?;
        if version >= 3 {
            expected
                .execute_batch(include_str!("migration_v3.sql"))
                .map_err(|_| "cannot validate schema")?;
        }
        if version >= 4 {
            expected
                .execute_batch(include_str!("migration_v4.sql"))
                .map_err(|_| "cannot validate schema")?;
            if version >= 5 {
                expected
                    .execute_batch(include_str!("migration_v5.sql"))
                    .map_err(|_| "cannot validate schema")?;
            }
            if version >= 6 {
                expected
                    .execute_batch(include_str!("migration_v6.sql"))
                    .map_err(|_| "cannot validate schema")?;
            }
            if version >= 7 {
                expected
                    .execute_batch(include_str!("migration_v7.sql"))
                    .map_err(|_| "cannot validate schema")?;
            }
            // Exactly one admin, and every stored project key belongs to a user.
            let admins: i64 = db
                .query_row("SELECT count(*) FROM users WHERE admin=1", [], |r| r.get(0))
                .map_err(|_| "invalid database")?;
            let archive_tables = if version >= 6 {
                " UNION SELECT project FROM project_changes UNION SELECT project FROM purges"
            } else {
                ""
            };
            let orphans: i64 = db
                .query_row(
                    &format!("SELECT count(*) FROM (SELECT project FROM memories UNION SELECT project FROM topics UNION SELECT project FROM memory_fts UNION SELECT project FROM project_members{archive_tables}) p \
                     WHERE NOT EXISTS (SELECT 1 FROM users u WHERE substr(p.project,1,length(u.handle)+1)=u.handle||'/' AND length(p.project)>length(u.handle)+1)"),
                    [],
                    |r| r.get(0),
                )
                .map_err(|_| "invalid database")?;
            if admins != 1 || orphans != 0 {
                return Err("inconsistent Brainkeep ownership".into());
            }
        }
        let missing:i64=db.query_row("SELECT count(*) FROM revisions r LEFT JOIN revision_metadata a ON r.id=a.id AND r.revision=a.revision WHERE a.id IS NULL",[],|r|r.get(0)).map_err(|_| "invalid database")?;
        let bad_topic:i64=db.query_row("SELECT count(*) FROM topics t JOIN memories m ON m.id=t.id WHERE t.project!=m.project",[],|r|r.get(0)).map_err(|_| "invalid database")?;
        let bad_fts:i64=db.query_row("SELECT count(*) FROM memory_fts f LEFT JOIN memories m ON m.rowid=f.rowid WHERE m.id IS NULL OR m.id!=f.id",[],|r|r.get(0)).map_err(|_|"invalid database")?;
        if missing != 0 || bad_topic != 0 || bad_fts != 0 {
            return Err("inconsistent Brainkeep metadata".into());
        }
    }
    if schema_objects(db)? != schema_objects(&expected)? {
        return Err("invalid or unsupported Brainkeep schema".into());
    }
    let id: i64 = db
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .map_err(|_| "invalid backup")?;
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|_| "invalid backup")?;
    let integrity: String = db
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|_| "invalid backup")?;
    if id != APP_ID || !(1..=SCHEMA_VERSION).contains(&version) || integrity != "ok" {
        return Err("invalid or unsupported Brainkeep database".into());
    }
    let foreign: Option<i64> = db
        .query_row("SELECT 1 FROM pragma_foreign_key_check LIMIT 1", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|_| "invalid database")?;
    let dangling:i64=db.query_row("SELECT count(*) FROM memories m LEFT JOIN revisions r ON r.id=m.id AND r.revision=m.current_revision WHERE r.id IS NULL",[],|r|r.get(0)).map_err(|_|"invalid database")?;
    let unexpected: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type IN ('trigger','view')",
            [],
            |r| r.get(0),
        )
        .map_err(|_| "invalid database")?;
    let indexed = if version == 1 {
        "SELECT m.id,m.project,r.content FROM memories m JOIN revisions r ON r.id=m.id AND r.revision=m.current_revision"
    } else {
        "SELECT m.id,m.project,CASE WHEN a.title='' THEN r.content ELSE a.title||' '||r.content END FROM memories m JOIN revisions r ON r.id=m.id AND r.revision=m.current_revision JOIN revision_metadata a ON a.id=r.id AND a.revision=r.revision"
    };
    let mismatch: i64 = db
        .query_row(
            &format!(
                "SELECT count(*) FROM ({indexed} EXCEPT SELECT id,project,content FROM memory_fts)"
            ),
            [],
            |r| r.get(0),
        )
        .map_err(|_| "invalid database")?;
    let extra: i64 = db
        .query_row(
            &format!(
                "SELECT count(*) FROM (SELECT id,project,content FROM memory_fts EXCEPT {indexed})"
            ),
            [],
            |r| r.get(0),
        )
        .map_err(|_| "invalid database")?;
    if foreign.is_some() || dangling != 0 || unexpected != 0 || mismatch != 0 || extra != 0 {
        return Err("inconsistent Brainkeep database".into());
    }
    Ok(())
}

type SchemaObject = (String, String, String, Option<String>);

fn schema_objects(db: &Connection) -> Result<Vec<SchemaObject>> {
    let mut stmt = db.prepare(
        "SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name"
    ).map_err(|_| "invalid database schema")?;
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(|_| "invalid database schema")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| "invalid database schema".into())
}
fn validate_arguments(v: &Value) -> Result<()> {
    if !v.is_object() || v.to_string().len() > 32768 {
        return Err(INVALID.into());
    }
    if optional_bool(v, "secret")? {
        return Err("secret rejected; nothing saved".into());
    }
    fn scan(v: &Value) -> bool {
        match v {
            Value::String(s) => security::sensitive(s),
            Value::Array(a) => a.iter().any(scan),
            Value::Object(o) => o.iter().any(|(k, v)| security::sensitive(k) || scan(v)),
            _ => false,
        }
    }
    if scan(v) {
        return Err("secret rejected; nothing saved".into());
    }
    Ok(())
}
fn allowed(args: &Value, names: &[&str]) -> Result<()> {
    if args
        .as_object()
        .ok_or(INVALID)?
        .keys()
        .any(|k| !names.contains(&k.as_str()))
    {
        return Err(INVALID.into());
    }
    Ok(())
}
fn text(v: &Value, key: &str, max: usize) -> Result<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= max && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or(INVALID.into())
}
fn optional_bool(v: &Value, key: &str) -> Result<bool> {
    match v.get(key) {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(INVALID.into()),
    }
}
fn number(v: &Value, key: &str, default: i64, min: i64, max: i64) -> Result<i64> {
    match v.get(key) {
        None => Ok(default),
        Some(n) => n
            .as_i64()
            .filter(|n| *n >= min && *n <= max)
            .ok_or(INVALID.into()),
    }
}
