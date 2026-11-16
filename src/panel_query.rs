//! Read-only queries for the web panel (archived memories only in the archived view). Callers pass untrusted string parameters;
//! errors never echo them. Error prefixes map to HTTP statuses in the panel layer.
use crate::store::{SELECT, row};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value as Sql};
use serde_json::{Map, Value, json};
use sha2::Digest;

pub const INVALID: &str = "invalid parameters";
pub const NOT_FOUND: &str = "not found";
pub const CHANGED: &str = "memory changed; restart listing";
const DB_ERROR: &str = "storage failure";
/// Revisions per history page: 50 x 16 KiB stays well under the 4 MiB response cap.
const HISTORY_PAGE: usize = 50;
/// SQL condition on `m.project` for the visible set bound as `?{n}`.
fn inside(n: usize) -> String {
    format!("m.project IN (SELECT value FROM json_each(?{n}))")
}
const KINDS: [&str; 5] = ["note", "decision", "preference", "discovery", "progress"];
type Result<T> = std::result::Result<T, String>;

/// `visible` is a JSON array of the internal project keys the session may read; every
/// query is confined to it so other users' projects look exactly like missing ones.
pub fn run(db: &Connection, view: &str, args: &Value, visible: &str) -> Result<Value> {
    let args = args.as_object().ok_or(INVALID)?;
    if args.values().any(|v| !v.is_string()) {
        return Err(INVALID.into());
    }
    match view {
        "status" => status(db, args, visible),
        "projects" => projects(db, args, visible),
        "facets" => facets(db, args, visible),
        "memories" => memories(db, args, visible),
        "memory" => memory(db, args, visible),
        "activity" => activity(db, args, visible),
        _ => Err(INVALID.into()),
    }
}

fn allowed(args: &Map<String, Value>, names: &[&str]) -> Result<()> {
    if args.keys().any(|k| !names.contains(&k.as_str())) {
        return Err(INVALID.into());
    }
    Ok(())
}
/// Optional bounded text; empty means absent.
fn opt(args: &Map<String, Value>, key: &str, max: usize) -> Result<Option<String>> {
    match args.get(key).and_then(Value::as_str).map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) if s.len() <= max && !s.contains('\0') => Ok(Some(s.to_owned())),
        _ => Err(INVALID.into()),
    }
}
fn req(args: &Map<String, Value>, key: &str, max: usize) -> Result<String> {
    opt(args, key, max)?.ok_or_else(|| INVALID.into())
}
fn int(args: &Map<String, Value>, key: &str, default: i64, min: i64, max: i64) -> Result<i64> {
    match opt(args, key, 20)? {
        None => Ok(default),
        Some(s) => s
            .parse::<i64>()
            .ok()
            .filter(|n| *n >= min && *n <= max)
            .ok_or_else(|| INVALID.into()),
    }
}
fn date(db: &Connection, args: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    let Some(value) = opt(args, key, 10)? else {
        return Ok(None);
    };
    let valid: bool = db
        .query_row("SELECT date(?1) IS ?1", [&value], |r| r.get(0))
        .map_err(|_| DB_ERROR)?;
    if !valid {
        return Err(INVALID.into());
    }
    Ok(Some(value))
}
/// Opaque listing version: changes with every write or share change the session can
/// see. It is a hash, so it never reveals how much other users wrote in between.
fn snapshot(db: &Connection, visible: &str) -> Result<i64> {
    let (last, count): (i64, i64) = db
        .query_row(
            &format!(
                "SELECT COALESCE(max(r.rowid),0),count(*) FROM revisions r JOIN memories m ON m.id=r.id WHERE {}",
                inside(1)
            ),
            [visible],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| DB_ERROR)?;
    // Archive, restore and purge add no revision; their per-project counter does.
    let changes: i64 = db
        .query_row(
            "SELECT COALESCE(sum(changes),0) FROM project_changes WHERE project IN (SELECT value FROM json_each(?1))",
            [visible],
            |r| r.get(0),
        )
        .map_err(|_| DB_ERROR)?;
    let digest = sha2::Sha256::new()
        .chain_update(visible.as_bytes())
        .chain_update(last.to_le_bytes())
        .chain_update(count.to_le_bytes())
        .chain_update(changes.to_le_bytes())
        .finalize();
    // 48 bits: exact as a JavaScript number.
    Ok(digest[..6]
        .iter()
        .fold(0i64, |n, b| (n << 8) | i64::from(*b)))
}
fn strings(
    db: &Connection,
    sql: &str,
    project: &Option<String>,
    visible: &str,
) -> Result<Vec<String>> {
    let mut stmt = db.prepare(sql).map_err(|_| DB_ERROR)?;
    stmt.query_map(params![project, visible], |r| r.get(0))
        .map_err(|_| DB_ERROR)?
        .collect::<rusqlite::Result<Vec<String>>>()
        .map_err(|_| DB_ERROR.into())
}

fn status(db: &Connection, args: &Map<String, Value>, visible: &str) -> Result<Value> {
    allowed(args, &[])?;
    let (memories, projects, archived): (i64, i64, i64) = db
        .query_row(
            &format!(
                "SELECT count(*) FILTER (WHERE m.archived_at IS NULL),\
                 count(DISTINCT m.project) FILTER (WHERE m.archived_at IS NULL),\
                 count(*) FILTER (WHERE m.archived_at IS NOT NULL) FROM memories m WHERE {}",
                inside(1)
            ),
            [visible],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| DB_ERROR)?;
    let last: Option<String> = db
        .query_row(
            &format!(
                "SELECT max(r.saved_at) FROM revisions r JOIN memories m ON m.id=r.id WHERE m.archived_at IS NULL AND {}",
                inside(1)
            ),
            [visible],
            |r| r.get(0),
        )
        .map_err(|_| DB_ERROR)?;
    Ok(
        json!({"memories":memories,"projects":projects,"archived":archived,"last_save":last,
        "schema_version":7,"version":env!("CARGO_PKG_VERSION")}),
    )
}

fn projects(db: &Connection, args: &Map<String, Value>, visible: &str) -> Result<Value> {
    allowed(args, &[])?;
    let mut stmt = db
        .prepare(&format!(
            "SELECT m.project,a.kind,count(*),max(r.saved_at) FROM memories m \
             JOIN revisions r ON r.id=m.id AND r.revision=m.current_revision \
             JOIN revision_metadata a ON a.id=r.id AND a.revision=r.revision \
             WHERE m.archived_at IS NULL AND {} GROUP BY m.project,a.kind ORDER BY m.project LIMIT 5000",
            inside(1)
        ))
        .map_err(|_| DB_ERROR)?;
    let rows = stmt
        .query_map([visible], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|_| DB_ERROR)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| DB_ERROR)?;
    let mut items: Vec<Value> = Vec::new();
    for (project, kind, count, last) in rows {
        if items.last().and_then(|v| v["project"].as_str()) != Some(project.as_str()) {
            let topics: i64 = db
                .query_row(
                    "SELECT count(*) FROM topics t JOIN memories m ON m.id=t.id WHERE t.project=?1 AND m.archived_at IS NULL",
                    [&project],
                    |r| r.get(0),
                )
                .map_err(|_| DB_ERROR)?;
            items.push(json!({"project":project,"total":0,"kinds":{},"last_saved_at":last,"topics":topics}));
        }
        let item = items.last_mut().ok_or(DB_ERROR)?;
        item["total"] = json!(item["total"].as_i64().unwrap_or(0) + count);
        item["kinds"][kind] = json!(count);
        if item["last_saved_at"].as_str() < Some(last.as_str()) {
            item["last_saved_at"] = json!(last);
        }
    }
    Ok(json!({"items":items}))
}

fn facets(db: &Connection, args: &Map<String, Value>, visible: &str) -> Result<Value> {
    allowed(args, &["project", "archived"])?;
    let project = opt(args, "project", 161)?;
    // Filters of the archived view offer archived values; every other view, active ones.
    let archived = match opt(args, "archived", 1)?.as_deref() {
        None | Some("0") => "IS NULL",
        Some("1") => "IS NOT NULL",
        _ => return Err(INVALID.into()),
    };
    let agents = strings(
        db,
        &format!(
            "SELECT DISTINCT r.agent FROM memories m JOIN revisions r ON r.id=m.id AND r.revision=m.current_revision \
         WHERE m.archived_at {archived} AND (?1 IS NULL OR m.project=?1) AND m.project IN (SELECT value FROM json_each(?2)) ORDER BY 1 LIMIT 200"
        ),
        &project,
        visible,
    )?;
    let principals = strings(
        db,
        &format!(
            "SELECT DISTINCT r.principal FROM memories m JOIN revisions r ON r.id=m.id AND r.revision=m.current_revision \
         WHERE r.principal IS NOT NULL AND m.archived_at {archived} AND (?1 IS NULL OR m.project=?1) \
         AND m.project IN (SELECT value FROM json_each(?2)) ORDER BY 1 LIMIT 200"
        ),
        &project,
        visible,
    )?;
    let topics = strings(
        db,
        &format!(
            "SELECT DISTINCT t.topic FROM topics t JOIN memories m ON m.id=t.id \
         WHERE m.archived_at {archived} AND (?1 IS NULL OR t.project=?1) \
         AND t.project IN (SELECT value FROM json_each(?2)) ORDER BY 1 LIMIT 500"
        ),
        &project,
        visible,
    )?;
    let projects = strings(
        db,
        &format!(
            "SELECT DISTINCT project FROM memories WHERE archived_at {archived} AND (?1 IS NULL OR project=?1) \
         AND project IN (SELECT value FROM json_each(?2)) ORDER BY 1 LIMIT 500"
        ),
        &project,
        visible,
    )?;
    Ok(json!({"agents":agents,"principals":principals,"topics":topics,"projects":projects}))
}

/// Agents that saved revisions since `since` (the client's local midnight, as
/// `YYYY-MM-DDTHH:MM:SS.sssZ`, at most 7 days back; default: the last 24 hours) in
/// visible, active memories. Panel edits are people, not agents.
fn activity(db: &Connection, args: &Map<String, Value>, visible: &str) -> Result<Value> {
    allowed(args, &["since"])?;
    let since: String = match opt(args, "since", 24)? {
        None => db
            .query_row(
                "SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now','-1 day')",
                [],
                |r| r.get(0),
            )
            .map_err(|_| DB_ERROR)?,
        Some(s) => {
            let valid: bool = db
                .query_row(
                    "SELECT strftime('%Y-%m-%dT%H:%M:%fZ',?1) IS ?1 \
                     AND ?1 >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-7 days')",
                    [&s],
                    |r| r.get(0),
                )
                .map_err(|_| DB_ERROR)?;
            if !valid {
                return Err(INVALID.into());
            }
            s
        }
    };
    let mut stmt = db
        .prepare(&format!(
            "SELECT r.agent,count(*),max(r.saved_at) FROM revisions r JOIN memories m ON m.id=r.id \
             WHERE m.archived_at IS NULL AND r.saved_at>=?2 AND r.agent!='panel' AND {} \
             GROUP BY r.agent ORDER BY count(*) DESC,r.agent LIMIT 50",
            inside(1)
        ))
        .map_err(|_| DB_ERROR)?;
    let agents = stmt
        .query_map(params![visible, since], |r| {
            Ok(json!({"agent":r.get::<_, String>(0)?,"revisions":r.get::<_, i64>(1)?,"last_saved_at":r.get::<_, String>(2)?}))
        })
        .map_err(|_| DB_ERROR)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| DB_ERROR)?;
    Ok(json!({"since":since,"agents":agents}))
}

fn memories(db: &Connection, args: &Map<String, Value>, visible: &str) -> Result<Value> {
    allowed(
        args,
        &[
            "q",
            "project",
            "kind",
            "agent",
            "principal",
            "state",
            "topic",
            "from",
            "to",
            "limit",
            "offset",
            "snapshot",
            "archived",
        ],
    )?;
    // The archived view lists only archived memories; every other view, only active ones.
    let archived = match opt(args, "archived", 1)?.as_deref() {
        None | Some("0") => false,
        Some("1") => true,
        _ => return Err(INVALID.into()),
    };
    let query = opt(args, "q", 512)?;
    let project = opt(args, "project", 161)?;
    let agent = opt(args, "agent", 128)?;
    let principal = opt(args, "principal", 128)?;
    let topic = opt(args, "topic", 128)?;
    let state = opt(args, "state", 16)?;
    if state
        .as_deref()
        .is_some_and(|s| s != "confirmed" && s != "proposed")
    {
        return Err(INVALID.into());
    }
    let kinds = match opt(args, "kind", 128)? {
        None => None,
        Some(list) => {
            let kinds: Vec<&str> = list.split(',').map(str::trim).collect();
            if kinds.iter().any(|k| !KINDS.contains(k)) {
                return Err(INVALID.into());
            }
            Some(json!(kinds).to_string())
        }
    };
    let from = date(db, args, "from")?;
    let to = date(db, args, "to")?;
    let limit = int(args, "limit", 20, 1, 50)?;
    let offset = int(args, "offset", 0, 0, 10_000)?;
    let current = snapshot(db, visible)?;
    let requested = match opt(args, "snapshot", 20)? {
        None if offset > 0 => return Err(INVALID.into()),
        None => None,
        Some(_) => Some(int(args, "snapshot", 0, 0, i64::MAX)?),
    };
    if requested.is_some_and(|s| s != current) {
        return Err(CHANGED.into());
    }
    // Each word must match as a prefix; quoting keeps FTS operators inert.
    let fts = match query {
        None => None,
        Some(q) => {
            // Words without letters or digits produce empty FTS phrases; ignore them.
            let words: Vec<String> = q
                .split_whitespace()
                .filter(|w| w.chars().any(char::is_alphanumeric))
                .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
                .collect();
            if words.len() > 16 {
                return Err(INVALID.into());
            }
            (!words.is_empty()).then(|| words.join(" AND "))
        }
    };
    let mut values: Vec<Sql> = Vec::new();
    let mut bind = |v: Option<String>| {
        values.push(v.map_or(Sql::Null, Sql::Text));
        values.len()
    };
    let filters = format!(
        " WHERE r.revision=m.current_revision AND (m.archived_at IS NOT NULL)={archived} AND {v} AND (?{p} IS NULL OR m.project=?{p}) \
         AND (?{k} IS NULL OR a.kind IN (SELECT value FROM json_each(?{k}))) \
         AND (?{a} IS NULL OR r.agent=?{a}) \
         AND (?{w} IS NULL OR r.principal=?{w}) \
         AND (?{s} IS NULL OR r.confirmed=(?{s}='confirmed')) \
         AND (?{t} IS NULL OR t.topic=?{t}) \
         AND (?{f} IS NULL OR r.saved_at>=?{f}) \
         AND (?{u} IS NULL OR r.saved_at<date(?{u},'+1 day'))",
        v = inside(bind(Some(visible.to_owned()))),
        p = bind(project),
        k = bind(kinds),
        a = bind(agent),
        w = bind(principal),
        s = bind(state),
        t = bind(topic),
        f = bind(from),
        u = bind(to),
    );
    let searching = fts.is_some();
    let (join, matching, order) = if searching {
        let n = bind(fts);
        (
            " JOIN memory_fts f ON f.rowid=m.rowid",
            format!(" AND memory_fts MATCH ?{n}"),
            "f.rank,r.saved_at DESC,m.id",
        )
    } else {
        ("", String::new(), "r.saved_at DESC,m.id")
    };
    let count_sql = format!(
        "SELECT count(*) FROM memories m JOIN revisions r ON r.id=m.id \
         JOIN revision_metadata a ON a.id=r.id AND a.revision=r.revision \
         LEFT JOIN topics t ON t.id=m.id{join}{filters}{matching}"
    );
    let total: i64 = db
        .query_row(&count_sql, params_from_iter(values.iter()), |r| r.get(0))
        .map_err(|_| DB_ERROR)?;
    let sql = format!(
        "{SELECT}{join}{filters}{matching} ORDER BY {order} LIMIT {} OFFSET {offset}",
        limit + 1
    );
    let mut stmt = db.prepare(&sql).map_err(|_| DB_ERROR)?;
    let mut items = stmt
        .query_map(params_from_iter(values.iter()), row)
        .map_err(|_| DB_ERROR)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| DB_ERROR)?;
    let more = items.len() > limit as usize;
    items.truncate(limit as usize);
    for item in &mut items {
        let full = item["content"].as_str().unwrap_or("");
        let preview: String = full.chars().take(300).collect();
        item["truncated"] = json!(preview.len() < full.len());
        item["content"] = json!(preview);
    }
    Ok(json!({"items":items,"more":more,"total":total,"snapshot":current,"offset":offset}))
}

fn memory(db: &Connection, args: &Map<String, Value>, visible: &str) -> Result<Value> {
    allowed(args, &["project", "id", "before"])?;
    let project = req(args, "project", 161)?;
    let shown: Vec<String> = serde_json::from_str(visible).map_err(|_| DB_ERROR)?;
    if !shown.contains(&project) {
        return Err(NOT_FOUND.into());
    }
    let id = req(args, "id", 128)?;
    let before = match opt(args, "before", 20)? {
        None => None,
        Some(_) => Some(int(args, "before", 0, 2, i64::MAX)?),
    };
    let current = db
        .query_row(
            &format!("{SELECT} WHERE m.project=?1 AND m.id=?2 AND r.revision=m.current_revision"),
            params![project, id],
            row,
        )
        .optional()
        .map_err(|_| DB_ERROR)?
        .ok_or(NOT_FOUND)?;
    let mut stmt = db
        .prepare(&format!(
            "{SELECT} WHERE m.project=?1 AND m.id=?2 AND (?3 IS NULL OR r.revision<?3) \
             ORDER BY r.revision DESC LIMIT {}",
            HISTORY_PAGE + 1
        ))
        .map_err(|_| DB_ERROR)?;
    let mut revisions = stmt
        .query_map(params![project, id, before], row)
        .map_err(|_| DB_ERROR)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| DB_ERROR)?;
    let more = revisions.len() > HISTORY_PAGE;
    revisions.truncate(HISTORY_PAGE);
    revisions.reverse();
    // Older pages continue below the oldest revision returned.
    let next_before = if more {
        revisions.first().map(|r| r["revision"].clone())
    } else {
        None
    };
    let (created, archived): (String, Option<String>) = db
        .query_row(
            "SELECT created_at,archived_at FROM memories WHERE project=?1 AND id=?2",
            params![project, id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| DB_ERROR)?;
    Ok(
        json!({"current":current,"revisions":revisions,"next_before":next_before,"created_at":created,"archived_at":archived}),
    )
}
