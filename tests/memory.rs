use brainkeep::store::Store;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
fn tempdir() -> std::io::Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
}
fn note(op: &str, text: &str) -> Value {
    json!({"project":"demo", "operation_id":op, "content":text, "source":"synthetic:test", "agent":"test"})
}
#[test]
fn durable_memory_search_revision_and_retry() {
    let d = tempdir().unwrap();
    let mut s = Store::open(d.path()).unwrap();
    let input = note("create-1", "Usar SQLite para memoria persistente");
    let first = s.call("remember", input.clone()).unwrap();
    assert_eq!(first["state"], "proposed");
    assert_eq!(s.call("remember", input).unwrap(), first);
    assert!(
        s.call("remember", note("create-1", "otro contenido"))
            .is_err()
    );
    let id = first["id"].clone();
    let changed = json!({"id":id,"project":"demo","expected_revision":1,"operation_id":"correct-1","content":"Usar SQLite con FTS5", "source":"synthetic:correction","agent":"claude","confirmed":true});
    let current = s.call("correct", changed.clone()).unwrap();
    assert_eq!(current["revision"], 2);
    assert_eq!(current["state"], "confirmed");
    assert_eq!(s.call("correct", changed).unwrap(), current);
    assert!(s.call("correct", json!({"id":id,"project":"demo","expected_revision":1,"operation_id":"stale","content":"overwrite","source":"test","agent":"test"})).is_err());
    let h = s
        .call("history", json!({"id":id,"project":"demo"}))
        .unwrap();
    assert_eq!(h["items"][0]["state"], "replaced");
    assert_eq!(h["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        s.call("search", json!({"project":"demo","query":"FTS5"}))
            .unwrap()["items"][0]["id"],
        id
    );
    assert!(
        s.call("search", json!({"project":"other","query":"FTS5"}))
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        s.call("recall", json!({"project":"other","id":id}))
            .is_err()
    );
    drop(s);
    let mut s = Store::open(d.path()).unwrap();
    assert_eq!(
        s.call("recall", json!({"project":"demo","id":id})).unwrap(),
        current
    );
    assert_eq!(s.call("status", json!({})).unwrap()["memories"], 1);
}
#[test]
fn secret_rejection_all_fields_leaves_no_trace() {
    let d = tempdir().unwrap();
    let mut s = Store::open(d.path()).unwrap();
    let fixtures = [
        "-----BEGIN OPENSSH PRIVATE KEY-----\nSYNTHETIC",
        "password=fictional-password-942812",
        "token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890",
        "api_key=sk-fictional942812942812",
    ];
    for secret in fixtures {
        for field in ["content", "project", "source", "agent", "operation_id"] {
            let mut v = note("reject", "safe");
            v[field] = json!(secret);
            let err = s.call("remember", v).unwrap_err();
            assert!(!err.contains(secret));
        }
    }
    let mut v = note("secret-flag", "ordinary text");
    v["secret"] = json!(true);
    assert!(s.call("remember", v).is_err());
    assert_eq!(s.call("status", json!({})).unwrap()["memories"], 0);
    s.call(
        "remember",
        note("ssh", "Acceder con ssh server1; autenticación ssh-agent"),
    )
    .unwrap();
    let backup = d.path().join("safe-backup.sqlite");
    s.backup(&backup).unwrap();
    for entry in std::fs::read_dir(d.path()).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() {
            let bytes = std::fs::read(p).unwrap();
            for secret in fixtures {
                assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()));
            }
        }
    }
}
#[test]
fn snapshot_restore_and_no_clobber() {
    let d = tempdir().unwrap();
    let mut s = Store::open(&d.path().join("live")).unwrap();
    let n = s
        .call("remember", note("backup", "SQLite backup decision"))
        .unwrap();
    let n = s
        .call(
            "correct",
            json!({
                "project":"demo", "id":n["id"], "expected_revision":1,
                "operation_id":"backup-revision", "content":"SQLite backup with revisions",
                "source":"synthetic:correction", "agent":"test", "confirmed":true
            }),
        )
        .unwrap();
    let backup = d.path().join("backup.sqlite");
    s.backup(&backup).unwrap();
    assert!(s.backup(&backup).is_err());
    let dest = d.path().join("restored");
    Store::restore(&backup, &dest).unwrap();
    let mut restored = Store::open(&dest).unwrap();
    assert_eq!(
        restored
            .call("recall", json!({"project":"demo","id":n["id"]}))
            .unwrap(),
        n
    );
    assert_eq!(
        restored
            .call("search", json!({"project":"demo","query":"SQLite"}))
            .unwrap()["items"][0]["id"],
        n["id"]
    );
    assert!(Store::restore(&backup, &dest).is_err());
    let history = restored
        .call("history", json!({"project":"demo","id":n["id"]}))
        .unwrap();
    assert_eq!(history["items"].as_array().unwrap().len(), 2);
    assert_eq!(history["items"][0]["state"], "replaced");
    let invalid = d.path().join("invalid");
    std::fs::write(&invalid, b"not a database").unwrap();
    std::fs::set_permissions(&invalid, std::fs::Permissions::from_mode(0o600)).unwrap();
    let newdest = d.path().join("invalid-restore");
    assert!(Store::restore(&invalid, &newdest).is_err());
    assert!(!newdest.exists());
}

#[test]
fn storage_error_is_atomic_and_retryable() {
    let d = tempdir().unwrap();
    let mut s = Store::open(d.path()).unwrap();
    // Inject a deterministic SQLite write failure into this disposable test database.
    let db = rusqlite::Connection::open(d.path().join("memory.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER test_failure BEFORE INSERT ON revisions BEGIN SELECT RAISE(ABORT,'synthetic write failure'); END;").unwrap();
    let request = note("retry-failed-write", "Durable atomic write");
    assert!(s.call("remember", request.clone()).is_err());
    assert_eq!(s.call("status", json!({})).unwrap()["memories"], 0);
    assert_eq!(
        db.query_row("SELECT count(*) FROM operations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM memory_fts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    db.execute_batch("DROP TRIGGER test_failure").unwrap();
    s.call("remember", request).unwrap();
    assert_eq!(s.call("status", json!({})).unwrap()["memories"], 1);
}
#[test]
fn only_one_storage_owner_and_private_paths() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let d = tempdir().unwrap();
    let s = Store::open(&d.path().join("owner")).unwrap();
    assert!(Store::open(&d.path().join("owner")).is_err());
    drop(s);
    assert!(Store::open(&d.path().join("owner")).is_ok());
    let public = d.path().join("public");
    std::fs::create_dir(&public).unwrap();
    std::fs::set_permissions(&public, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Store::open(&public).is_err());
    symlink(d.path().join("owner"), d.path().join("linked")).unwrap();
    assert!(Store::open(&d.path().join("linked")).is_err());
}

#[test]
fn incomplete_backup_is_rejected_before_creating_destination() {
    let d = tempdir().unwrap();
    let mut store = Store::open(&d.path().join("live")).unwrap();
    store
        .call("remember", note("restore-schema", "Synthetic decision"))
        .unwrap();
    let backup = d.path().join("incomplete.sqlite");
    store.backup(&backup).unwrap();
    let corrupt = rusqlite::Connection::open(&backup).unwrap();
    corrupt.execute_batch("DROP TABLE operations;").unwrap();
    assert_eq!(
        corrupt
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(corrupt);
    let dest = d.path().join("must-not-exist");
    assert!(Store::restore(&backup, &dest).is_err());
    assert!(!dest.exists());
}
