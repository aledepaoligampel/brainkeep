//! Private origin behind a TLS reverse proxy. Static bearer tokens, optional OAuth
//! authorization server for remote MCP clients, optional read-only panel.
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::any,
};
use brainkeep::access::Access;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::{net::TcpListener, sync::Semaphore};

pub struct Prepared {
    listener: TcpListener,
    credentials: Vec<(Vec<u8>, Arc<Access>)>,
    host: String,
    panel: Option<crate::panel::Credentials>,
    oauth: Option<crate::oauth::Config>,
}

pub async fn prepare(
    listen: std::net::IpAddr,
    port: u16,
    auth: &Path,
    host: String,
    panel: Option<&Path>,
    oauth: Option<&Path>,
) -> Result<Prepared, String> {
    if port == 0
        || host.is_empty()
        || host.len() > 253
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return Err("HTTP needs a nonzero port and DNS hostname".into());
    }
    brainkeep::files::check_file(auth)?;
    let mut raw = Vec::new();
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(auth)
        .map_err(|_| "cannot open authentication file for reading")?
        .take(8193)
        .read_to_end(&mut raw)
        .map_err(|_| "cannot read authentication file")?;
    if raw.len() > 8192 {
        return Err("invalid authentication file".into());
    }
    let credentials: Vec<(Vec<u8>, Arc<Access>)> = brainkeep::access::parse_auth(&raw)?
        .into_iter()
        .map(|(token, access)| (token, Arc::new(access)))
        .collect();
    let tokens: Vec<Vec<u8>> = credentials.iter().map(|(t, _)| t.clone()).collect();
    let panel = match panel {
        Some(path) => Some(crate::panel::load(path, &tokens)?),
        None => None,
    };
    let oauth = match oauth {
        Some(path) => Some(crate::oauth::load(path)?),
        None => None,
    };
    let listener = TcpListener::bind((listen, port))
        .await
        .map_err(|_| "cannot bind HTTP port")?;
    Ok(Prepared {
        listener,
        credentials,
        host,
        panel,
        oauth,
    })
}

/// Concurrent /mcp requests in total and per user: one slow or misbehaving client
/// (e.g. holding incomplete bodies) fills only its user's share, however many
/// credentials or OAuth clients that user has. Six users would be needed to fill
/// the server.
const SLOTS: usize = 32;
const SLOTS_PER_USER: usize = 6;
/// Time to receive a request body; the whole request still has 15 s.
const BODY_TIMEOUT: Duration = Duration::from_secs(5);

/// One in-flight request of a credential; releases its share when dropped.
struct Share {
    active: Arc<Mutex<HashMap<String, usize>>>,
    key: String,
}
impl Share {
    fn take(active: &Arc<Mutex<HashMap<String, usize>>>, key: &str) -> Option<Self> {
        let mut table = active.lock().unwrap_or_else(|e| e.into_inner());
        let n = table.entry(key.to_owned()).or_insert(0);
        if *n >= SLOTS_PER_USER {
            return None;
        }
        *n += 1;
        Some(Self {
            active: active.clone(),
            key: key.to_owned(),
        })
    }
}
impl Drop for Share {
    fn drop(&mut self) {
        let mut table = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = table.get_mut(&self.key) {
            *n -= 1;
            if *n == 0 {
                table.remove(&self.key);
            }
        }
    }
}

#[derive(Clone)]
struct Guard {
    credentials: Arc<Vec<(Vec<u8>, Arc<Access>)>>,
    slots: Arc<Semaphore>,
    active: Arc<Mutex<HashMap<String, usize>>>,
    store: crate::mcp::MemoryServer,
    oauth: Option<Arc<crate::oauth::OAuth>>,
}

fn reject(code: StatusCode) -> Response {
    let mut response = code.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
/// 401 that tells OAuth-capable clients where to discover the authorization server.
fn unauthorized(guard: &Guard) -> Response {
    let mut response = reject(StatusCode::UNAUTHORIZED);
    let challenge = match &guard.oauth {
        Some(o) => o.challenge(),
        None => "Bearer realm=\"brainkeep\"".into(),
    };
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, challenge.parse().unwrap());
    response
}

async fn protect(State(guard): State<Guard>, mut request: Request, next: Next) -> Response {
    let mut headers = request.headers().get_all(header::AUTHORIZATION).iter();
    let supplied = headers
        .next()
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("")
        .as_bytes();
    // Compare against every credential; tokens are unique, so at most one matches.
    let mut matched = None;
    for (token, access) in guard.credentials.iter() {
        if bool::from(supplied.ct_eq(token)) {
            matched = Some(access.clone());
        }
    }
    let duplicated = headers.next().is_some();
    if duplicated {
        return unauthorized(&guard);
    }
    // Personal tokens issued from the panel: looked up by SHA-256, with or without OAuth.
    if matched.is_none()
        && supplied.len() == 68
        && let Ok(token) = std::str::from_utf8(supplied)
        && let Some(secret) = token.strip_prefix(brainkeep::store::PERSONAL_PREFIX)
        && secret
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        use sha2::{Digest, Sha256};
        let digest: String = Sha256::digest(supplied)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        match guard
            .store
            .internal("oauth.personal", serde_json::json!({"hash":digest}))
            .await
        {
            Ok(found) => {
                if let Some(handle) = found["handle"]
                    .as_str()
                    .filter(|h| brainkeep::access::valid_handle(h))
                {
                    // Receipts are namespaced per user, apart from auth.json and OAuth.
                    matched = Some(Arc::new(Access::for_user(
                        &format!("personal:{handle}"),
                        handle,
                    )));
                }
            }
            Err(e) if e == brainkeep::access::INACTIVE => {}
            Err(_) => return reject(StatusCode::SERVICE_UNAVAILABLE),
        }
    }
    if matched.is_none()
        && let Some(oauth) = &guard.oauth
        && let Ok(token) = std::str::from_utf8(supplied)
    {
        match oauth.bearer(token).await {
            Ok(found) => matched = found,
            Err(()) => return reject(StatusCode::SERVICE_UNAVAILABLE),
        }
    }
    let Some(access) = matched else {
        return unauthorized(&guard);
    };
    // A static token stays valid only while its user is invited and enabled.
    match guard.store.check(access.clone()).await {
        Ok(true) => {}
        Ok(false) => return unauthorized(&guard),
        Err(()) => return reject(StatusCode::SERVICE_UNAVAILABLE),
    }
    // Shares count per user: every credential and OAuth client of a user (`user()`);
    // credentials without a user (legacy admin tokens) count on their own.
    let share_key = match access.user() {
        Some(user) => format!("user:{user}"),
        None => format!("credential:{}", access.principal()),
    };
    // Server-side identity for MCP handlers; never derived from tool arguments.
    request.extensions_mut().insert(access);
    // Reject ambiguous authorities/origins before the SDK inspects a single value.
    if request.headers().get_all(header::HOST).iter().count() != 1
        || request.headers().get_all(header::ORIGIN).iter().count() > 1
    {
        return reject(StatusCode::FORBIDDEN);
    }
    let Some(_share) = Share::take(&guard.active, &share_key) else {
        let mut response = reject(StatusCode::TOO_MANY_REQUESTS);
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, "1".parse().unwrap());
        return response;
    };
    let Ok(_permit) = guard.slots.try_acquire() else {
        return reject(StatusCode::SERVICE_UNAVAILABLE);
    };
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let (parts, body) = request.into_parts();
        let bytes = match tokio::time::timeout(BODY_TIMEOUT, to_bytes(body, 65536)).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(_)) => return reject(StatusCode::PAYLOAD_TOO_LARGE),
            Err(_) => return reject(StatusCode::REQUEST_TIMEOUT),
        };
        let response = next
            .run(Request::from_parts(parts, Body::from(bytes)))
            .await;
        let (mut parts, body) = response.into_parts();
        parts
            .headers
            .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        let Ok(bytes) = to_bytes(body, 4 * 1024 * 1024).await else {
            return reject(StatusCode::BAD_GATEWAY);
        };
        Response::from_parts(parts, Body::from(bytes))
    })
    .await;
    result.unwrap_or_else(|_| reject(StatusCode::GATEWAY_TIMEOUT))
}

pub fn start(
    prepared: Prepared,
    mut api: crate::mcp::MemoryServer,
) -> (
    tokio_util::sync::CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    api.allow_admin = false;
    let api_for_guard = api.clone();
    // Live panel streams end when the server shuts down, so graceful shutdown never waits on them.
    let config_cancel = tokio_util::sync::CancellationToken::new();
    let panel = prepared.panel.map(|c| {
        crate::panel::Panel::new(
            c,
            api.jobs.clone(),
            prepared.host.clone(),
            prepared.oauth.is_some(),
            api.changes.clone(),
            config_cancel.clone(),
        )
    });
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(65536)
        .with_allowed_hosts([prepared.host.clone()])
        .with_allowed_origins([format!("https://{}:443", prepared.host)])
        .enforce_origin_validation();
    let cancel = config.cancellation_token.clone();
    let stream_stop = config_cancel.clone();
    let watched = cancel.clone();
    tokio::spawn(async move {
        watched.cancelled().await;
        stream_stop.cancel();
    });
    let service = StreamableHttpService::new(
        move || Ok(api.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let oauth = prepared.oauth.map(|config| {
        Arc::new(crate::oauth::OAuth::new(
            config,
            &prepared.host,
            api_for_guard.clone(),
            panel.clone(),
        ))
    });
    let guard = Guard {
        credentials: Arc::new(prepared.credentials),
        slots: Arc::new(Semaphore::new(SLOTS)),
        active: Arc::new(Mutex::new(HashMap::new())),
        store: api_for_guard,
        oauth: oauth.clone(),
    };
    let mut app = Router::new()
        .route_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(guard, protect));
    if let Some(oauth) = oauth {
        app = app.merge(oauth.routes());
    }
    if let Some(panel) = panel {
        app = app.merge(
            Router::new()
                .route("/panel", any(crate::panel::handle))
                .route("/panel/", any(crate::panel::handle))
                .route("/panel/{*rest}", any(crate::panel::handle))
                .with_state(panel),
        );
    }
    let shutdown = cancel.clone();
    let task = tokio::spawn(async move {
        if axum::serve(prepared.listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .is_err()
        {
            eprintln!("Brainkeep HTTP listener stopped unexpectedly");
        }
    });
    (cancel, task)
}
