//! Web panel: static assets plus a cookie-authenticated JSON API. User sessions (Google,
//! admin key or personal token) may edit (owner or write member); the read credential
//! stays read-only. Panel credentials are separate from agent tokens and never grant
//! /mcp. Sessions act for one user, or for the admin read-only (read credential).
use axum::{
    body::{Body, Bytes, to_bytes},
    extract::{Query, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::OpenOptions,
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::sync::{Semaphore, broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const COOKIE: &str = "brainkeep_panel";
const LIFETIME: u64 = 30 * 86400;
const FAILURES_PER_WINDOW: u32 = 10;
const FAILURE_WINDOW: Duration = Duration::from_secs(60);
const MAX_CLIENTS: usize = 4096;
const BODY_TIMEOUT: Duration = Duration::from_secs(5);
/// Largest edit body: 16 KiB of content plus the other fields, JSON-escaped.
const EDIT_BODY: usize = 64 * 1024;
/// Store messages an edit may show as-is: they describe the user's own input.
const EDIT_MESSAGES: [&str; 13] = [
    "secret rejected; nothing saved",
    "topic is stable; create a separate memory for a new subject",
    "invalid email or handle",
    "email already belongs to another user",
    "handle already belongs to another email",
    "user not found or is the admin",
    "user not found, disabled or is the admin",
    "user not found, not disabled or is the admin",
    "no enabled user with that handle",
    "project is not shared with that user",
    "role must be read or write",
    "email or handle belongs to the admin",
    "user already exists and is enabled",
];
/// Panel writes: memory edits, sharing (owners) and user administration (admin only).
const WRITES: [&str; 9] = [
    "correct", "archive", "purge", "share", "unshare", "invite", "disable", "enable", "token",
];
/// Live-update streams: comment every 30 s keeps proxies (Cloudflare: 100 s idle) open.
const KEEPALIVE: Duration = Duration::from_secs(30);
const MAX_STREAMS: usize = 32;
const MAX_STREAMS_PER_USER: usize = 4;
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' https://lh3.googleusercontent.com; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// Secrets loaded once at startup. `mac_key` binds sessions to the session key, the
/// read credential and the admin key (when set), so rotating any revokes every cookie.
pub struct Credentials {
    read: Vec<u8>,
    /// Optional admin key: signs in as the admin user with write access.
    admin: Option<Vec<u8>>,
    mac_key: [u8; 32],
}

pub fn load(path: &Path, agent_tokens: &[Vec<u8>]) -> Result<Credentials, String> {
    const INVALID: &str = "panel file requires distinct 64-hex \"read\" and \"session_key\", and optionally \"admin\"";
    brainkeep::files::check_file(path)?;
    let mut raw = Vec::new();
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "cannot open panel credentials file")?
        .take(4097)
        .read_to_end(&mut raw)
        .map_err(|_| "cannot read panel credentials file")?;
    let entries: BTreeMap<String, String> = serde_json::from_slice(&raw).map_err(|_| INVALID)?;
    let (Some(read), Some(key)) = (entries.get("read"), entries.get("session_key")) else {
        return Err(INVALID.into());
    };
    let hex = |v: &str| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit());
    // An all-zero value is a placeholder, never a generated secret.
    let weak = |v: &str| v.bytes().all(|b| b == b'0');
    let admin = entries.get("admin");
    if raw.len() > 4096
        || entries.len() != 2 + usize::from(admin.is_some())
        || !hex(read)
        || !hex(key)
        || read == key
        || weak(read)
        || weak(key)
        || read.eq_ignore_ascii_case(key)
        || admin.is_some_and(|a| {
            !hex(a) || weak(a) || a.eq_ignore_ascii_case(read) || a.eq_ignore_ascii_case(key)
        })
    {
        return Err(INVALID.into());
    }
    if agent_tokens.iter().any(|t| {
        t.eq_ignore_ascii_case(read.as_bytes())
            || admin.is_some_and(|a| t.eq_ignore_ascii_case(a.as_bytes()))
    }) {
        return Err("panel credentials must differ from agent credentials".into());
    }
    let key: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&key[i * 2..i * 2 + 2], 16).unwrap_or(0))
        .collect();
    // Without an admin key the MAC input is unchanged, so existing cookies stay valid.
    let bound = match admin {
        Some(a) => format!("{read}.{a}"),
        None => read.clone(),
    };
    Ok(Credentials {
        mac_key: hmac(&key, bound.as_bytes()),
        read: read.clone().into_bytes(),
        admin: admin.map(|a| a.clone().into_bytes()),
    })
}

fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| block.map(|b| b ^ byte);
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(message)
        .finalize();
    let outer = Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer);
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[derive(Clone)]
pub struct Panel {
    credentials: Arc<Credentials>,
    jobs: mpsc::Sender<crate::mcp::Job>,
    host: Arc<String>,
    failures: Arc<Mutex<HashMap<String, (Instant, u32)>>>,
    slots: Arc<Semaphore>,
    login_slots: Arc<Semaphore>,
    /// Whether "Sign in with Google" is offered (OAuth configured).
    google: bool,
    changes: broadcast::Sender<(String, bool)>,
    shutdown: CancellationToken,
    streams: Arc<Semaphore>,
    per_user: Arc<Mutex<HashMap<String, usize>>>,
}

impl Panel {
    pub fn new(
        credentials: Credentials,
        jobs: mpsc::Sender<crate::mcp::Job>,
        host: String,
        google: bool,
        changes: broadcast::Sender<(String, bool)>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            credentials: Arc::new(credentials),
            jobs,
            host: Arc::new(host),
            failures: Arc::new(Mutex::new(HashMap::new())),
            slots: Arc::new(Semaphore::new(16)),
            // Unauthenticated, body-reading requests get their own small pool so
            // slow uploads cannot starve sessions or static assets.
            login_slots: Arc::new(Semaphore::new(4)),
            google,
            changes,
            shutdown,
            streams: Arc::new(Semaphore::new(MAX_STREAMS)),
            per_user: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    /// `v1` sessions (read credential) act as the admin; `v2` sessions carry the
    /// handle of the user who signed in with Google, last because handles contain dots.
    fn issue(&self, handle: Option<&str>) -> (String, u64) {
        self.issue_with(handle, &hex(uuid::Uuid::new_v4().as_bytes()))
    }
    fn issue_with(&self, handle: Option<&str>, nonce: &str) -> (String, u64) {
        let expires = now() + LIFETIME;
        let value = match handle {
            None => format!("v1.{expires}.{nonce}"),
            Some(h) => format!("v2.{expires}.{nonce}.{h}"),
        };
        let mac = hex(&hmac(&self.credentials.mac_key, value.as_bytes()));
        (format!("{value}.{mac}"), expires)
    }
    /// Session cookie for a user who just signed in with Google, and the SHA-256 of its
    /// nonce, which the store keeps so disabling the user or logging out ends it.
    pub fn login_cookie(&self, handle: &str) -> (HeaderValue, String) {
        let (value, _) = self.issue(Some(handle));
        let nonce = value.split('.').nth(2).unwrap_or_default();
        (
            cookie(&value, LIFETIME),
            hex(&Sha256::digest(nonce.as_bytes())),
        )
    }
    /// Expiry and user (None: admin via read credential) of a valid session cookie;
    /// a user session also yields the hash that identifies it in the store.
    fn session(&self, headers: &HeaderMap) -> Option<(u64, Option<(String, String)>)> {
        let mut found = None;
        for header in headers.get_all(header::COOKIE) {
            let Ok(header) = header.to_str() else {
                continue;
            };
            for pair in header.split(';') {
                if let Some(value) = pair.trim().strip_prefix("brainkeep_panel=") {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(value.to_owned());
                }
            }
        }
        let value = found?;
        let (signed, mac) = value.rsplit_once('.')?;
        let expected = hex(&hmac(&self.credentials.mac_key, signed.as_bytes()));
        if !bool::from(mac.as_bytes().ct_eq(expected.as_bytes())) {
            return None;
        }
        let mut parts = signed.splitn(4, '.');
        let (version, Some(expires), Some(nonce), handle) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        let user = match (version, handle) {
            (Some("v1"), None) => None,
            (Some("v2"), Some(h)) if brainkeep::access::valid_handle(h) => {
                Some((h.to_owned(), hex(&Sha256::digest(nonce.as_bytes()))))
            }
            _ => return None,
        };
        let expires: u64 = expires.parse().ok()?;
        let now = now();
        (expires > now && expires <= now + LIFETIME + 60).then_some((expires, user))
    }
    /// Anti-CSRF token of a Google session: bound to that session's nonce hash, so it
    /// is useless with any other cookie. The read key has none (it never writes).
    fn csrf(&self, hash: &str) -> String {
        hex(&hmac(
            &self.credentials.mac_key,
            format!("csrf.{hash}").as_bytes(),
        ))
    }
    fn same_host(&self, headers: &HeaderMap) -> bool {
        let mut hosts = headers.get_all(header::HOST).iter();
        matches!((hosts.next(), hosts.next()), (Some(h), None) if h.as_bytes() == self.host.as_bytes())
    }
    fn same_origin(&self, headers: &HeaderMap) -> bool {
        let mut origins = headers.get_all(header::ORIGIN).iter();
        let expected = format!("https://{}", self.host);
        matches!((origins.next(), origins.next()), (Some(o), None) if o.as_bytes() == expected.as_bytes())
    }
    /// Throttle bucket. Cloudflare overwrites `CF-Connecting-IP` at its edge and the
    /// origin listens on loopback only, so the header identifies remote clients.
    /// Without it (or once the table is full) every attempt shares one bucket.
    fn client(headers: &HeaderMap) -> String {
        let mut values = headers.get_all("cf-connecting-ip").iter();
        match (values.next().and_then(|v| v.to_str().ok()), values.next()) {
            (Some(ip), None) => ip
                .parse::<std::net::IpAddr>()
                .map_or_else(|_| String::new(), |ip| ip.to_string()),
            _ => String::new(),
        }
    }
    /// Counts this attempt as a failure in advance; `forgive` undoes it on success.
    /// Returns the bucket used, or None while that bucket's window is exhausted.
    fn attempt(&self, client: &str) -> Option<String> {
        let mut table = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        table.retain(|_, (start, _)| start.elapsed() < FAILURE_WINDOW);
        let key = if table.contains_key(client) || table.len() < MAX_CLIENTS {
            client
        } else {
            ""
        };
        let key = key.to_owned();
        let entry = table.entry(key.clone()).or_insert((Instant::now(), 0));
        if entry.1 >= FAILURES_PER_WINDOW {
            return None;
        }
        entry.1 += 1;
        Some(key)
    }
    fn forgive(&self, client: &str) {
        let mut table = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(client) {
            entry.1 = entry.1.saturating_sub(1);
        }
    }
}

fn secure(mut response: Response, cache: &'static str) -> Response {
    let headers = response.headers_mut();
    for (name, value) in [
        (header::CACHE_CONTROL, cache),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::CONTENT_SECURITY_POLICY, CSP),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    response
}

fn api(code: StatusCode, body: Value) -> Response {
    secure(
        (
            code,
            [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
            body.to_string(),
        )
            .into_response(),
        "no-store",
    )
}

fn error(code: StatusCode) -> Response {
    let message = match code {
        StatusCode::UNAUTHORIZED => "session required",
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::NOT_FOUND => "not found",
        StatusCode::BAD_REQUEST => "invalid parameters",
        StatusCode::CONFLICT => "memory changed; restart listing",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "JSON required",
        StatusCode::TOO_MANY_REQUESTS => "too many attempts; wait a minute",
        StatusCode::REQUEST_TIMEOUT => "request body too slow",
        _ => "Brainkeep unavailable",
    };
    let mut response = api(code, json!({"error":message}));
    if code == StatusCode::TOO_MANY_REQUESTS {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
    }
    response
}

fn cookie(value: &str, max_age: u64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE}={value}; Path=/panel; Max-Age={max_age}; HttpOnly; Secure; SameSite=Strict"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("brainkeep_panel=; Path=/panel; Max-Age=0"))
}

/// Built by `panel-ui` (Svelte) into src/panel with fixed names; embedded, never read from disk.
fn asset(path: &str) -> Option<(&'static [u8], &'static str)> {
    Some(match path {
        "" | "/" => (
            include_bytes!("panel/index.html"),
            "text/html; charset=utf-8",
        ),
        "/app.css" => (include_bytes!("panel/app.css"), "text/css; charset=utf-8"),
        "/app.js" => (
            include_bytes!("panel/app.js"),
            "text/javascript; charset=utf-8",
        ),
        "/icon.svg" => (include_bytes!("panel/icon.svg"), "image/svg+xml"),
        "/fonts/inter-latin-wght-normal.woff2" => (
            include_bytes!("panel/fonts/inter-latin-wght-normal.woff2"),
            "font/woff2",
        ),
        "/fonts/lora-latin-wght-normal.woff2" => (
            include_bytes!("panel/fonts/lora-latin-wght-normal.woff2"),
            "font/woff2",
        ),
        "/fonts/lora-latin-wght-italic.woff2" => (
            include_bytes!("panel/fonts/lora-latin-wght-italic.woff2"),
            "font/woff2",
        ),
        "/fonts/jetbrains-mono-latin-wght-normal.woff2" => (
            include_bytes!("panel/fonts/jetbrains-mono-latin-wght-normal.woff2"),
            "font/woff2",
        ),
        _ => return None,
    })
}

pub async fn handle(State(panel): State<Panel>, request: Request) -> Response {
    let is_login = request.method() == Method::POST && request.uri().path() == "/panel/api/login";
    let pool = if is_login {
        &panel.login_slots
    } else {
        &panel.slots
    };
    let Ok(_permit) = pool.try_acquire() else {
        return error(StatusCode::SERVICE_UNAVAILABLE);
    };
    if !panel.same_host(request.headers()) {
        return error(StatusCode::FORBIDDEN);
    }
    let path = request
        .uri()
        .path()
        .strip_prefix("/panel")
        .unwrap_or("")
        .to_owned();
    let Some(view) = path.strip_prefix("/api/") else {
        if request.method() != Method::GET {
            return error(StatusCode::NOT_FOUND);
        }
        return match asset(&path) {
            Some((body, kind)) => secure(
                ([(header::CONTENT_TYPE, kind)], body).into_response(),
                "no-cache",
            ),
            None => error(StatusCode::NOT_FOUND),
        };
    };
    let view = view.to_owned();
    match (request.method().clone(), view.as_str()) {
        (Method::POST, "login") => login(&panel, request).await,
        (Method::GET, "login") => api(StatusCode::OK, json!({"google":panel.google})),
        (Method::POST, "logout") => {
            if !panel.same_origin(request.headers()) {
                return error(StatusCode::FORBIDDEN);
            }
            // A user session also ends in the store, so a copied cookie stops working.
            if let Some((_, Some((_, hash)))) = panel.session(request.headers()) {
                let (reply, rx) = oneshot::channel();
                let job = crate::mcp::Job {
                    name: "oauth.panel_end".into(),
                    args: json!({"hash":hash}),
                    access: Arc::new(brainkeep::access::Access::local()),
                    reply,
                };
                if panel.jobs.try_send(job).is_err()
                    || !matches!(
                        tokio::time::timeout(Duration::from_secs(15), rx).await,
                        Ok(Ok(Ok(_)))
                    )
                {
                    return error(StatusCode::SERVICE_UNAVAILABLE);
                }
                // Only a session that really ended wakes the streams (re-check now);
                // anonymous logout calls cost nothing.
                let _ = panel.changes.send(("*".into(), true));
            }
            let mut response = api(StatusCode::OK, json!({"logged_out":true}));
            response
                .headers_mut()
                .insert(header::SET_COOKIE, cookie("", 0));
            response
        }
        (Method::GET, "events") => events(&panel, request.headers()).await,
        (Method::POST, v) if WRITES.contains(&v) => edit(&panel, &view, request).await,
        (
            Method::GET,
            "session" | "status" | "projects" | "facets" | "memories" | "memory" | "activity"
            | "users" | "sharing",
        ) => {
            let Some((expires, user)) = panel.session(request.headers()) else {
                return error(StatusCode::UNAUTHORIZED);
            };
            let access = session_access(&user);
            let csrf = user.as_ref().map(|(_, hash)| panel.csrf(hash));
            let user = user.map(|(handle, _)| handle);
            if view == "session" {
                // A disabled user's cookie stops working at once, not at expiry.
                return match call(&panel, "profile", json!({}), access).await {
                    Ok(p) => api(
                        StatusCode::OK,
                        json!({"expires_at":expires,"user":user,"name":p["name"],"picture":p["picture"],"email":p["email"],"admin":p["admin"],"csrf":csrf}),
                    ),
                    Err(status) => error(status),
                };
            }
            let Ok(Query(pairs)) = Query::<Vec<(String, String)>>::try_from_uri(request.uri())
            else {
                return error(StatusCode::BAD_REQUEST);
            };
            let mut args = Map::new();
            for (key, value) in pairs {
                if args.insert(key, Value::String(value)).is_some() {
                    return error(StatusCode::BAD_REQUEST);
                }
            }
            query(&panel, &view, Value::Object(args), access).await
        }
        _ => error(StatusCode::NOT_FOUND),
    }
}

async fn login(panel: &Panel, request: Request) -> Response {
    if !panel.same_origin(request.headers()) {
        return error(StatusCode::FORBIDDEN);
    }
    let json_body = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(';').next() == Some("application/json"));
    if !json_body {
        return error(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let client = Panel::client(request.headers());
    let Some(bucket) = panel.attempt(&client) else {
        return error(StatusCode::TOO_MANY_REQUESTS);
    };
    let bytes = match tokio::time::timeout(BODY_TIMEOUT, to_bytes(request.into_body(), 1024)).await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => return error(StatusCode::BAD_REQUEST),
        Err(_) => return error(StatusCode::REQUEST_TIMEOUT),
    };
    let supplied = serde_json::from_slice::<Map<String, Value>>(&bytes)
        .ok()
        .filter(|m| m.len() == 1)
        .and_then(|m| m.get("token").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    // The read key opens the read-only admin session; the admin key and personal tokens
    // open a user session recorded in the store, like a Google sign-in.
    if bool::from(supplied.as_bytes().ct_eq(&panel.credentials.read)) {
        panel.forgive(&bucket);
        let (value, expires) = panel.issue(None);
        let mut response = api(StatusCode::OK, json!({"expires_at":expires}));
        response
            .headers_mut()
            .insert(header::SET_COOKIE, cookie(&value, LIFETIME));
        return response;
    }
    let is_admin = panel
        .credentials
        .admin
        .as_ref()
        .is_some_and(|a| bool::from(supplied.as_bytes().ct_eq(a)));
    let personal = supplied.len() == 68
        && supplied
            .strip_prefix(brainkeep::store::PERSONAL_PREFIX)
            .is_some_and(|h| h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    if !is_admin && !personal {
        return error(StatusCode::UNAUTHORIZED);
    }
    let nonce = hex(uuid::Uuid::new_v4().as_bytes());
    let mut args = json!({"hash":hex(&Sha256::digest(nonce.as_bytes())),"now":now()});
    if !is_admin {
        args["token"] = json!(hex(&Sha256::digest(supplied.as_bytes())));
    }
    let local = brainkeep::access::Access::local();
    let found = match job(panel, "oauth.panel_login", args, local).await {
        Ok(found) => found,
        // Only an unknown token or a disabled user is a failed attempt; store errors are not.
        Err(Some(e)) if e == brainkeep::access::INACTIVE => {
            return error(StatusCode::UNAUTHORIZED);
        }
        Err(_) => {
            panel.forgive(&bucket);
            return error(StatusCode::SERVICE_UNAVAILABLE);
        }
    };
    let Some(handle) = found["handle"]
        .as_str()
        .filter(|h| brainkeep::access::valid_handle(h))
    else {
        return error(StatusCode::SERVICE_UNAVAILABLE);
    };
    panel.forgive(&bucket);
    let (value, expires) = panel.issue_with(Some(handle), &nonce);
    let mut response = api(StatusCode::OK, json!({"expires_at":expires}));
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie(&value, LIFETIME));
    response
}

/// Panel writes: `correct` (new revision), `archive` (also restores), `purge` and, for
/// the admin, user administration. Only a user session (Google, admin key or personal
/// token) may write, and only with the page's Origin and its anti-CSRF token; the store
/// checks owner/write rights and that administration comes from the admin.
async fn edit(panel: &Panel, view: &str, request: Request) -> Response {
    let Some((_, user)) = panel.session(request.headers()) else {
        return error(StatusCode::UNAUTHORIZED);
    };
    if !panel.same_origin(request.headers()) {
        return error(StatusCode::FORBIDDEN);
    }
    let Some((handle, hash)) = user else {
        return error(StatusCode::FORBIDDEN);
    };
    let mut tokens = request.headers().get_all("x-csrf-token").iter();
    let valid = match (tokens.next(), tokens.next()) {
        (Some(t), None) => bool::from(t.as_bytes().ct_eq(panel.csrf(&hash).as_bytes())),
        _ => false,
    };
    if !valid {
        return error(StatusCode::FORBIDDEN);
    }
    let json_body = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(';').next() == Some("application/json"));
    if !json_body {
        return error(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let bytes =
        match tokio::time::timeout(BODY_TIMEOUT, to_bytes(request.into_body(), EDIT_BODY)).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(_)) => return error(StatusCode::BAD_REQUEST),
            Err(_) => return error(StatusCode::REQUEST_TIMEOUT),
        };
    let Ok(args) = serde_json::from_slice::<Map<String, Value>>(&bytes) else {
        return error(StatusCode::BAD_REQUEST);
    };
    let access = session_access(&Some((handle, hash)));
    match job(panel, &format!("panel.{view}"), Value::Object(args), access).await {
        Ok(value) => api(StatusCode::OK, value),
        Err(Some(message)) => match message.as_str() {
            brainkeep::store::REVISION_CONFLICT | brainkeep::store::MUST_ARCHIVE => {
                error(StatusCode::CONFLICT)
            }
            brainkeep::panel_query::NOT_FOUND => error(StatusCode::NOT_FOUND),
            brainkeep::access::DENIED => error(StatusCode::FORBIDDEN),
            brainkeep::access::INACTIVE => error(StatusCode::UNAUTHORIZED),
            m if EDIT_MESSAGES.contains(&m) => api(StatusCode::BAD_REQUEST, json!({"error":m})),
            // Remaining store errors are input checks (fields, kind, sizes, retries).
            _ => error(StatusCode::BAD_REQUEST),
        },
        Err(None) => error(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn query(
    panel: &Panel,
    view: &str,
    args: Value,
    access: brainkeep::access::Access,
) -> Response {
    match call(panel, view, args, access).await {
        Ok(value) => api(StatusCode::OK, value),
        Err(status) => error(status),
    }
}

async fn call(
    panel: &Panel,
    view: &str,
    args: Value,
    access: brainkeep::access::Access,
) -> Result<Value, StatusCode> {
    job(panel, &format!("panel.{view}"), args, access)
        .await
        .map_err(|message| match message.as_deref() {
            Some(brainkeep::panel_query::INVALID) => StatusCode::BAD_REQUEST,
            Some(brainkeep::panel_query::NOT_FOUND) => StatusCode::NOT_FOUND,
            Some(brainkeep::panel_query::CHANGED) => StatusCode::CONFLICT,
            Some(brainkeep::access::INACTIVE) => StatusCode::UNAUTHORIZED,
            Some(brainkeep::access::DENIED) => StatusCode::FORBIDDEN,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        })
}

/// Runs a store job; `Err(None)` means busy or no answer in time.
async fn job(
    panel: &Panel,
    name: &str,
    args: Value,
    access: brainkeep::access::Access,
) -> Result<Value, Option<String>> {
    let (reply, rx) = oneshot::channel();
    let job = crate::mcp::Job {
        name: name.to_owned(),
        args,
        access: std::sync::Arc::new(access),
        reply,
    };
    if panel.jobs.try_send(job).is_err() {
        return Err(None);
    }
    match tokio::time::timeout(Duration::from_secs(15), rx).await {
        Ok(Ok(Ok(value))) => Ok(value),
        Ok(Ok(Err(message))) => Err(Some(message)),
        _ => Err(None),
    }
}

/// Session identity for store calls: a Google user's `panel:` principal or the admin.
fn session_access(user: &Option<(String, String)>) -> brainkeep::access::Access {
    match user {
        Some((handle, hash)) => brainkeep::access::Access::for_user(
            &format!("{}{hash}", brainkeep::store::PANEL_PRINCIPAL),
            handle,
        ),
        None => brainkeep::access::Access::local(),
    }
}

/// What a session may see: its handle (own projects, including new ones) and the
/// internal keys of every visible project.
struct Seen {
    me: String,
    keys: HashSet<String>,
}
impl Seen {
    fn sees(&self, key: &str) -> bool {
        self.keys.contains(key)
            || key
                .strip_prefix(self.me.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

/// `Err(true)`: the session ended (stop streaming); `Err(false)`: the store is busy
/// (keep the previous set).
async fn visible_keys(panel: &Panel, access: &brainkeep::access::Access) -> Result<Seen, bool> {
    let value = stream_query(panel, "visible", access).await?;
    let me = value["me"].as_str().ok_or(false)?.to_owned();
    let keys = value["keys"]
        .as_array()
        .ok_or(false)?
        .iter()
        .filter_map(|k| k.as_str().map(str::to_owned))
        .collect();
    Ok(Seen { me, keys })
}

/// Summary of everything the session can see (counts and last visible save). Comparing
/// it tells whether missed events touched this session, without revealing other writes.
async fn fingerprint(panel: &Panel, access: &brainkeep::access::Access) -> Result<Value, bool> {
    stream_query(panel, "status", access).await
}

/// Panel query for a live stream; never waits for queue room, so streams cannot crowd
/// out MCP. `Err(true)`: the session ended; `Err(false)`: the store is busy.
async fn stream_query(
    panel: &Panel,
    view: &str,
    access: &brainkeep::access::Access,
) -> Result<Value, bool> {
    let (reply, rx) = oneshot::channel();
    let job = crate::mcp::Job {
        name: format!("panel.{view}"),
        args: json!({}),
        access: Arc::new(access.clone()),
        reply,
    };
    if panel.jobs.try_send(job).is_err() {
        return Err(false);
    }
    match tokio::time::timeout(Duration::from_secs(15), rx).await {
        Ok(Ok(Ok(value))) => Ok(value),
        Ok(Ok(Err(message)))
            if message == brainkeep::access::INACTIVE || message == brainkeep::access::DENIED =>
        {
            Err(true)
        }
        _ => Err(false),
    }
}

/// Runs a store query for a stream unless it must stop first (shutdown, client gone,
/// cookie expired): `None` ends the stream.
async fn guarded<T>(
    stop: &CancellationToken,
    tx: &mpsc::Sender<Bytes>,
    expiry: tokio::time::Instant,
    query: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        _ = stop.cancelled() => None,
        _ = tx.closed() => None,
        _ = tokio::time::sleep_until(expiry) => None,
        result = query => Some(result),
    }
}

/// Releases a user's stream slot when the stream ends.
struct StreamSlot {
    table: Arc<Mutex<HashMap<String, usize>>>,
    user: String,
}
impl Drop for StreamSlot {
    fn drop(&mut self) {
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = table.get_mut(&self.user) {
            *count -= 1;
            if *count == 0 {
                table.remove(&self.user);
            }
        }
    }
}

struct Chunks(mpsc::Receiver<Bytes>);
impl futures_core::Stream for Chunks {
    type Item = Result<Bytes, std::convert::Infallible>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.0.poll_recv(cx).map(|chunk| chunk.map(Ok))
    }
}

/// `GET /panel/api/events`: Server-Sent Events carrying no data, only that a project the
/// session can see changed. Content changes are matched against the session's cached
/// visibility; membership changes (shares, users) and keep-alives refresh it, so a
/// revoked share stops its events and a disabled or logged-out user's stream ends.
async fn events(panel: &Panel, headers: &HeaderMap) -> Response {
    let Some((expires, user)) = panel.session(headers) else {
        return error(StatusCode::UNAUTHORIZED);
    };
    // Limits first: a refused stream costs no store work. Too many streams is a
    // client-side condition (tabs), not an outage: 429 like the per-user limit.
    let Ok(permit) = panel.streams.clone().try_acquire_owned() else {
        return error(StatusCode::TOO_MANY_REQUESTS);
    };
    // The stream ends when its cookie expires, whatever kind of session it is.
    let expiry = tokio::time::Instant::now() + Duration::from_secs(expires.saturating_sub(now()));
    let access = session_access(&user);
    let owner = user.map(|(handle, _)| handle).unwrap_or_default();
    let slot = {
        let mut table = panel.per_user.lock().unwrap_or_else(|e| e.into_inner());
        let count = table.entry(owner.clone()).or_insert(0);
        if *count >= MAX_STREAMS_PER_USER {
            return error(StatusCode::TOO_MANY_REQUESTS);
        }
        *count += 1;
        StreamSlot {
            table: panel.per_user.clone(),
            user: owner,
        }
    };
    // Subscribe before reading visibility so no write in between is missed.
    let mut changes = panel.changes.subscribe();
    let mut seen = match visible_keys(panel, &access).await {
        Ok(seen) => seen,
        Err(true) => return error(StatusCode::UNAUTHORIZED),
        Err(false) => return error(StatusCode::SERVICE_UNAVAILABLE),
    };
    let mut summary = match fingerprint(panel, &access).await {
        Ok(value) => value,
        Err(true) => return error(StatusCode::UNAUTHORIZED),
        Err(false) => return error(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (tx, rx) = mpsc::channel::<Bytes>(8);
    let stop = panel.shutdown.clone();
    let panel = panel.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let _slot = slot;
        // Never wait on a client that does not read: a full buffer ends its stream.
        if tx.try_send(Bytes::from_static(b"retry: 5000\n\n")).is_err() {
            return;
        }
        let mut tick = tokio::time::interval(KEEPALIVE);
        tick.tick().await;
        // Changes not yet notified, and whether visibility must be re-read before any
        // of them is (membership change, keep-alive, missed events). While the store is
        // busy nothing is sent: a stale view could name a revoked project.
        let mut pending: HashSet<String> = HashSet::new();
        let mut recheck = false;
        let mut lag = false;
        let mut retry_at: Option<tokio::time::Instant> = None;
        // `summary` is the baseline for missed events; after a notification it must be
        // retaken, or a later lag caused only by others' writes would differ from it.
        let mut baseline_stale = false;
        loop {
            let first = tokio::select! {
                _ = stop.cancelled() => break,
                _ = tx.closed() => break,
                _ = tokio::time::sleep_until(expiry) => break,
                _ = tick.tick() => {
                    if tx.try_send(Bytes::from_static(b": keep-alive\n\n")).is_err() {
                        break;
                    }
                    recheck = true;
                    None
                }
                _ = async {
                    match retry_at {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                } => None,
                received = changes.recv() => Some(received),
            };
            if let Some(first) = first {
                // Coalesce a burst of writes into one event.
                match first {
                    Ok((key, membership)) => {
                        recheck |= membership;
                        pending.insert(key);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => lag = true,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                }
                loop {
                    match changes.try_recv() {
                        Ok((key, membership)) => {
                            recheck |= membership;
                            pending.insert(key);
                        }
                        Err(broadcast::error::TryRecvError::Lagged(_)) => lag = true,
                        Err(_) => break,
                    }
                }
            }
            if pending.len() > 1024 {
                // Bounded memory: fall back to comparing what the session sees.
                pending.clear();
                lag = true;
            }
            if retry_at.is_some_and(|at| at > tokio::time::Instant::now()) {
                // The store was busy: keep the 2 s cadence even while writes arrive.
                continue;
            }
            let mut notify = false;
            if recheck || lag {
                let Some(result) = guarded(&stop, &tx, expiry, visible_keys(&panel, &access)).await
                else {
                    break;
                };
                match result {
                    Ok(found) => {
                        // A changed audience: tell the client to reload.
                        notify |= found.keys != seen.keys;
                        seen = found;
                        recheck = false;
                    }
                    Err(true) => break,
                    Err(false) => {
                        retry_at = Some(tokio::time::Instant::now() + Duration::from_secs(2));
                        continue;
                    }
                }
            }
            if lag {
                // Events were missed: notify only if what this session sees changed,
                // so a burst elsewhere reveals nothing.
                let Some(result) = guarded(&stop, &tx, expiry, fingerprint(&panel, &access)).await
                else {
                    break;
                };
                match result {
                    Ok(current) => {
                        // Without a valid baseline nothing can be told apart safely:
                        // take a new one and stay silent rather than reveal others' writes.
                        notify |= !baseline_stale && current != summary;
                        summary = current;
                        baseline_stale = false;
                        lag = false;
                    }
                    Err(true) => break,
                    Err(false) => {
                        retry_at = Some(tokio::time::Instant::now() + Duration::from_secs(2));
                        continue;
                    }
                }
            }
            retry_at = None;
            notify |= pending.iter().any(|key| seen.sees(key));
            pending.clear();
            if notify {
                if tx
                    .try_send(Bytes::from_static(b"event: change\ndata: {}\n\n"))
                    .is_err()
                {
                    break;
                }
                // New baseline for future missed events (see `baseline_stale`).
                let Some(result) = guarded(&stop, &tx, expiry, fingerprint(&panel, &access)).await
                else {
                    break;
                };
                match result {
                    Ok(current) => {
                        summary = current;
                        baseline_stale = false;
                    }
                    Err(true) => break,
                    Err(false) => baseline_stale = true,
                }
            }
        }
    });
    let mut response = secure(
        (
            [
                (header::CONTENT_TYPE, "text/event-stream"),
                (header::HeaderName::from_static("x-accel-buffering"), "no"),
            ],
            Body::from_stream(Chunks(rx)),
        )
            .into_response(),
        "no-store",
    );
    response.headers_mut().remove(header::CONTENT_LENGTH);
    response
}
