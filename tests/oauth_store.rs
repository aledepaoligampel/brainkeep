//! OAuth persistence: clients, Google login mapping, token issue/rotation (synthetic data).
use brainkeep::store::Store;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;

fn temp() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
/// Distinct 64-hex value per label (stands in for a SHA-256 of a secret).
fn h(c: char) -> String {
    format!("{:02x}", c as u8).repeat(32)
}
const CLIENT: &str = "sbclient_0123456789abcdef";
const NOW: i64 = 1_800_000_000;

fn setup() -> (tempfile::TempDir, Store, i64) {
    let d = temp();
    let mut s = Store::open_with_owner(d.path(), Some("alice")).unwrap();
    s.call(
        "users.invite",
        json!({"email":"ana@example.org","handle":"ana"}),
    )
    .unwrap();
    s.call(
        "oauth.register",
        json!({"client_id":CLIENT,"redirect_uris":["https://claude.ai/api/mcp/auth_callback"],"client_name":"Claude"}),
    )
    .unwrap();
    let user = s
        .call(
            "oauth.login",
            json!({"email":"ana@example.org","sub":"g-1"}),
        )
        .unwrap()["user_id"]
        .as_i64()
        .unwrap();
    (d, s, user)
}
fn issue(
    s: &mut Store,
    user: i64,
    family: char,
    access: char,
    refresh: char,
) -> Result<Value, String> {
    s.call(
        "oauth.issue",
        json!({"user_id":user,"client_id":CLIENT,"family":h(family),"access":h(access),"refresh":h(refresh),"now":NOW}),
    )
}
fn access(s: &mut Store, hash: char, now: i64) -> Result<Value, String> {
    s.call("oauth.access", json!({"hash":h(hash),"now":now}))
}

#[test]
fn clients_register_and_resolve() {
    let (_d, mut s, _) = setup();
    let c = s.call("oauth.client", json!({"client_id":CLIENT})).unwrap();
    assert_eq!(
        c["redirect_uris"],
        json!(["https://claude.ai/api/mcp/auth_callback"])
    );
    assert!(
        s.call(
            "oauth.client",
            json!({"client_id":"sbclient_desconocido00"})
        )
        .is_err()
    );
    assert!(
        s.call(
            "oauth.register",
            json!({"client_id":CLIENT,"redirect_uris":["https://x.test/cb"],"client_name":"Dup"})
        )
        .is_err()
    );
}

#[test]
fn google_login_requires_invited_enabled_email_and_binds_subject() {
    let (_d, mut s, ana) = setup();
    let again = s
        .call(
            "oauth.login",
            json!({"email":"ANA@example.org","sub":"g-1"}),
        )
        .unwrap();
    assert_eq!(
        (again["user_id"].as_i64(), again["handle"].as_str()),
        (Some(ana), Some("ana"))
    );
    // A different Google account claiming the same email is rejected once bound.
    assert!(
        s.call(
            "oauth.login",
            json!({"email":"ana@example.org","sub":"g-2"})
        )
        .is_err()
    );
    // Not invited: rejected and no user is created.
    assert!(
        s.call(
            "oauth.login",
            json!({"email":"intruso@example.org","sub":"g-3"})
        )
        .is_err()
    );
    let users = s.call("users.list", json!({})).unwrap()["users"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(users, 2);
    s.call("users.disable", json!({"handle":"ana"})).unwrap();
    assert!(
        s.call(
            "oauth.login",
            json!({"email":"ana@example.org","sub":"g-1"})
        )
        .is_err()
    );
}

#[test]
fn access_tokens_expire_and_follow_user_state() {
    let (_d, mut s, ana) = setup();
    issue(&mut s, ana, 'f', 'a', 'r').unwrap();
    let ok = access(&mut s, 'a', NOW + 10).unwrap();
    assert_eq!(ok["handle"], "ana");
    assert!(
        access(&mut s, 'a', NOW + 3601).is_err(),
        "access lives one hour"
    );
    assert!(
        access(&mut s, 'r', NOW).is_err(),
        "refresh is not an access token"
    );
    s.call("users.disable", json!({"handle":"ana"})).unwrap();
    assert!(access(&mut s, 'a', NOW + 10).is_err());
    assert!(issue(&mut s, ana, 'g', 'b', 'c').is_err());
    // Re-inviting does not revive sessions that existed before disabling.
    let email = s.call("users.list", json!({})).unwrap()["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["handle"] == "ana")
        .unwrap()["email"]
        .clone();
    s.call("users.invite", json!({"email":email,"handle":"ana"}))
        .unwrap();
    assert!(
        access(&mut s, 'a', NOW + 10).is_err(),
        "revived after re-invite"
    );
}

#[test]
fn refresh_rotates_and_reuse_revokes_the_family() {
    let (_d, mut s, ana) = setup();
    issue(&mut s, ana, 'f', 'a', 'r').unwrap();
    let rotate = |s: &mut Store, old: char, new_a: char, new_r: char, now: i64, client: &str| {
        s.call(
            "oauth.refresh",
            json!({"refresh":h(old),"client_id":client,"access":h(new_a),"new_refresh":h(new_r),"now":now}),
        )
    };
    assert!(rotate(&mut s, 'r', 'b', 's', NOW + 5, "sbclient_otroclienteXX").is_err());
    rotate(&mut s, 'r', 'b', 's', NOW + 5, CLIENT).unwrap();
    assert!(access(&mut s, 'b', NOW + 6).is_ok());
    // Reusing the rotated token revokes every token of the family.
    assert!(rotate(&mut s, 'r', 'c', 't', NOW + 7, CLIENT).is_err());
    assert!(access(&mut s, 'b', NOW + 8).is_err());
    assert!(rotate(&mut s, 's', 'd', 'u', NOW + 9, CLIENT).is_err());
    // Refresh tokens expire after 30 days.
    issue(&mut s, ana, 'g', 'e', 'v').unwrap();
    assert!(rotate(&mut s, 'v', '1', '2', NOW + 30 * 86400 + 1, CLIENT).is_err());
}

#[test]
fn explicit_family_revocation_and_remote_isolation() {
    let (_d, mut s, ana) = setup();
    issue(&mut s, ana, 'f', 'a', 'r').unwrap();
    s.call("oauth.revoke", json!({"family":h('f')})).unwrap();
    assert!(access(&mut s, 'a', NOW).is_err());
    let remote = brainkeep::access::parse_auth(json!({"x":"9".repeat(64)}).to_string().as_bytes())
        .unwrap()
        .remove(0)
        .1;
    for tool in [
        "oauth.access",
        "oauth.login",
        "oauth.issue",
        "oauth.register",
    ] {
        assert!(s.call_as(tool, json!({}), &remote).is_err(), "{tool}");
    }
}

#[test]
fn google_profile_is_dropped_when_the_user_is_disabled() {
    let (d, mut s, _) = setup();
    s.call(
        "oauth.login",
        json!({"email":"ana@example.org","sub":"g-1","name":"Ana","picture":"https://lh3.googleusercontent.com/a/x"}),
    )
    .unwrap();
    s.call("users.disable", json!({"handle":"ana"})).unwrap();
    drop(s);
    let db = rusqlite::Connection::open(d.path().join("memory.sqlite")).unwrap();
    let profile: (Option<String>, Option<String>) = db
        .query_row(
            "SELECT name,picture FROM users WHERE handle='ana'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(profile, (None, None));
}
