//! Multi-user schema v4: users, project ownership and memberships (synthetic data).
use brainkeep::store::Store;
use rusqlite::Connection;
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, path::Path};

fn temp() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

/// Builds a v3 database with two projects, a topic and a legacy receipt.
fn v3_database(dir: &Path) {
    std::fs::create_dir(dir).unwrap();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.join("memory.sqlite");
    let db = Connection::open(&path).unwrap();
    for sql in [
        include_str!("../src/schema.sql"),
        include_str!("../src/migration_v2.sql"),
        include_str!("../src/migration_v3.sql"),
    ] {
        db.execute_batch(sql).unwrap();
    }
    drop(db);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    // Populate through the v3 code path is not available anymore; write rows directly,
    // matching what v3 stored (plain project names, FTS rowid = memories rowid).
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "INSERT INTO memories(rowid,id,project,current_revision,created_at) VALUES(1,'m1','demo',1,'2026-01-01'),(2,'m2','equipo/api',1,'2026-01-02');
         INSERT INTO revisions VALUES('m1',1,'Decisión sintética uno','synthetic:v3','codex',1,'2026-01-01','codex'),('m2',1,'Nota sintética dos','synthetic:v3','claude',0,'2026-01-02','claude');
         INSERT INTO revision_metadata VALUES('m1',1,'Título uno','decision'),('m2',1,'','note');
         INSERT INTO topics VALUES('demo','arquitectura','m1');
         INSERT INTO memory_fts(rowid,id,project,content) VALUES(1,'m1','demo','Título uno Decisión sintética uno'),(2,'m2','equipo/api','Nota sintética dos');
         INSERT INTO operations VALUES('op-legacy','{\"tool\":\"remember\",\"arguments\":{\"project\":\"demo\"}}','{\"id\":\"m1\",\"project\":\"demo\"}','2026-01-01');",
    )
    .unwrap();
}

#[test]
fn v3_with_data_requires_owner_and_stays_untouched() {
    let d = temp();
    let live = d.path().join("live");
    v3_database(&live);
    assert!(Store::open(&live).is_err());
    assert!(!live.join("pre-v4.sqlite").exists());
    let db = Connection::open(live.join("memory.sqlite")).unwrap();
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
}

#[test]
fn v3_migrates_to_v4_under_owner_preserving_names_for_the_owner() {
    let d = temp();
    let live = d.path().join("live");
    v3_database(&live);
    let mut s = Store::open_with_owner(&live, Some("alice")).unwrap();
    assert!(live.join("pre-v4.sqlite").exists());
    assert_eq!(s.call("status", json!({})).unwrap()["schema_version"], 7);

    // The owner keeps using short names; responses show them unchanged.
    let one = s
        .call("recall", json!({"project":"demo","id":"m1"}))
        .unwrap();
    assert_eq!(
        (one["project"].as_str(), one["content"].as_str()),
        (Some("demo"), Some("Decisión sintética uno"))
    );
    let two = s
        .call("recall", json!({"project":"equipo/api","id":"m2"}))
        .unwrap();
    assert_eq!(two["project"], "equipo/api");
    let found = s
        .call("search", json!({"project":"demo","query":"sintética"}))
        .unwrap();
    assert_eq!(found["items"][0]["id"], "m1");
    assert_eq!(found["items"][0]["project"], "demo");
    let topic = s
        .call("context", json!({"project":"demo","topic":"arquitectura"}))
        .unwrap();
    assert_eq!(topic["items"][0]["id"], "m1");

    // New writes land in the owner's namespace and stay addressable by short name.
    let new = s
        .call("remember", json!({"project":"demo","operation_id":"op-new","content":"Nota sintética nueva","source":"synthetic:v4","agent":"test","topic":"nuevo"}))
        .unwrap();
    assert_eq!(new["project"], "demo");
    let again = s
        .call("remember", json!({"project":"demo","operation_id":"op-new","content":"Nota sintética nueva","source":"synthetic:v4","agent":"test","topic":"nuevo"}))
        .unwrap();
    assert_eq!(again["id"], new["id"]);
    drop(s);

    // Internally every project belongs to the owner user.
    let db = Connection::open(live.join("memory.sqlite")).unwrap();
    let projects: Vec<String> = db
        .prepare("SELECT DISTINCT project FROM memories ORDER BY 1")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(projects, ["alice/demo", "alice/equipo/api"]);
    let fts: i64 = db
        .query_row(
            "SELECT count(*) FROM memory_fts WHERE project NOT LIKE 'alice/%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let topic: String = db
        .query_row("SELECT project FROM topics WHERE id='m1'", [], |r| r.get(0))
        .unwrap();
    let owner: (String, i64) = db
        .query_row("SELECT handle,admin FROM users", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(
        (fts, topic.as_str(), owner),
        (0, "alice/demo", ("alice".to_string(), 1))
    );
    drop(db);

    // Reopening needs no owner argument and the pre-v4 snapshot restores as v3.
    assert!(Store::open(&live).is_ok());
    Store::restore(&live.join("pre-v4.sqlite"), &d.path().join("restored")).unwrap();
    let restored = Connection::open(d.path().join("restored/memory.sqlite")).unwrap();
    let version: i64 = restored
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
}

#[test]
fn owner_handle_is_validated_and_fixed_after_migration() {
    let d = temp();
    for (i, bad) in ["", "Ana", "a/b", "local", "-x", &"a".repeat(33)]
        .into_iter()
        .enumerate()
    {
        let live = d.path().join(format!("bad{i}"));
        v3_database(&live);
        assert!(Store::open_with_owner(&live, Some(bad)).is_err(), "{bad}");
    }
    let live = d.path().join("ok");
    v3_database(&live);
    drop(Store::open_with_owner(&live, Some("alice")).unwrap());
    // A different owner on an already migrated database is rejected, not applied.
    assert!(Store::open_with_owner(&live, Some("otro")).is_err());
    assert!(Store::open_with_owner(&live, Some("alice")).is_ok());
}

#[test]
fn fresh_database_creates_default_owner() {
    let d = temp();
    let mut s = Store::open(d.path()).unwrap();
    s.call(
        "remember",
        json!({"project":"demo","operation_id":"a","content":"Nota sintética","source":"synthetic:fresh","agent":"t"}),
    )
    .unwrap();
    assert_eq!(s.call("status", json!({})).unwrap()["schema_version"], 7);
    drop(s);
    let db = Connection::open(d.path().join("memory.sqlite")).unwrap();
    let project: String = db
        .query_row("SELECT project FROM memories", [], |r| r.get(0))
        .unwrap();
    assert_eq!(project, "owner/demo");
}

#[test]
fn inconsistent_v4_ownership_is_rejected() {
    let d = temp();
    drop(Store::open_with_owner(d.path(), Some("alice")).unwrap());
    let db = Connection::open(d.path().join("memory.sqlite")).unwrap();
    db.execute_batch(
        "INSERT INTO memories(rowid,id,project,current_revision,created_at) VALUES(9,'x','nadie/p',1,'2026-01-01');
         INSERT INTO revisions VALUES('x',1,'c','s','a',0,'2026-01-01',NULL);
         INSERT INTO revision_metadata VALUES('x',1,'','note');
         INSERT INTO memory_fts(rowid,id,project,content) VALUES(9,'x','nadie/p','c');",
    )
    .unwrap();
    drop(db);
    assert!(Store::open(d.path()).is_err());
}

/// Schema v5: Google profile photo on users. A v4 database migrates in place with a
/// pre-v5 snapshot; the column only accepts https photos from googleusercontent.
#[test]
fn v4_migrates_to_v5_with_snapshot_and_picture_check() {
    let d = temp();
    let live = d.path().join("live");
    v3_database(&live);
    let db = Connection::open(live.join("memory.sqlite")).unwrap();
    db.execute_batch(include_str!("../src/migration_v4.sql"))
        .unwrap();
    db.execute_batch(
        "INSERT INTO users(handle,email,admin,disabled,created_at) VALUES('alice','alice@example.test',1,0,'2026-01-01');
         UPDATE memories SET project='alice/'||project; UPDATE topics SET project='alice/'||project;
         UPDATE memory_fts SET project='alice/'||project;",
    )
    .unwrap();
    drop(db);
    let mut s = Store::open(&live).unwrap();
    assert_eq!(s.call("status", json!({})).unwrap()["schema_version"], 7);
    drop(s);
    assert!(live.join("pre-v5.sqlite").exists());
    let db = Connection::open(live.join("memory.sqlite")).unwrap();
    let (version, memories, picture): (i64, i64, Option<String>) = db
        .query_row(
            "SELECT (SELECT user_version FROM pragma_user_version),(SELECT count(*) FROM memories),(SELECT picture FROM users WHERE handle='alice')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((version, memories, picture), (7, 2, None));
    let set = |value: &str| {
        db.execute("UPDATE users SET picture=?1 WHERE handle='alice'", [value])
            .is_ok()
    };
    assert!(set("https://lh3.googleusercontent.com/a/ok=s96-c"));
    for bad in [
        "http://lh3.googleusercontent.com/a",
        "https://evil.test/a",
        "https://lh3.googleusercontent.com.evil.test/a",
        "https://lh3.googleusercontent.com/a b",
        "https://lh3.googleusercontent.com/a\"x",
        "https://lh3.googleusercontent.com/<x>",
    ] {
        assert!(!set(bad), "{bad}");
    }
    assert!(!set(&format!(
        "https://lh3.googleusercontent.com/{}",
        "a".repeat(2048)
    )));
    let name = |value: &str| {
        db.execute("UPDATE users SET name=?1 WHERE handle='alice'", [value])
            .is_ok()
    };
    assert!(name("Ana Sintética"));
    assert!(!name(""));
    assert!(!name(" Ana"));
    assert!(!name("a\nb"));
    assert!(!name("a\u{0}b"));
    assert!(!name(&"x".repeat(129)));
    drop(db);
    // Reopening keeps the current schema; the pre-v5 snapshot restores as v4.
    assert!(Store::open(&live).is_ok());
    Store::restore(&live.join("pre-v5.sqlite"), &d.path().join("restored")).unwrap();
    let restored = Connection::open(d.path().join("restored/memory.sqlite")).unwrap();
    let version: i64 = restored
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 4);
}

/// Schema v6: archive and purge. A v5 database migrates in place with a pre-v6
/// snapshot; every memory stays active and readable, and the snapshot restores as v5.
#[test]
fn v5_migrates_to_v6_with_snapshot_and_active_memories() {
    let d = temp();
    let live = d.path().join("live");
    v3_database(&live);
    let db = Connection::open(live.join("memory.sqlite")).unwrap();
    for sql in [
        include_str!("../src/migration_v4.sql"),
        include_str!("../src/migration_v5.sql"),
    ] {
        db.execute_batch(sql).unwrap();
    }
    db.execute_batch(
        "INSERT INTO users(handle,email,admin,disabled,created_at) VALUES('alice','alice@example.test',1,0,'2026-01-01');
         UPDATE memories SET project='alice/'||project; UPDATE topics SET project='alice/'||project;
         UPDATE memory_fts SET project='alice/'||project;",
    )
    .unwrap();
    drop(db);
    let mut s = Store::open(&live).unwrap();
    assert_eq!(s.call("status", json!({})).unwrap()["schema_version"], 7);
    let found = s
        .call("search", json!({"project":"demo","query":"sintética"}))
        .unwrap();
    assert_eq!(found["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        s.call("recall", json!({"project":"demo","id":"m1"}))
            .unwrap()["content"],
        "Decisión sintética uno"
    );
    drop(s);
    assert!(live.join("pre-v6.sqlite").exists());
    let db = Connection::open(live.join("memory.sqlite")).unwrap();
    let (version, archived, purges): (i64, i64, i64) = db
        .query_row(
            "SELECT (SELECT user_version FROM pragma_user_version),(SELECT count(*) FROM memories WHERE archived_at IS NOT NULL),(SELECT count(*) FROM purges)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((version, archived, purges), (7, 0, 0));
    drop(db);
    assert!(Store::open(&live).is_ok());
    Store::restore(&live.join("pre-v6.sqlite"), &d.path().join("restored")).unwrap();
    let restored = Connection::open(d.path().join("restored/memory.sqlite")).unwrap();
    let version: i64 = restored
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 5);
}

/// Schema v7: personal tokens. A v6 database migrates in place with a pre-v7 snapshot,
/// no user has a token yet, and the snapshot restores as v6.
#[test]
fn v6_migrates_to_v7_with_snapshot_and_no_tokens() {
    let d = temp();
    let live = d.path().join("live");
    v3_database(&live);
    let db = Connection::open(live.join("memory.sqlite")).unwrap();
    for sql in [
        include_str!("../src/migration_v4.sql"),
        include_str!("../src/migration_v5.sql"),
        include_str!("../src/migration_v6.sql"),
    ] {
        db.execute_batch(sql).unwrap();
    }
    db.execute_batch(
        "INSERT INTO users(handle,email,admin,disabled,created_at) VALUES('alice','alice@example.test',1,0,'2026-01-01');
         UPDATE memories SET project='alice/'||project; UPDATE topics SET project='alice/'||project;
         UPDATE memory_fts SET project='alice/'||project;",
    )
    .unwrap();
    drop(db);
    let mut s = Store::open(&live).unwrap();
    assert_eq!(s.call("status", json!({})).unwrap()["schema_version"], 7);
    assert_eq!(
        s.call("recall", json!({"project":"demo","id":"m1"}))
            .unwrap()["content"],
        "Decisión sintética uno"
    );
    drop(s);
    assert!(live.join("pre-v7.sqlite").exists());
    let db = Connection::open(live.join("memory.sqlite")).unwrap();
    let tokens: i64 = db
        .query_row("SELECT count(*) FROM personal_tokens", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tokens, 0);
    // Only SHA-256 hex fits: a plaintext token is rejected by the schema.
    assert!(
        db.execute(
            "INSERT INTO personal_tokens VALUES(1,?1,'2026-01-01',0)",
            [format!("sbp_{}", "a".repeat(64))]
        )
        .is_err()
    );
    drop(db);
    Store::restore(&live.join("pre-v7.sqlite"), &d.path().join("restored")).unwrap();
    let restored = Connection::open(d.path().join("restored/memory.sqlite")).unwrap();
    let version: i64 = restored
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 6);
}

fn credentials(raw: serde_json::Value) -> Vec<(String, brainkeep::access::Access)> {
    brainkeep::access::parse_auth(raw.to_string().as_bytes())
        .unwrap()
        .into_iter()
        .map(|(_, a)| (a.principal().to_owned(), a))
        .collect()
}
fn get<'a>(
    c: &'a [(String, brainkeep::access::Access)],
    name: &str,
) -> &'a brainkeep::access::Access {
    &c.iter().find(|(n, _)| n == name).unwrap().1
}
fn note(project: &str, op: &str) -> serde_json::Value {
    json!({"project":project,"operation_id":op,"content":format!("Nota sintética {op}"),"source":"synthetic:users","agent":"t"})
}

#[test]
fn credentials_of_one_user_record_the_same_principal() {
    let d = temp();
    let mut s = Store::open_with_owner(d.path(), Some("alice")).unwrap();
    let c = credentials(json!({"codex":"a".repeat(64),"claude":"b".repeat(64)}));
    let first = s
        .call_as("remember", note("demo", "one"), get(&c, "codex"))
        .unwrap();
    s.call_as(
        "correct",
        json!({"project":"demo","id":first["id"],"expected_revision":1,"operation_id":"two","content":"Corrección sintética","source":"synthetic:users","agent":"t"}),
        get(&c, "claude"),
    )
    .unwrap();
    s.call(
        "correct",
        json!({"project":"demo","id":first["id"],"expected_revision":2,"operation_id":"three","content":"Corrección local","source":"synthetic:users","agent":"t"}),
    )
    .unwrap();
    let history = s
        .call("history", json!({"project":"demo","id":first["id"]}))
        .unwrap();
    let principals: Vec<_> = history["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["principal"].as_str().unwrap())
        .collect();
    assert_eq!(principals, ["alice", "alice", "alice"]);
    let guide = s.call_as("guide", json!({}), get(&c, "codex")).unwrap();
    assert_eq!(guide["principal"], "alice");
}

#[test]
fn invited_users_work_in_their_own_namespace_until_disabled() {
    let d = temp();
    let mut s = Store::open_with_owner(d.path(), Some("alice")).unwrap();
    let c = credentials(json!({
        "ana-laptop": {"token":"c".repeat(64),"user":"ana","read":["*"],"write":["*"]},
        "codex": "d".repeat(64),
    }));
    let ana = get(&c, "ana-laptop");
    // Not invited yet: the credential is inert.
    assert!(s.call_as("remember", note("demo", "early"), ana).is_err());
    assert!(s.call_as("auth.check", json!({}), ana).is_err());

    let invited = s
        .call(
            "users.invite",
            json!({"email":"Ana@Example.org","handle":"ana"}),
        )
        .unwrap();
    assert_eq!(
        (invited["handle"].as_str(), invited["email"].as_str()),
        (Some("ana"), Some("ana@example.org"))
    );
    s.call_as("auth.check", json!({}), ana).unwrap();
    let mine = s.call_as("remember", note("demo", "ana-1"), ana).unwrap();
    assert_eq!(
        (mine["project"].as_str(), mine["principal"].as_str()),
        (Some("demo"), Some("ana"))
    );
    // Same short name, separate projects.
    s.call_as("remember", note("demo", "alice-1"), get(&c, "codex"))
        .unwrap();
    let theirs = s
        .call_as("context", json!({"project":"demo"}), ana)
        .unwrap();
    assert_eq!(theirs["items"].as_array().unwrap().len(), 1);
    assert!(
        s.call_as(
            "recall",
            json!({"project":"demo","id":mine["id"]}),
            get(&c, "codex")
        )
        .is_err()
    );
    assert_eq!(s.call_as("status", json!({}), ana).unwrap()["memories"], 1);
    assert_eq!(s.call("status", json!({})).unwrap()["memories"], 1);

    let listed = s.call("users.list", json!({})).unwrap();
    let handles: Vec<_> = listed["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| {
            (
                u["handle"].as_str().unwrap(),
                u["admin"].as_bool().unwrap(),
                u["disabled"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(handles, [("alice", true, false), ("ana", false, false)]);

    // Disabling takes effect on the next request, without restart.
    s.call("users.disable", json!({"handle":"ana"})).unwrap();
    assert!(s.call_as("auth.check", json!({}), ana).is_err());
    assert!(
        s.call_as("recall", json!({"project":"demo","id":mine["id"]}), ana)
            .is_err()
    );
    // Re-inviting the same email enables the user again with their data intact.
    s.call(
        "users.invite",
        json!({"email":"ana@example.org","handle":"ana"}),
    )
    .unwrap();
    assert_eq!(
        s.call_as("recall", json!({"project":"demo","id":mine["id"]}), ana)
            .unwrap()["id"],
        mine["id"]
    );
}

#[test]
fn user_administration_is_local_and_validated() {
    let d = temp();
    let mut s = Store::open_with_owner(d.path(), Some("alice")).unwrap();
    let c = credentials(json!({"codex":"e".repeat(64)}));
    for tool in ["users.invite", "users.disable", "users.list"] {
        assert!(
            s.call_as(
                tool,
                json!({"email":"x@y.z","handle":"x"}),
                get(&c, "codex")
            )
            .is_err(),
            "{tool}"
        );
    }
    s.call(
        "users.invite",
        json!({"email":"bob@example.org","handle":"bob"}),
    )
    .unwrap();
    for bad in [
        json!({"email":"no-arroba","handle":"x"}),
        json!({"email":"a b@c.d","handle":"x"}),
        json!({"email":"x@y.z","handle":"X"}),
        json!({"email":"x@y.z","handle":"local"}),
        json!({"email":"bob@example.org","handle":"otro"}),
        json!({"email":"otro@example.org","handle":"bob"}),
        json!({"email":"x@y.z","handle":"x","admin":true}),
    ] {
        assert!(s.call("users.invite", bad.clone()).is_err(), "{bad}");
    }
    // The admin can attach an email to their own handle, but cannot be disabled.
    s.call(
        "users.invite",
        json!({"email":"alice@example.org","handle":"alice"}),
    )
    .unwrap();
    assert!(s.call("users.disable", json!({"handle":"alice"})).is_err());
    assert!(s.call("users.disable", json!({"handle":"nadie"})).is_err());
}

#[test]
fn auth_file_user_field_is_validated() {
    let t = "f".repeat(64);
    for bad in [
        json!({"a":{"token":t,"user":"Ana","read":["*"]}}),
        json!({"a":{"token":t,"user":"local","read":["*"]}}),
        json!({"a":{"token":t,"user":7,"read":["*"]}}),
    ] {
        assert!(
            brainkeep::access::parse_auth(bad.to_string().as_bytes()).is_err(),
            "{bad}"
        );
    }
    let ok = credentials(json!({"a":{"token":t,"user":"ana","read":["*"]}}));
    assert_eq!(ok[0].1.user(), Some("ana"));
    let legacy = credentials(json!({"b":t}));
    assert_eq!(legacy[0].1.user(), None);
}

/// alice owns `demo`; ana and bob are invited users with full-pattern credentials.
fn team() -> (
    tempfile::TempDir,
    Store,
    Vec<(String, brainkeep::access::Access)>,
    String,
) {
    let d = temp();
    let mut s = Store::open_with_owner(d.path(), Some("alice")).unwrap();
    for (email, handle) in [("ana@example.org", "ana"), ("bob@example.org", "bob")] {
        s.call("users.invite", json!({"email":email,"handle":handle}))
            .unwrap();
    }
    let c = credentials(json!({
        "ana": {"token":"1".repeat(64),"user":"ana","read":["*"],"write":["*"]},
        "bob": {"token":"2".repeat(64),"user":"bob","read":["*"],"write":["*"]},
    }));
    let id = s
        .call("remember", json!({"project":"demo","operation_id":"o1","content":"Decisión sintética compartible","source":"synthetic:share","agent":"t","topic":"base"}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (d, s, c, id)
}

#[test]
fn unshared_projects_are_invisible_in_every_tool() {
    let (_d, mut s, c, id) = team();
    let ana = get(&c, "ana");
    let not_found = |r: Result<serde_json::Value, String>| r.unwrap_err();
    let recall = not_found(s.call_as("recall", json!({"project":"alice/demo","id":id}), ana));
    let missing = not_found(s.call_as("recall", json!({"project":"alice/nada","id":id}), ana));
    assert_eq!(recall, missing);
    not_found(s.call_as("history", json!({"project":"alice/demo","id":id}), ana));
    not_found(s.call_as("recall", json!({"project":"demo","id":id}), ana));
    for tool in ["context", "search"] {
        let r = s
            .call_as(
                tool,
                json!({"project":"alice/demo","query":"sintética"})
                    .as_object()
                    .map(|o| {
                        let mut o = o.clone();
                        if tool == "context" {
                            o.remove("query");
                        }
                        serde_json::Value::Object(o)
                    })
                    .unwrap(),
                ana,
            )
            .unwrap();
        assert_eq!(r["items"].as_array().unwrap().len(), 0, "{tool}");
    }
    // Writing to `alice/demo` without a share is refused exactly like a missing project,
    // instead of silently creating a look-alike project that would later shadow a share.
    let write = |s: &mut Store, project: &str, op: &str| {
        s.call_as("remember", json!({"project":project,"operation_id":op,"content":"Nota de ana","source":"synthetic:share","agent":"t"}), ana)
            .unwrap_err()
    };
    assert_eq!(
        write(&mut s, "alice/demo", "x"),
        write(&mut s, "alice/nada", "y")
    );
    assert_eq!(
        s.call("context", json!({"project":"demo"})).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(s.call_as("status", json!({}), ana).unwrap()["memories"], 0);
    let listed = s.call_as("projects", json!({}), ana).unwrap();
    assert_eq!(listed["projects"], json!([]));
}

#[test]
fn shared_projects_follow_role_and_revocation() {
    let (_d, mut s, c, id) = team();
    let (ana, bob) = (get(&c, "ana"), get(&c, "bob"));
    s.call(
        "share",
        json!({"project":"demo","email":"ana@example.org","role":"read"}),
    )
    .unwrap();
    let seen = s
        .call_as("recall", json!({"project":"alice/demo","id":id}), ana)
        .unwrap();
    assert_eq!(seen["project"], "alice/demo");
    let found = s
        .call_as(
            "search",
            json!({"project":"alice/demo","query":"compartible"}),
            ana,
        )
        .unwrap();
    assert_eq!(found["items"][0]["id"], json!(id));
    assert_eq!(
        s.call_as(
            "context",
            json!({"project":"alice/demo","topic":"base"}),
            ana
        )
        .unwrap()["items"][0]["id"],
        json!(id)
    );
    // Read role cannot write.
    assert!(
        s.call_as("remember", json!({"project":"alice/demo","operation_id":"r","content":"Intento","source":"s","agent":"t"}), ana)
            .is_err()
    );
    // Only the owner shares; a member cannot reshare.
    assert!(
        s.call_as(
            "share",
            json!({"project":"alice/demo","email":"bob@example.org","role":"read"}),
            ana
        )
        .is_err()
    );
    // Upgrade to write: ana's revision is attributed to ana in alice's project.
    s.call(
        "share",
        json!({"project":"demo","email":"ana@example.org","role":"write"}),
    )
    .unwrap();
    let corrected = s
        .call_as("correct", json!({"project":"alice/demo","id":id,"expected_revision":1,"operation_id":"w","content":"Corrección de ana","source":"s","agent":"t"}), ana)
        .unwrap();
    assert_eq!(corrected["principal"], "ana");
    assert_eq!(
        s.call("recall", json!({"project":"demo","id":id})).unwrap()["content"],
        "Corrección de ana"
    );
    let listed = s.call_as("projects", json!({}), ana).unwrap();
    assert_eq!(
        listed["projects"],
        json!([{"project":"alice/demo","role":"write"}])
    );
    let mine = s.call("projects", json!({})).unwrap();
    assert_eq!(
        mine["projects"],
        json!([{"project":"demo","role":"owner","members":[{"email":"ana@example.org","handle":"ana","role":"write"}]}])
    );
    // bob still sees nothing.
    assert!(
        s.call_as("recall", json!({"project":"alice/demo","id":id}), bob)
            .is_err()
    );
    // Revocation is immediate.
    s.call(
        "unshare",
        json!({"project":"demo","email":"ana@example.org"}),
    )
    .unwrap();
    assert!(
        s.call_as("recall", json!({"project":"alice/demo","id":id}), ana)
            .is_err()
    );
    assert_eq!(
        s.call_as("projects", json!({}), ana).unwrap()["projects"],
        json!([])
    );
}

#[test]
fn sharing_is_validated() {
    let (_d, mut s, c, _) = team();
    let ana = get(&c, "ana");
    for bad in [
        json!({"project":"demo","email":"nadie@example.org","role":"read"}),
        json!({"project":"demo","email":"ana@example.org","role":"admin"}),
        json!({"project":"inexistente","email":"ana@example.org","role":"read"}),
        json!({"project":"demo","email":"ana@example.org"}),
    ] {
        assert!(s.call("share", bad.clone()).is_err(), "{bad}");
    }
    // Sharing with a disabled user, or with oneself, is rejected.
    s.call("users.disable", json!({"handle":"bob"})).unwrap();
    assert!(
        s.call(
            "share",
            json!({"project":"demo","email":"bob@example.org","role":"read"})
        )
        .is_err()
    );
    s.call(
        "users.invite",
        json!({"email":"alice@example.org","handle":"alice"}),
    )
    .unwrap();
    assert!(
        s.call(
            "share",
            json!({"project":"demo","email":"alice@example.org","role":"read"})
        )
        .is_err()
    );
    // A credential restricted by patterns can only share projects it may write.
    let limited = credentials(json!({"l":{"token":"3".repeat(64),"read":["*"],"write":["otro"]}}));
    assert!(
        s.call_as(
            "share",
            json!({"project":"demo","email":"ana@example.org","role":"read"}),
            &limited[0].1
        )
        .is_err()
    );
    assert!(
        s.call_as(
            "unshare",
            json!({"project":"demo","email":"ana@example.org"}),
            ana
        )
        .is_err()
    );
    // A disabled member loses access even while the share row remains.
    s.call(
        "share",
        json!({"project":"demo","email":"ana@example.org","role":"read"}),
    )
    .unwrap();
    s.call("users.disable", json!({"handle":"ana"})).unwrap();
    assert!(s.call_as("projects", json!({}), ana).is_err());
}
