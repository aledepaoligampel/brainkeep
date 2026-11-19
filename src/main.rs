use brainkeep::{files, store::Store};
use clap::{Parser, Subcommand};
use rmcp::{ServiceExt, transport::async_rw::AsyncRwTransport};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{Semaphore, mpsc},
    task::JoinSet,
};
mod client_ip;
mod http;
mod mcp;
mod oauth;
mod panel;

// History returns up to 20 full revisions. MCP wraps serialized JSON in a text
// block, so valid control characters/backslashes can expand more than once.
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Parser)]
#[command(
    version,
    about = "Brainkeep: persistent shared memory for agents. Unix prototype."
)]
struct Args {
    #[arg(long, global = true, default_value = ".brainkeep")]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Run the storage owner; optionally enable authenticated HTTP (loopback by default).
    Serve {
        #[arg(long, requires_all = ["auth_file", "http_host"])]
        http_port: Option<u16>,
        #[arg(long, requires = "http_port")]
        auth_file: Option<PathBuf>,
        #[arg(long, requires = "http_port")]
        http_host: Option<String>,
        /// Address to listen on (default 127.0.0.1). Use 0.0.0.0 or :: only behind a
        /// proxy that sets the client IP header.
        #[arg(long, requires = "http_port")]
        http_listen: Option<std::net::IpAddr>,
        /// Header the proxy overwrites with the client address (default CF-Connecting-IP).
        #[arg(long, requires = "http_port")]
        client_ip_header: Option<String>,
        /// Enable the read-only web panel with a dedicated credential file.
        #[arg(long, requires = "http_port")]
        panel_auth_file: Option<PathBuf>,
        /// Enable the OAuth authorization server with a private Google client file.
        #[arg(long, requires = "http_port")]
        oauth_file: Option<PathBuf>,
        /// Admin user handle; required once to migrate existing memories to schema v4.
        #[arg(long)]
        owner: Option<String>,
    },
    /// MCP stdio bridge for agents. Requires `serve` running.
    Mcp,
    /// Call a Brainkeep tool with a JSON object on stdin. Outputs JSON.
    Call { tool: String },
    /// Check the running service.
    Status,
    /// Create a consistent private backup at a new file path.
    Backup { destination: PathBuf },
    /// Restore a private backup into a NEW data directory (offline).
    Restore { source: PathBuf },
    /// Manage invited users. Requires `serve` running; changes apply without restart.
    Users {
        #[command(subcommand)]
        action: UsersAction,
    },
}
#[derive(Subcommand)]
enum UsersAction {
    /// Enable an email to sign in as a handle (re-enables a disabled user).
    Invite {
        email: String,
        #[arg(long)]
        handle: String,
    },
    /// Disable a user and reject all of their credentials immediately.
    Disable { handle: String },
    /// List users.
    List,
}
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args = Args::parse();
    let result = run(args).await;
    if let Err(message) = result {
        eprintln!("Brainkeep: {message}");
        std::process::exit(1);
    }
}
async fn run(args: Args) -> Result<(), String> {
    match args.command {
        Command::Serve {
            http_port,
            auth_file,
            http_host,
            http_listen,
            client_ip_header,
            panel_auth_file,
            oauth_file,
            owner,
        } => {
            if let Some(name) = client_ip_header {
                client_ip::configure(&name)?;
            }
            let remote = match (http_port, auth_file, http_host) {
                (Some(port), Some(auth), Some(host)) => Some(
                    http::prepare(
                        http_listen.unwrap_or(std::net::Ipv4Addr::LOCALHOST.into()),
                        port,
                        &auth,
                        host,
                        panel_auth_file.as_deref(),
                        oauth_file.as_deref(),
                    )
                    .await?,
                ),
                (None, None, None) => None,
                _ => return Err("HTTP needs port, host and auth file".into()),
            };
            serve(&args.data_dir, remote, owner).await
        }
        Command::Mcp => bridge(&args.data_dir).await,
        Command::Call { tool } => {
            let mut body = Vec::new();
            tokio::io::stdin()
                .take(32769)
                .read_to_end(&mut body)
                .await
                .map_err(|_| "cannot read input")?;
            if body.len() > 32768 {
                return Err("input too large".into());
            }
            let input = serde_json::from_slice(&body).map_err(|_| "invalid JSON input")?;
            print_result(call(&args.data_dir, &tool, input).await?)
        }
        Command::Status => print_result(call(&args.data_dir, "status", json!({})).await?),
        Command::Backup { destination } => {
            let full = if destination.is_absolute() {
                destination
            } else {
                std::env::current_dir()
                    .map_err(|_| "cannot resolve path")?
                    .join(destination)
            };
            print_result(call(&args.data_dir, "backup", json!({"destination":full})).await?)
        }
        Command::Users { action } => {
            let (tool, input) = match action {
                UsersAction::Invite { email, handle } => {
                    ("users.invite", json!({"email":email,"handle":handle}))
                }
                UsersAction::Disable { handle } => ("users.disable", json!({"handle":handle})),
                UsersAction::List => ("users.list", json!({})),
            };
            print_result(call(&args.data_dir, tool, input).await?)
        }
        Command::Restore { source } => {
            tokio::task::spawn_blocking(move || Store::restore(&source, &args.data_dir))
                .await
                .map_err(|_| "restore worker failed")??;
            print_result(json!({"restored":true}))
        }
    }
}
fn print_result(value: Value) -> Result<(), String> {
    println!("{value}");
    Ok(())
}
fn socket(dir: &Path) -> PathBuf {
    dir.join("brainkeep.sock")
}
async fn connect(dir: &Path) -> Result<UnixStream, String> {
    files::private_dir(dir)?;
    let stream = UnixStream::connect(socket(dir))
        .await
        .map_err(|_| "Brainkeep unavailable; start `brainkeep --data-dir <directory> serve`")?;
    if stream
        .peer_cred()
        .map_err(|_| "cannot authenticate local server")?
        .uid()
        != unsafe { libc::geteuid() }
    {
        return Err("local server belongs to another user".into());
    }
    Ok(stream)
}
async fn bridge(dir: &Path) -> Result<(), String> {
    let stream = connect(dir).await?;
    let (mut read, mut write) = stream.into_split();
    let input = async {
        tokio::io::copy(&mut tokio::io::stdin(), &mut write).await?;
        write.shutdown().await
    };
    let output = async {
        tokio::io::copy(&mut read, &mut tokio::io::stdout()).await?;
        Ok::<(), std::io::Error>(())
    };
    tokio::try_join!(input, output).map_err(|_| "MCP connection closed")?;
    Ok(())
}
async fn serve(
    dir: &Path,
    remote: Option<http::Prepared>,
    owner: Option<String>,
) -> Result<(), String> {
    let owned = dir.to_owned();
    let mut store =
        tokio::task::spawn_blocking(move || Store::open_with_owner(&owned, owner.as_deref()))
            .await
            .map_err(|_| "storage worker failed")??;
    // Store holds the directory lock, so stale sockets may now be removed safely.
    let sock = socket(dir);
    if std::fs::symlink_metadata(&sock).is_ok() {
        use std::os::unix::fs::FileTypeExt;
        if !std::fs::symlink_metadata(&sock)
            .map_err(|_| "cannot inspect socket")?
            .file_type()
            .is_socket()
        {
            return Err("socket path is occupied by a non-socket".into());
        }
        std::fs::remove_file(&sock).map_err(|_| "cannot remove stale socket")?;
    }
    let listener = UnixListener::bind(&sock)
        .map_err(|_| "cannot bind private local socket (check path length)")?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| "cannot secure socket")?;
    // Room for bursts from several panel tabs without starving MCP calls (try_send).
    let (tx, mut rx) = mpsc::channel::<mcp::Job>(128);
    let (changes, _) = tokio::sync::broadcast::channel::<(String, bool)>(256);
    let notify = changes.clone();
    let worker = std::thread::spawn(move || {
        while let Some(job) = rx.blocking_recv() {
            // Accepted operations complete even if the requester disconnects.
            let result = store.call_as(&job.name, job.args, &job.access);
            for change in store.take_changes() {
                // No subscriber is fine: nobody has the panel open.
                let _ = notify.send(change);
            }
            let _ = job.reply.send(result);
        }
    });
    let server = mcp::MemoryServer {
        jobs: tx,
        changes,
        allow_admin: true,
    };
    let remote = remote.map(|prepared| http::start(prepared, server.clone()));
    let slots = Arc::new(Semaphore::new(16));
    let mut clients = JoinSet::new();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|_| "cannot install shutdown handler")?;
    if remote.is_some() {
        eprintln!("Brainkeep ready (private socket and authenticated HTTP)");
    } else {
        eprintln!("Brainkeep ready (private local socket, no TCP listener)");
    }
    loop {
        tokio::select! {
            _=tokio::signal::ctrl_c()=>break,
            _=terminate.recv()=>break,
            Some(_)=clients.join_next(), if !clients.is_empty()=>{},
            incoming=listener.accept()=> {
                let (stream,_)=incoming.map_err(|_|"local accept failed")?;
                if stream.peer_cred().map_err(|_|"cannot authenticate peer")?.uid()!=unsafe{libc::geteuid()} {continue;}
                let Ok(permit)=slots.clone().try_acquire_owned() else {continue;};
                let api=server.clone();
                clients.spawn(async move {
                    let _permit=permit;
                    let (r,w)=stream.into_split();
                    let transport=AsyncRwTransport::new_server(r,w).with_max_line_length(65536);
                    if let Ok(Ok(service))=tokio::time::timeout(Duration::from_secs(10),api.serve(transport)).await {
                        let _=service.waiting().await;
                    }
                });
            }
        }
    }
    drop(listener);
    if let Some((cancel, task)) = remote {
        cancel.cancel();
        task.abort();
        let _ = task.await;
    }
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    std::fs::remove_file(&sock).map_err(|_| "cannot remove stopped service socket")?;
    drop(server);
    tokio::task::spawn_blocking(move || worker.join())
        .await
        .map_err(|_| "storage shutdown failed")?
        .map_err(|_| "storage worker failed")?;
    Ok(())
}
// Administration uses the exact same local MCP endpoint; no second SQLite writer.
async fn call(dir: &Path, name: &str, args: Value) -> Result<Value, String> {
    let stream = connect(dir).await?;
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    async fn send(w: &mut tokio::net::unix::OwnedWriteHalf, value: Value) -> Result<(), String> {
        w.write_all(format!("{value}\n").as_bytes())
            .await
            .map_err(|_| "connection failed".into())
    }
    async fn response(
        r: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
        id: i64,
    ) -> Result<Value, String> {
        loop {
            let mut buffer = Vec::new();
            let n = (&mut *r)
                .take((MAX_RESPONSE_BYTES + 1) as u64)
                .read_until(b'\n', &mut buffer)
                .await
                .map_err(|_| "connection failed")?;
            if n == 0 || n > MAX_RESPONSE_BYTES {
                return Err("invalid or closed server response".into());
            }
            let value: Value =
                serde_json::from_slice(&buffer).map_err(|_| "invalid server response")?;
            if value.get("id").and_then(Value::as_i64) == Some(id) {
                return Ok(value);
            }
        }
    }
    send(&mut write,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"brainkeep-admin","version":env!("CARGO_PKG_VERSION")}}})).await?;
    let init = tokio::time::timeout(Duration::from_secs(10), response(&mut read, 1))
        .await
        .map_err(|_| "initialize timeout")??;
    if init.get("error").is_some() {
        return Err("MCP initialization failed".into());
    }
    send(
        &mut write,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await?;
    send(&mut write,json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":args}})).await?;
    let out = tokio::time::timeout(Duration::from_secs(60), response(&mut read, 2))
        .await
        .map_err(|_| "operation timed out; outcome unknown, retry with the same operation_id")??;
    if out.get("error").is_some() {
        return Err("MCP request failed".into());
    }
    let result = &out["result"];
    let text = result["content"][0]["text"]
        .as_str()
        .ok_or("invalid tool response")?;
    if result["isError"] == true {
        return Err(text.into());
    }
    let data: Value = serde_json::from_str(text).map_err(|_| "invalid tool data")?;
    Ok(data["data"].clone())
}
