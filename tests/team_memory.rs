//! Team access: server-side principal and per-project permissions (synthetic data).
use brainkeep::{access::parse_auth, store::Store};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;

fn temp() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn note(project: &str, op: &str, agent: &str) -> Value {
    json!({"project":project,"operation_id":op,"content":format!("Decisión sintética {op}"),"source":"synthetic:team","agent":agent})
}
fn credentials() -> Vec<brainkeep::access::Access> {
    let raw = json!({
        "ana": {"token":"a".repeat(64),"read":["equipo/*"],"write":["equipo/api"]},
        "bob": {"token":"b".repeat(64),"read":["equipo/web"]},
        "owner": "c".repeat(64),
    });
    let mut v: Vec<_> = parse_auth(raw.to_string().as_bytes())
        .unwrap()
        .into_iter()
        .map(|(_, a)| a)
        .collect();
    v.sort_by(|a, b| a.principal().cmp(b.principal()));
    v
}

#[test]
fn principal_is_the_credential_user_not_arguments() {
    let d = temp();
    let mut s = Store::open(d.path()).unwrap();
    let [ana, _, _] = <[_; 3]>::try_from(credentials()).unwrap();
    let saved = s
        .call_as("remember", note("equipo/api", "p1", "bob"), &ana)
        .unwrap();
    // Legacy credentials act for the admin user (`owner` on a fresh database).
    assert_eq!(
        (saved["principal"].as_str(), saved["agent"].as_str()),
        (Some("owner"), Some("bob"))
    );
    let history = s
        .call_as(
            "history",
            json!({"project":"equipo/api","id":saved["id"]}),
            &ana,
        )
        .unwrap();
    assert_eq!(history["items"][0]["principal"], "owner");
    let local = s.call("remember", note("personal", "p2", "test")).unwrap();
    assert_eq!(local["principal"], "owner");
}

#[test]
fn permissions_apply_to_every_tool_without_revealing_existence() {
    let d = temp();
    let mut s = Store::open(d.path()).unwrap();
    let [ana, bob, owner] = <[_; 3]>::try_from(credentials()).unwrap();
    let api = s
        .call_as("remember", note("equipo/api", "a1", "x"), &ana)
        .unwrap();
    let web = s.call("remember", note("equipo/web", "w1", "x")).unwrap();
    s.call("remember", note("personal", "h1", "x")).unwrap();
    let denied = "project not accessible with this credential";
    // Writes outside the write list fail, even where reading is allowed.
    assert_eq!(
        s.call_as("remember", note("equipo/web", "a2", "x"), &ana)
            .unwrap_err(),
        denied
    );
    assert_eq!(
        s.call_as("remember", note("equipo/web", "b1", "x"), &bob)
            .unwrap_err(),
        denied
    );
    let correct = json!({"project":"equipo/web","id":web["id"],"expected_revision":1,"operation_id":"b2","content":"cambio","source":"s","agent":"x"});
    assert_eq!(s.call_as("correct", correct, &bob).unwrap_err(), denied);
    // Readable projects work for every read tool.
    for (tool, args) in [
        ("recall", json!({"project":"equipo/web","id":web["id"]})),
        ("history", json!({"project":"equipo/web","id":web["id"]})),
        (
            "search",
            json!({"project":"equipo/web","query":"sintética"}),
        ),
        ("context", json!({"project":"equipo/web"})),
    ] {
        assert!(s.call_as(tool, args.clone(), &ana).is_ok(), "{tool}");
        assert!(s.call_as(tool, args, &bob).is_ok(), "{tool}");
    }
    // Unreadable projects give the same error for existing and missing projects.
    for project in ["personal", "inexistente", "equipo/api"] {
        for (tool, args) in [
            ("recall", json!({"project":project,"id":api["id"]})),
            ("history", json!({"project":project,"id":api["id"]})),
            ("search", json!({"project":project,"query":"sintética"})),
            ("context", json!({"project":project})),
        ] {
            assert_eq!(
                s.call_as(tool, args, &bob).unwrap_err(),
                denied,
                "{tool} {project}"
            );
        }
    }
    // Remote credentials never reach owner-only tools.
    assert!(
        s.call_as(
            "backup",
            json!({"destination":d.path().join("x.sqlite")}),
            &owner
        )
        .is_err()
    );
    assert!(s.call_as("panel.status", json!({}), &owner).is_err());
    assert!(!d.path().join("x.sqlite").exists());
}

#[test]
fn status_counts_only_readable_projects() {
    let d = temp();
    let mut s = Store::open(d.path()).unwrap();
    let [ana, bob, owner] = <[_; 3]>::try_from(credentials()).unwrap();
    s.call("remember", note("equipo/api", "a", "x")).unwrap();
    s.call("remember", note("equipo/web", "w", "x")).unwrap();
    s.call("remember", note("personal", "h", "x")).unwrap();
    let count = |s: &mut Store, a| s.call_as("status", json!({}), a).unwrap()["memories"].clone();
    assert_eq!(count(&mut s, &ana), 2);
    assert_eq!(count(&mut s, &bob), 1);
    assert_eq!(count(&mut s, &owner), 3);
    assert_eq!(s.call("status", json!({})).unwrap()["schema_version"], 7);
}

#[test]
fn receipts_are_bound_to_their_principal() {
    let d = temp();
    let mut s = Store::open(d.path()).unwrap();
    let [ana, _, owner] = <[_; 3]>::try_from(credentials()).unwrap();
    let first = s
        .call_as("remember", note("equipo/api", "same", "x"), &ana)
        .unwrap();
    assert_eq!(
        s.call_as("remember", note("equipo/api", "same", "x"), &ana)
            .unwrap(),
        first
    );
    // Another principal's identical operation_id is a separate operation, not an oracle.
    let other = s
        .call_as("remember", note("equipo/api", "same", "x"), &owner)
        .unwrap();
    assert_ne!(other["id"], first["id"]);
    assert_eq!(other["principal"], "owner");
    let local = s.call("remember", note("equipo/api", "same", "x")).unwrap();
    assert_ne!(local["id"], first["id"]);
}

#[test]
fn listing_snapshot_ignores_other_projects() {
    let d = temp();
    let mut s = Store::open(d.path()).unwrap();
    let [_, bob, _] = <[_; 3]>::try_from(credentials()).unwrap();
    s.call("remember", note("equipo/web", "w", "x")).unwrap();
    let snap = |s: &mut Store| {
        s.call_as("context", json!({"project":"equipo/web"}), &bob)
            .unwrap()["snapshot"]
            .clone()
    };
    let before = snap(&mut s);
    for i in 0..5 {
        s.call("remember", note("personal", &format!("h{i}"), "x"))
            .unwrap();
    }
    assert_eq!(snap(&mut s), before);
    s.call("remember", note("equipo/web", "w2", "x")).unwrap();
    // The step reflects only this project's revisions, not the five elsewhere.
    assert_eq!(snap(&mut s), before.as_i64().unwrap() + 1);
}

#[test]
fn remote_retry_of_pre_v3_receipt_is_accepted_only_on_exact_match() {
    let d = temp();
    let mut s = Store::open(d.path()).unwrap();
    let [ana, _, _] = <[_; 3]>::try_from(credentials()).unwrap();
    // A global (unprefixed) receipt, as written by a remote client before v3.
    let old = s.call("remember", note("equipo/api", "pre", "x")).unwrap();
    assert_eq!(
        s.call_as("remember", note("equipo/api", "pre", "x"), &ana)
            .unwrap(),
        old
    );
    // Different arguments do not reveal the id: it is simply a new operation.
    let mut other = note("equipo/api", "pre", "x");
    other["content"] = json!("otra cosa");
    let fresh = s.call_as("remember", other, &ana).unwrap();
    assert_ne!(fresh["id"], old["id"]);
}

#[test]
fn v2_database_migrates_through_v3_with_snapshot_and_null_principal() {
    let d = temp();
    let live = d.path().join("live");
    std::fs::create_dir(&live).unwrap();
    std::fs::set_permissions(&live, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = live.join("memory.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(include_str!("../src/schema.sql")).unwrap();
    db.execute_batch(include_str!("../src/migration_v2.sql"))
        .unwrap();
    db.execute_batch("INSERT INTO memories(rowid,id,project,current_revision,created_at) VALUES(7,'old','demo',1,'2026-01-01'); INSERT INTO revisions VALUES('old',1,'SQLite previo','synthetic:old','old',1,'2026-01-01'); INSERT INTO revision_metadata VALUES('old',1,'','note'); INSERT INTO memory_fts(rowid,id,project,content) VALUES(7,'old','demo','SQLite previo');").unwrap();
    drop(db);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut s = Store::open_with_owner(&live, Some("alice")).unwrap();
    let old = s
        .call("recall", json!({"project":"demo","id":"old"}))
        .unwrap();
    assert_eq!(
        (old["content"].as_str(), old["principal"].is_null()),
        (Some("SQLite previo"), true)
    );
    assert!(live.join("pre-v3.sqlite").exists());
    assert!(!live.join("pre-v2.sqlite").exists());
    Store::restore(&live.join("pre-v3.sqlite"), &d.path().join("restored")).unwrap();
    let mut restored = Store::open_with_owner(&d.path().join("restored"), Some("alice")).unwrap();
    assert_eq!(
        restored
            .call("recall", json!({"project":"demo","id":"old"}))
            .unwrap(),
        old
    );
    drop(s);
    assert!(Store::open(&live).is_ok());
}

#[test]
fn guide_lists_only_own_patterns() {
    let d = temp();
    let mut s = Store::open(d.path()).unwrap();
    let [ana, _, owner] = <[_; 3]>::try_from(credentials()).unwrap();
    s.call("remember", note("personal", "h", "x")).unwrap();
    let g = s.call_as("guide", json!({}), &ana).unwrap();
    assert_eq!(g["principal"], "owner");
    assert_eq!(g["read"], json!(["equipo/*"]));
    assert_eq!(g["write"], json!(["equipo/api"]));
    assert!(!g.to_string().contains("personal"));
    assert_eq!(
        s.call_as("guide", json!({}), &owner).unwrap()["read"],
        json!(["*"])
    );
    assert!(s.call_as("guide", json!({"x":1}), &ana).is_err());
}

#[test]
fn guide_stays_within_budget_for_largest_configuration() {
    // Escaped characters double their serialized size; fill up to the byte budget.
    let config = |unit: &str, n: usize| {
        let long = |i: usize| format!("{}{i}/*", unit.repeat(n));
        json!({"z".repeat(64): {"token":"f".repeat(64),
            "read": (0..8).map(long).collect::<Vec<_>>(),
            "write": (10..18).map(long).collect::<Vec<_>>()}})
        .to_string()
    };
    assert!(parse_auth(config("\\", 125).as_bytes()).is_err());
    let mut n = 125;
    let access = loop {
        if let Ok(mut v) = parse_auth(config("\\", n).as_bytes()) {
            break v.remove(0).1;
        }
        n -= 1;
    };
    let d = temp();
    let mut s = Store::open(d.path()).unwrap();
    let g = s.call_as("guide", json!({}), &access).unwrap();
    let size = json!({"trust":"data_only","data":g}).to_string().len();
    assert!(size <= 4096, "guide bytes: {size}");
}
