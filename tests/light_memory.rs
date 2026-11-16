use brainkeep::store::Store;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;

fn temp() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn note(op: &str) -> Value {
    json!({"project":"demo","operation_id":op,"content":"Elegimos SQLite para persistencia local", "source":"synthetic:decision","agent":"test","title":"Persistencia local","kind":"decision","topic":"architecture/storage","confirmed":true})
}
fn size(v: &Value) -> usize {
    json!({"trust":"data_only","data":v}).to_string().len()
}

#[test]
fn stable_topic_noop_and_optimistic_correction() {
    let d = temp();
    let mut s = Store::open(d.path()).unwrap();
    let first = s.call("remember", note("one")).unwrap();
    let second = s.call("remember", note("two")).unwrap();
    assert_eq!(first["id"], second["id"]);
    assert_eq!(second["revision"], 1);
    let mut changed = note("three");
    changed["content"] = json!("Elegimos PostgreSQL");
    assert!(s.call("remember", changed.clone()).is_err());
    changed["id"] = first["id"].clone();
    changed["expected_revision"] = json!(1);
    let updated = s.call("correct", changed.clone()).unwrap();
    assert_eq!(updated["revision"], 2);
    changed["operation_id"] = json!("stale");
    assert!(s.call("correct", changed).is_err());
    let mut same = note("four");
    same["id"] = first["id"].clone();
    same["expected_revision"] = json!(2);
    same["content"] = updated["content"].clone();
    assert_eq!(s.call("correct", same).unwrap()["revision"], 2);
    let h = s
        .call("history", json!({"project":"demo","id":first["id"]}))
        .unwrap();
    assert_eq!(h["items"].as_array().unwrap().len(), 2);
    assert_eq!(h["items"][0]["kind"], "decision");
    assert_eq!(h["items"][0]["state"], "replaced");
    let mut other = note("other");
    other["project"] = json!("elsewhere");
    assert_ne!(s.call("remember", other).unwrap()["id"], first["id"]);
    assert!(
        s.call("search", json!({"project":"demo","query":"SQLite"}))
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn compact_budget_and_complete_unicode_revision_paging() {
    let d = temp();
    let mut s = Store::open(d.path()).unwrap();
    let content = "🦀\"\\\tá".repeat(1100);
    let mut n = note("long");
    n["content"] = json!(content);
    n["source"] = json!("\"".repeat(1000));
    n["compact"] = json!(true);
    let saved = s.call("remember", n).unwrap();
    assert!(size(&saved) <= 512);
    assert!(saved.get("content").is_none());
    let mut offset = 0;
    let mut collected = String::new();
    loop {
        let page=s.call("recall",json!({"project":"demo","id":saved["id"],"compact":true,"revision":1,"offset":offset})).unwrap();
        assert!(size(&page) <= 16384);
        collected.push_str(page["content"].as_str().unwrap());
        match page["next_offset"].as_u64() {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    assert_eq!(collected, content);
    for i in 0..8 {
        let mut n = note(&format!("item-{i}"));
        n["topic"] = json!(format!("topic-{i}"));
        n["content"] = json!("SQLite ".repeat(1500));
        s.call("remember", n).unwrap();
    }
    for (tool, limit) in [("search", 2048), ("context", 4096), ("history", 16384)] {
        let mut a = json!({"project":"demo","compact":true,"limit":20});
        if tool == "search" {
            a["query"] = json!("SQLite");
        }
        if tool == "history" {
            a["id"] = saved["id"].clone();
        }
        assert!(size(&s.call(tool, a).unwrap()) <= limit);
    }
    let legacy = s
        .call("recall", json!({"project":"demo","id":saved["id"]}))
        .unwrap();
    assert_eq!(legacy["content"], content);
}

#[test]
fn migration_preserves_legacy_receipts_and_can_restore_snapshot() {
    let d = temp();
    let live = d.path().join("live");
    std::fs::create_dir(&live).unwrap();
    std::fs::set_permissions(&live, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = live.join("memory.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(include_str!("../src/schema.sql")).unwrap();
    db.execute_batch("INSERT INTO memories(rowid,id,project,current_revision,created_at) VALUES(40,'old','demo',1,'2026-01-01'); INSERT INTO revisions VALUES('old',1,'SQLite previo','synthetic:old','old',1,'2026-01-01'); INSERT INTO memory_fts VALUES('old','demo','SQLite previo');").unwrap();
    let legacy_args = json!({"project":"demo","operation_id":"legacy-op","content":"SQLite previo","source":"synthetic:old","agent":"old","confirmed":true});
    let legacy_response = json!({"id":"old","project":"demo","revision":1,"content":"SQLite previo","source":"synthetic:old","agent":"old","saved_at":"2026-01-01","state":"confirmed","source_verified":false});
    db.execute(
        "INSERT INTO operations VALUES(?1,?2,?3,?4)",
        rusqlite::params![
            "legacy-op",
            json!({"tool":"remember","arguments":legacy_args}).to_string(),
            legacy_response.to_string(),
            "2026-01-01"
        ],
    )
    .unwrap();
    drop(db);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut s = Store::open_with_owner(&live, Some("alice")).unwrap();
    assert_eq!(s.call("status", json!({})).unwrap()["schema_version"], 7);
    assert!(live.join("pre-v7.sqlite").exists());
    assert!(live.join("pre-v3.sqlite").exists());
    assert!(live.join("pre-v4.sqlite").exists());
    let old = s
        .call("recall", json!({"project":"demo","id":"old"}))
        .unwrap();
    assert_eq!(old["content"], "SQLite previo");
    assert_eq!(old["kind"], "note");
    assert!(live.join("pre-v2.sqlite").exists());
    Store::restore(&live.join("pre-v2.sqlite"), &d.path().join("restored")).unwrap();
    let mut restored = Store::open_with_owner(&d.path().join("restored"), Some("alice")).unwrap();
    assert_eq!(
        restored
            .call("recall", json!({"project":"demo","id":"old"}))
            .unwrap(),
        old
    );
    assert_eq!(
        s.call("remember", legacy_args.clone()).unwrap(),
        legacy_response
    );
    assert_eq!(
        restored.call("remember", legacy_args).unwrap(),
        legacy_response
    );
    let backup = d.path().join("v2.sqlite");
    s.backup(&backup).unwrap();
    drop(s);
    assert!(Store::open(&live).is_ok());
}

#[test]
fn kind_filters_and_secret_metadata_rejection() {
    let d = temp();
    let mut s = Store::open(d.path()).unwrap();
    s.call("remember", note("good")).unwrap();
    assert!(
        s.call("context", json!({"project":"demo","kind":"progress"}))
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for field in ["title", "topic", "kind"] {
        let mut n = note("bad");
        n[field] = json!("password=synthetic-only");
        assert!(s.call("remember", n).is_err());
    }
    assert_eq!(s.call("status", json!({})).unwrap()["memories"], 1);
}

#[test]
fn listing_continuation_detects_intervening_writes() {
    let d = temp();
    let mut s = Store::open(d.path()).unwrap();
    for i in 0..7 {
        let mut n = note(&format!("list-{i}"));
        n["topic"] = json!(format!("list-{i}"));
        s.call("remember", n).unwrap();
    }
    let first = s
        .call(
            "context",
            json!({"project":"demo","compact":true,"limit":2}),
        )
        .unwrap();
    assert!(first["snapshot"].is_i64());
    let next = json!({"project":"demo","compact":true,"limit":2,"offset":first["next_offset"],"snapshot":first["snapshot"]});
    assert_eq!(
        s.call("context", next.clone()).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        s.call(
            "context",
            json!({"project":"demo","compact":true,"offset":2})
        )
        .is_err()
    );
    let mut n = note("intervening");
    n["topic"] = json!("new-topic");
    s.call("remember", n).unwrap();
    assert!(s.call("context", next).unwrap_err().contains("changed"));
}
