//! OAuth 2.1 authorization server for remote MCP clients (claude.ai web and mobile).
//! Brainkeep issues its own tokens; Google OIDC only proves the user's email. Pending logins
//! and authorization codes live in memory (a restart only interrupts in-flight logins);
//! clients and tokens persist in the store as SHA-256 hashes.
use crate::mcp::MemoryServer;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use brainkeep::access::Access;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const GOOGLE_AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];
const LOGIN_TTL: Duration = Duration::from_secs(600);
const CODE_TTL: Duration = Duration::from_secs(60);
/// Used codes are remembered this long so a late replay still revokes its tokens.
const USED_CODE_TTL: Duration = Duration::from_secs(600);
/// Distinct rate-limit buckets kept in memory; beyond it new clients share one bucket.
const MAX_BUCKETS: usize = 10000;
const MAX_PENDING: usize = 1000;

pub struct Config {
    client_id: String,
    client_secret: String,
    authorization_endpoint: String,
    token_endpoint: String,
}

/// Endpoint overrides exist for tests; they must be HTTPS or loopback HTTP.
fn valid_endpoint(url: &str) -> bool {
    let host_ok = |rest: &str| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        !host.is_empty() && !host.contains('@') && host.len() <= 253
    };
    url.len() <= 512
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
        && match url.strip_prefix("https://") {
            Some(rest) => host_ok(rest),
            None => url
                .strip_prefix("http://127.0.0.1:")
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit())),
        }
}

/// Loads `google.json`; any problem stops startup (fail closed).
pub fn load(path: &Path) -> Result<Config, String> {
    const INVALID: &str = "OAuth file requires \"client_id\" and \"client_secret\" (optional \"authorization_endpoint\", \"token_endpoint\": HTTPS)";
    brainkeep::files::check_file(path)?;
    let mut raw = Vec::new();
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "cannot open OAuth file")?
        .take(4097)
        .read_to_end(&mut raw)
        .map_err(|_| "cannot read OAuth file")?;
    if raw.len() > 4096 {
        return Err(INVALID.into());
    }
    let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(&raw) else {
        return Err(INVALID.into());
    };
    let known = [
        "client_id",
        "client_secret",
        "authorization_endpoint",
        "token_endpoint",
    ];
    if map.keys().any(|k| !known.contains(&k.as_str())) {
        return Err(INVALID.into());
    }
    let field = |key: &str, default: Option<&str>| -> Result<String, String> {
        match (map.get(key), default) {
            (Some(Value::String(v)), _)
                if !v.trim().is_empty() && v.len() <= 512 && !v.chars().any(char::is_control) =>
            {
                Ok(v.clone())
            }
            (None, Some(d)) => Ok(d.into()),
            _ => Err(INVALID.into()),
        }
    };
    let config = Config {
        client_id: field("client_id", None)?,
        client_secret: field("client_secret", None)?,
        authorization_endpoint: field("authorization_endpoint", Some(GOOGLE_AUTHORIZE))?,
        token_endpoint: field("token_endpoint", Some(GOOGLE_TOKEN))?,
    };
    if !valid_endpoint(&config.authorization_endpoint) || !valid_endpoint(&config.token_endpoint) {
        return Err(INVALID.into());
    }
    Ok(config)
}

/// Browser-binding cookie of a panel login; scoped to the Google callback path.
/// `__Host-` forbids `Domain=`, so a sibling subdomain cannot plant it (requires Path=/).
const PANEL_LOGIN_COOKIE: &str = "__Host-brainkeep_panel_login";
/// Share of the pending-login table panel sign-ins may hold, so they cannot crowd out
/// MCP client logins.
const MAX_PANEL_PENDING: usize = MAX_PENDING / 4;

struct Pending {
    /// Panel logins carry the SHA-256 of their browser-binding cookie; client fields
    /// are then empty. A callback without the matching cookie is rejected (login CSRF).
    panel: Option<String>,
    client_id: String,
    redirect_uri: String,
    challenge: String,
    client_state: String,
    nonce: String,
    verifier: String,
    created: Instant,
}
struct Code {
    client_id: String,
    redirect_uri: String,
    challenge: String,
    user_id: i64,
    family: String,
    created: Instant,
    used: bool,
    /// Set when the code is presented again; the legitimate exchange then revokes.
    replayed: bool,
}
/// Fixed-window counter per endpoint class and client address.
struct Window {
    start: Instant,
    count: u32,
}

pub struct OAuth {
    config: Config,
    base: String,
    store: MemoryServer,
    logins: Mutex<HashMap<String, Pending>>,
    codes: Mutex<HashMap<String, Code>>,
    limits: Mutex<HashMap<(&'static str, String), Window>>,
    panel: Option<crate::panel::Panel>,
}

fn random(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("operating system randomness");
    URL_SAFE_NO_PAD.encode(buf)
}
fn sha256_hex(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
/// PKCE verifier/challenge alphabet (RFC 7636): 43-128 unreserved characters.
fn pkce_value(v: &str) -> bool {
    (43..=128).contains(&v.len())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
}
/// HTTPS callbacks of the remote MCP clients Brainkeep serves. Registration is open, so any
/// other HTTPS destination would let a stranger's client receive a victim's code.
const ALLOWED_REDIRECTS: [&str; 3] = [
    "https://claude.ai/api/mcp/auth_callback",
    "https://claude.com/api/mcp/auth_callback",
    "https://chatgpt.com/connector_platform_oauth_redirect",
];
/// ChatGPT's per-connector callback: this prefix plus one opaque path segment.
const CHATGPT_CALLBACK: &str = "https://chatgpt.com/connector/oauth/";
/// Registered redirect URIs: the allowlisted HTTPS callbacks, or HTTP on loopback
/// (local clients such as Claude Code or Codex receive the code on the same machine).
fn valid_redirect(uri: &str) -> bool {
    if uri.len() > 512
        || uri.contains(['#', '\\'])
        || uri.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return false;
    }
    // Userinfo (`user@host`) would hide the real host, so it is never accepted.
    let authority = |rest: &str| rest.split(['/', '?']).next().unwrap_or("").to_owned();
    if ALLOWED_REDIRECTS.contains(&uri) {
        return true;
    }
    if let Some(id) = uri.strip_prefix(CHATGPT_CALLBACK) {
        return (1..=128).contains(&id.len())
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    }
    if let Some(rest) = uri.strip_prefix("http://") {
        let a = authority(rest);
        if a.contains('@') {
            return false;
        }
        let host = if a == "[::1]" || a.starts_with("[::1]:") {
            "[::1]"
        } else {
            a.split(':').next().unwrap_or("")
        };
        return ["127.0.0.1", "localhost", "[::1]"].contains(&host);
    }
    false
}
fn with_query(uri: &str, pairs: &[(&str, &str)]) -> String {
    let mut q = form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        q.append_pair(k, v);
    }
    let sep = if uri.contains('?') { '&' } else { '?' };
    format!("{uri}{sep}{}", q.finish())
}
fn no_store(mut r: Response) -> Response {
    r.headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    r
}
fn page(code: StatusCode, message: &str) -> Response {
    no_store(
        (
            code,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            message.to_owned(),
        )
            .into_response(),
    )
}
fn redirect(location: &str) -> Response {
    no_store((StatusCode::FOUND, [(header::LOCATION, location.to_owned())]).into_response())
}
fn token_error(error: &str) -> Response {
    let status = if error == "invalid_client" {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(json!({"error":error}))).into_response())
}
fn too_many() -> Response {
    no_store(StatusCode::TOO_MANY_REQUESTS.into_response())
}

impl OAuth {
    pub fn new(
        config: Config,
        host: &str,
        store: MemoryServer,
        panel: Option<crate::panel::Panel>,
    ) -> Self {
        Self {
            config,
            base: format!("https://{host}"),
            store,
            logins: Mutex::new(HashMap::new()),
            codes: Mutex::new(HashMap::new()),
            limits: Mutex::new(HashMap::new()),
            panel,
        }
    }
    /// Records a pending login and returns the redirect to Google's consent screen.
    fn google_start(&self, mut pending: Pending) -> Response {
        let google_state = random(32);
        pending.nonce = random(32);
        pending.verifier = random(48);
        let location = with_query(
            &self.config.authorization_endpoint,
            &[
                ("client_id", &self.config.client_id),
                (
                    "redirect_uri",
                    &format!("{}/oauth/google/callback", self.base),
                ),
                ("response_type", "code"),
                ("scope", "openid email profile"),
                ("state", &google_state),
                ("nonce", &pending.nonce),
                ("code_challenge", &s256(&pending.verifier)),
                ("code_challenge_method", "S256"),
                ("prompt", "select_account"),
            ],
        );
        let mut logins = self.logins.lock().unwrap();
        logins.retain(|_, p| p.created.elapsed() < LOGIN_TTL);
        if logins.len() >= MAX_PENDING
            || pending.panel.is_some()
                && logins.values().filter(|p| p.panel.is_some()).count() >= MAX_PANEL_PENDING
        {
            drop(logins);
            return too_many();
        }
        logins.insert(google_state, pending);
        drop(logins);
        redirect(&location)
    }
    /// Exchanges Google's code and returns the invited, enabled user it proves.
    async fn google_user(&self, code: &str, p: &Pending) -> Result<Value, &'static str> {
        let config = Config {
            client_id: self.config.client_id.clone(),
            client_secret: self.config.client_secret.clone(),
            authorization_endpoint: self.config.authorization_endpoint.clone(),
            token_endpoint: self.config.token_endpoint.clone(),
        };
        let (code, verifier, nonce) = (code.to_owned(), p.verifier.clone(), p.nonce.clone());
        let google_redirect = format!("{}/oauth/google/callback", self.base);
        let claims = tokio::task::spawn_blocking(move || {
            google_claims(&config, &code, &verifier, &google_redirect, &nonce)
        })
        .await;
        let Ok(Ok(claims)) = claims else {
            return Err("Google sign-in could not be verified");
        };
        let (Some(email), Some(sub)) = (
            claims.get("email").and_then(Value::as_str),
            claims.get("sub").and_then(Value::as_str),
        ) else {
            return Err("Google sign-in could not be verified");
        };
        let user = self
            .store(
                "oauth.login",
                json!({"email":email,"sub":sub,"name":claims.get("name"),"picture":claims.get("picture")}),
            )
            .await
            .map_err(|_| "This email is not enabled for Brainkeep")?;
        if user["user_id"].as_i64().is_none() || user["handle"].as_str().is_none() {
            return Err("Google sign-in could not be verified");
        }
        Ok(user)
    }
    pub fn resource(&self) -> String {
        format!("{}/mcp", self.base)
    }
    /// `WWW-Authenticate` for /mcp: points clients at the protected resource metadata.
    pub fn challenge(&self) -> String {
        format!(
            "Bearer realm=\"brainkeep\", resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"",
            self.base
        )
    }
    /// Fixed window per endpoint class and client. Cloudflare overwrites
    /// `CF-Connecting-IP` at its edge and the origin listens on loopback, so the header
    /// identifies remote clients (IPv6 grouped by /64, what one host usually controls);
    /// without it they share one bucket. A full table evicts its oldest window, so
    /// churning addresses can only reset limits, never lock others out.
    fn allow(&self, class: &'static str, headers: &HeaderMap) -> bool {
        let (max, window) = match class {
            "register" => (20, Duration::from_secs(600)),
            "token" => (120, Duration::from_secs(60)),
            _ => (60, Duration::from_secs(60)),
        };
        let mut values = headers.get_all("cf-connecting-ip").iter();
        let client = match (values.next().and_then(|v| v.to_str().ok()), values.next()) {
            (Some(ip), None) => match ip.parse::<std::net::IpAddr>() {
                Ok(std::net::IpAddr::V6(v6)) => {
                    let s = v6.segments();
                    format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
                }
                Ok(v4) => v4.to_string(),
                Err(_) => String::new(),
            },
            _ => String::new(),
        };
        let mut limits = self.limits.lock().unwrap_or_else(|e| e.into_inner());
        let key = (class, client);
        if limits.len() >= MAX_BUCKETS && !limits.contains_key(&key) {
            limits.retain(|_, w| w.start.elapsed() <= Duration::from_secs(600));
            if limits.len() >= MAX_BUCKETS
                && let Some(oldest) = limits
                    .iter()
                    .min_by_key(|(_, w)| w.start)
                    .map(|(k, _)| k.clone())
            {
                limits.remove(&oldest);
            }
        }
        let w = limits.entry(key).or_insert(Window {
            start: Instant::now(),
            count: 0,
        });
        if w.start.elapsed() > window {
            *w = Window {
                start: Instant::now(),
                count: 0,
            };
        }
        w.count += 1;
        w.count <= max
    }
    async fn store(&self, name: &str, args: Value) -> Result<Value, String> {
        self.store.internal(name, args).await
    }
    /// Access for a Brainkeep-issued bearer token, if valid and its user is enabled.
    /// `Err` means the store could not answer (busy), not that the token is invalid.
    pub async fn bearer(&self, token: &str) -> Result<Option<Arc<Access>>, ()> {
        if !token.starts_with("sba_") || token.len() > 128 {
            return Ok(None);
        }
        let found = match self
            .store(
                "oauth.access",
                json!({"hash":sha256_hex(token),"now":now()}),
            )
            .await
        {
            Ok(found) => found,
            Err(e) if e == brainkeep::access::INACTIVE => return Ok(None),
            Err(_) => return Err(()),
        };
        let (Some(handle), Some(client)) = (found["handle"].as_str(), found["client_id"].as_str())
        else {
            return Ok(None);
        };
        // Idempotency receipts are namespaced per client *and* user: a client_id is
        // public, so several people may share one.
        Ok(Some(Arc::new(Access::for_user(
            &format!("oauth-{client}:{handle}"),
            handle,
        ))))
    }
    pub fn routes(self: Arc<Self>) -> Router {
        Router::new()
            .route(
                "/.well-known/oauth-protected-resource",
                get(resource_metadata),
            )
            .route(
                "/.well-known/oauth-protected-resource/mcp",
                get(resource_metadata),
            )
            .route(
                "/.well-known/oauth-authorization-server",
                get(server_metadata),
            )
            .route("/oauth/register", post(register))
            .route("/oauth/authorize", get(authorize))
            .route("/oauth/google/callback", get(callback))
            .route("/oauth/panel/start", get(panel_start))
            .route("/oauth/token", post(token))
            .layer(DefaultBodyLimit::max(16384))
            .with_state(self)
    }
}

async fn resource_metadata(State(o): State<Arc<OAuth>>) -> Response {
    no_store(
        Json(json!({
            "resource": o.resource(),
            "authorization_servers": [o.base],
            "bearer_methods_supported": ["header"],
            "resource_name": "Brainkeep",
        }))
        .into_response(),
    )
}

async fn server_metadata(State(o): State<Arc<OAuth>>) -> Response {
    let b = &o.base;
    no_store(
        Json(json!({
            "issuer": b,
            "authorization_endpoint": format!("{b}/oauth/authorize"),
            "token_endpoint": format!("{b}/oauth/token"),
            "registration_endpoint": format!("{b}/oauth/register"),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
            "authorization_response_iss_parameter_supported": true,
        }))
        .into_response(),
    )
}

/// Dynamic client registration (RFC 7591) for public clients only.
async fn register(State(o): State<Arc<OAuth>>, headers: HeaderMap, body: Bytes) -> Response {
    if !o.allow("register", &headers) {
        return too_many();
    }
    let invalid = |d: &str| {
        no_store(
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_client_metadata","error_description":d})),
            )
                .into_response(),
        )
    };
    let Ok(Value::Object(meta)) = serde_json::from_slice::<Value>(&body) else {
        return invalid("JSON object required");
    };
    let uris: Vec<String> = match meta.get("redirect_uris") {
        Some(Value::Array(items)) if (1..=5).contains(&items.len()) => {
            let uris: Vec<String> = items
                .iter()
                .filter_map(|v| v.as_str().filter(|u| valid_redirect(u)).map(str::to_owned))
                .collect();
            if uris.len() != items.len() {
                return invalid(
                    "redirect_uris must be a supported client callback or loopback HTTP",
                );
            }
            uris
        }
        _ => return invalid("1-5 redirect_uris required"),
    };
    let subset = |key: &str, allowed: &[&str]| match meta.get(key) {
        None => true,
        Some(Value::Array(v)) => v
            .iter()
            .all(|x| x.as_str().is_some_and(|s| allowed.contains(&s))),
        _ => false,
    };
    if !subset("grant_types", &["authorization_code", "refresh_token"])
        || !subset("response_types", &["code"])
        || meta
            .get("token_endpoint_auth_method")
            .is_some_and(|m| m != "none")
    {
        return invalid("only public clients using authorization_code with PKCE");
    }
    let name: String = meta
        .get("client_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(128)
        .collect();
    let client_id = format!("sbclient_{}", random(18));
    if o.store(
        "oauth.register",
        json!({"client_id":client_id,"redirect_uris":uris,"client_name":name}),
    )
    .await
    .is_err()
    {
        return no_store(StatusCode::SERVICE_UNAVAILABLE.into_response());
    }
    no_store(
        (
            StatusCode::CREATED,
            Json(json!({
                "client_id": client_id,
                "client_id_issued_at": now(),
                "client_name": name,
                "redirect_uris": uris,
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            })),
        )
            .into_response(),
    )
}

async fn authorize(
    State(o): State<Arc<OAuth>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !o.allow("authorize", &headers) {
        return too_many();
    }
    let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
    let (client_id, redirect_uri) = (get("client_id"), get("redirect_uri"));
    // Until client and redirect are verified, errors must not redirect anywhere.
    let known = match o
        .store("oauth.client", json!({"client_id":client_id}))
        .await
    {
        Ok(c) => {
            valid_redirect(redirect_uri)
                && c["redirect_uris"]
                    .as_array()
                    .is_some_and(|u| u.iter().any(|v| v == redirect_uri))
        }
        Err(_) => false,
    };
    if client_id.is_empty() || redirect_uri.is_empty() || !known {
        return page(StatusCode::BAD_REQUEST, "Unknown client or redirect URI.");
    }
    let state = get("state");
    let fail = |error: &str| {
        redirect(&with_query(
            redirect_uri,
            &[("error", error), ("state", state), ("iss", &o.base)],
        ))
    };
    if get("response_type") != "code" {
        return fail("unsupported_response_type");
    }
    if get("code_challenge_method") != "S256"
        || !pkce_value(get("code_challenge"))
        || state.is_empty()
        || state.len() > 512
        || q.get("resource").is_some_and(|r| *r != o.resource())
    {
        return fail("invalid_request");
    }
    o.google_start(Pending {
        panel: None,
        client_id: client_id.into(),
        redirect_uri: redirect_uri.into(),
        challenge: get("code_challenge").into(),
        client_state: state.into(),
        nonce: String::new(),
        verifier: String::new(),
        created: Instant::now(),
    })
}

/// Starts a panel sign-in: binds the pending login to this browser with a short-lived
/// cookie that only the Google callback path receives.
async fn panel_start(State(o): State<Arc<OAuth>>, headers: HeaderMap) -> Response {
    if o.panel.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !o.allow("authorize", &headers) {
        return too_many();
    }
    let binding = random(32);
    let mut response = o.google_start(Pending {
        panel: Some(sha256_hex(&binding)),
        client_id: String::new(),
        redirect_uri: String::new(),
        challenge: String::new(),
        client_state: String::new(),
        nonce: String::new(),
        verifier: String::new(),
        created: Instant::now(),
    });
    if response.status() == StatusCode::FOUND
        && let Ok(value) = format!(
            "{PANEL_LOGIN_COOKIE}={binding}; Path=/; Max-Age={}; HttpOnly; Secure; SameSite=Lax",
            LOGIN_TTL.as_secs()
        )
        .parse()
    {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

/// Value of a cookie that must appear exactly once.
fn single_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE) {
        for pair in value.to_str().ok()?.split(';') {
            if let Some(v) = pair
                .trim()
                .strip_prefix(name)
                .and_then(|r| r.strip_prefix('='))
            {
                if found.is_some() {
                    return None;
                }
                found = Some(v.to_owned());
            }
        }
    }
    found
}

/// Ends a panel sign-in. Errors show a page and never create a session.
async fn panel_finish(
    o: &OAuth,
    headers: &HeaderMap,
    q: &HashMap<String, String>,
    p: Pending,
) -> Response {
    let clear = format!("{PANEL_LOGIN_COOKIE}=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Lax");
    let fail = |message: &str| {
        let mut r = page(
            StatusCode::FORBIDDEN,
            &format!("{message}. Return to {}/panel to try again.", o.base),
        );
        if let Ok(v) = clear.parse() {
            r.headers_mut().insert(header::SET_COOKIE, v);
        }
        r
    };
    let bound =
        single_cookie(headers, PANEL_LOGIN_COOKIE).is_some_and(|c| Some(sha256_hex(&c)) == p.panel);
    if !bound {
        return fail("This sign-in was not started in this browser");
    }
    let (Some(code), Some(panel)) = (q.get("code").filter(|_| !q.contains_key("error")), &o.panel)
    else {
        return fail("Google sign-in was cancelled");
    };
    let user = match o.google_user(code, &p).await {
        Ok(user) => user,
        Err(message) => return fail(message),
    };
    let handle = user["handle"].as_str().unwrap_or_default();
    let (session, hash) = panel.login_cookie(handle);
    if o.store(
        "oauth.panel_session",
        json!({"user_id":user["user_id"],"hash":hash,"now":now()}),
    )
    .await
    .is_err()
    {
        return fail("Brainkeep could not start the session");
    }
    // A same-origin refresh (not a redirect) so the SameSite=Strict session cookie is
    // sent on the first panel load after Google's cross-site navigation.
    let mut r = no_store(
        (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::REFERRER_POLICY, "no-referrer"),
                (
                    header::CONTENT_SECURITY_POLICY,
                    "default-src 'none'; frame-ancestors 'none'",
                ),
            ],
            "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"0;url=/panel/\"><title>Brainkeep</title><a href=\"/panel/\">Brainkeep</a>",
        )
            .into_response(),
    );
    let cookies = r.headers_mut();
    cookies.append(header::SET_COOKIE, session);
    if let Ok(v) = clear.parse() {
        cookies.append(header::SET_COOKIE, v);
    }
    r
}

/// Exchanges Google's code over TLS and returns the verified ID token claims. The
/// token comes directly from Google's token endpoint over TLS, so issuer validation
/// relies on TLS (OIDC Core 3.1.3.7); audience, expiry and nonce are still checked.
fn google_claims(
    config: &Config,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    nonce: &str,
) -> Result<Map<String, Value>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .post(&config.token_endpoint)
        .send_form([
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
        ])
        .map_err(|_| "Google token request failed")?;
    if response.status() != 200 {
        return Err("Google rejected the code".into());
    }
    let body: Value = response
        .body_mut()
        .with_config()
        .limit(65536)
        .read_json()
        .map_err(|_| "invalid Google response")?;
    let id_token = body["id_token"].as_str().ok_or("missing ID token")?;
    let payload = id_token.split('.').nth(1).ok_or("invalid ID token")?;
    let claims: Map<String, Value> = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(payload.trim_end_matches('='))
            .map_err(|_| "invalid ID token")?,
    )
    .map_err(|_| "invalid ID token")?;
    let aud_ok = match &claims.get("aud") {
        Some(Value::String(a)) => *a == config.client_id,
        Some(Value::Array(a)) => {
            a.iter().any(|v| *v == config.client_id)
                && (a.len() == 1
                    || claims.get("azp").and_then(Value::as_str) == Some(&config.client_id))
        }
        _ => false,
    };
    let verified = matches!(claims.get("email_verified"), Some(Value::Bool(true)))
        || claims.get("email_verified").and_then(Value::as_str) == Some("true");
    let iss_ok = claims
        .get("iss")
        .and_then(Value::as_str)
        .is_some_and(|i| GOOGLE_ISSUERS.contains(&i));
    let exp_ok = claims
        .get("exp")
        .and_then(Value::as_i64)
        .is_some_and(|e| e > now());
    let nonce_ok = claims.get("nonce").and_then(Value::as_str) == Some(nonce);
    if !(aud_ok && verified && iss_ok && exp_ok && nonce_ok) {
        return Err("ID token rejected".into());
    }
    Ok(claims)
}

async fn callback(
    State(o): State<Arc<OAuth>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !o.allow("callback", &headers) {
        return too_many();
    }
    let state = q.get("state").map(String::as_str).unwrap_or("");
    let pending = o.logins.lock().unwrap().remove(state);
    let Some(p) = pending.filter(|p| p.created.elapsed() < LOGIN_TTL) else {
        return page(
            StatusCode::BAD_REQUEST,
            "Login expired or invalid. Start again from your client.",
        );
    };
    if p.panel.is_some() {
        return panel_finish(&o, &headers, &q, p).await;
    }
    let deny = |description: &str| {
        redirect(&with_query(
            &p.redirect_uri,
            &[
                ("error", "access_denied"),
                ("error_description", description),
                ("state", &p.client_state),
                ("iss", &o.base),
            ],
        ))
    };
    let Some(code) = q.get("code").filter(|_| !q.contains_key("error")) else {
        return deny("Google sign-in was cancelled");
    };
    let user = match o.google_user(code, &p).await {
        Ok(user) => user,
        Err(message) => return deny(message),
    };
    let Some(user_id) = user["user_id"].as_i64() else {
        return deny("Google sign-in could not be verified");
    };
    let code = format!("sbc_{}", random(32));
    let mut codes = o.codes.lock().unwrap();
    codes.retain(|_, c| c.created.elapsed() < if c.used { USED_CODE_TTL } else { CODE_TTL });
    if codes.len() >= MAX_PENDING {
        drop(codes);
        return too_many();
    }
    codes.insert(
        sha256_hex(&code),
        Code {
            client_id: p.client_id.clone(),
            redirect_uri: p.redirect_uri.clone(),
            challenge: p.challenge.clone(),
            user_id,
            family: sha256_hex(&random(32)),
            created: Instant::now(),
            used: false,
            replayed: false,
        },
    );
    drop(codes);
    redirect(&with_query(
        &p.redirect_uri,
        &[
            ("code", &code),
            ("state", &p.client_state),
            ("iss", &o.base),
        ],
    ))
}

async fn token(State(o): State<Arc<OAuth>>, headers: HeaderMap, body: Bytes) -> Response {
    if !o.allow("token", &headers) {
        return too_many();
    }
    // Form body; repeated parameters are ambiguous and rejected (RFC 6749 3.2).
    let mut f = HashMap::new();
    for (k, v) in form_urlencoded::parse(&body) {
        if f.insert(k.into_owned(), v.into_owned()).is_some() {
            return token_error("invalid_request");
        }
    }
    let get = |k: &str| f.get(k).map(String::as_str).unwrap_or("");
    if f.get("resource").is_some_and(|r| *r != o.resource()) {
        return token_error("invalid_target");
    }
    let client_id = get("client_id");
    if client_id.is_empty() {
        return token_error("invalid_client");
    }
    let access = format!("sba_{}", random(32));
    let refresh = format!("sbr_{}", random(32));
    let issued = match get("grant_type") {
        "authorization_code" => {
            let code_hash = sha256_hex(get("code"));
            let grant = {
                let mut codes = o.codes.lock().unwrap();
                match codes.get_mut(&code_hash) {
                    None => Err(None),
                    Some(c) if c.used => {
                        c.replayed = true;
                        Err(Some(c.family.clone()))
                    }
                    Some(c) => {
                        let ok = c.created.elapsed() < CODE_TTL
                            && c.client_id == client_id
                            && c.redirect_uri == get("redirect_uri")
                            && pkce_value(get("code_verifier"))
                            && s256(get("code_verifier")) == c.challenge;
                        c.used = true;
                        if ok {
                            Ok((c.user_id, c.family.clone()))
                        } else {
                            Err(None)
                        }
                    }
                }
            };
            match grant {
                Ok((user_id, family)) => {
                    let issued = o
                        .store(
                            "oauth.issue",
                            json!({"user_id":user_id,"client_id":client_id,"family":family,
                                   "access":sha256_hex(&access),"refresh":sha256_hex(&refresh),"now":now()}),
                        )
                        .await;
                    // A replay that raced this exchange may have revoked before the
                    // issue ran; check again so its tokens never survive.
                    let replayed = o
                        .codes
                        .lock()
                        .unwrap()
                        .get(&code_hash)
                        .is_some_and(|c| c.replayed);
                    if replayed {
                        let _ = o.store("oauth.revoke", json!({"family":family})).await;
                        return token_error("invalid_grant");
                    }
                    issued
                }
                Err(reused) => {
                    // A replayed code may have leaked: revoke what it produced.
                    if let Some(family) = reused {
                        let _ = o.store("oauth.revoke", json!({"family":family})).await;
                    }
                    return token_error("invalid_grant");
                }
            }
        }
        "refresh_token" => {
            let old = get("refresh_token");
            if !old.starts_with("sbr_") || old.len() > 128 {
                return token_error("invalid_grant");
            }
            o.store(
                "oauth.refresh",
                json!({"refresh":sha256_hex(old),"client_id":client_id,
                       "access":sha256_hex(&access),"new_refresh":sha256_hex(&refresh),"now":now()}),
            )
            .await
        }
        _ => return token_error("unsupported_grant_type"),
    };
    match issued {
        Ok(v) => no_store(
            Json(json!({
                "access_token": access,
                "token_type": "Bearer",
                "expires_in": v["expires_in"],
                "refresh_token": refresh,
            }))
            .into_response(),
        ),
        Err(_) => token_error("invalid_grant"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redirect_and_endpoint_rules() {
        for ok in [
            "https://claude.ai/api/mcp/auth_callback",
            "https://claude.com/api/mcp/auth_callback",
            "https://chatgpt.com/connector_platform_oauth_redirect",
            "https://chatgpt.com/connector/oauth/Ab_9-x",
            "http://127.0.0.1:3000/cb",
            "http://localhost/cb",
            "http://[::1]:8080/cb",
        ] {
            assert!(valid_redirect(ok), "{ok}");
        }
        for bad in [
            "http://claude.ai/cb",
            "https://evil.test/cb",
            "https://claude.ai/api/mcp/auth_callback?x=1",
            "https://claude.ai.evil.test/api/mcp/auth_callback",
            "https://chatgpt.com/connector/oauth/",
            "https://chatgpt.com/connector/oauth/a/b",
            "https://",
            "https://a@b.test/cb",
            "https://x.test/cb#frag",
            "javascript:alert(1)",
            "http://localhost.evil.test/cb",
            "https://x.test/ cb",
            "http://localhost:80@evil.test/cb",
            "http://127.0.0.1:1@evil.test/cb",
            "http://[::1]@evil.test/cb",
            "http://[::1]evil.test/cb",
            "http://[::1].evil.test/cb",
            "https://evil.test\\@claude.ai/cb",
        ] {
            assert!(!valid_redirect(bad), "{bad}");
        }
        assert!(valid_endpoint(GOOGLE_TOKEN) && valid_endpoint("http://127.0.0.1:9/t"));
        assert!(!valid_endpoint("http://example.test/t") && !valid_endpoint("ftp://x"));
    }
    #[test]
    fn pkce_rules() {
        let v = "a".repeat(43);
        assert!(pkce_value(&v) && !pkce_value(&"a".repeat(42)) && !pkce_value(&"a+".repeat(30)));
        // RFC 7636 appendix B test vector.
        assert_eq!(
            s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(
            with_query("https://x.test/cb?a=1", &[("state", "a b&c")]),
            "https://x.test/cb?a=1&state=a+b%26c"
        );
    }
}
