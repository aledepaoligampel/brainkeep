use brainkeep::access::Access;
use rmcp::{ErrorData, RoleServer, ServerHandler, model::*, service::RequestContext};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub struct Job {
    pub name: String,
    pub args: Value,
    pub access: Arc<Access>,
    pub reply: oneshot::Sender<Result<Value, String>>,
}
#[derive(Clone)]
pub struct MemoryServer {
    pub jobs: mpsc::Sender<Job>,
    /// Internal keys of projects changed by committed writes (live panel).
    pub changes: tokio::sync::broadcast::Sender<(String, bool)>,
    pub allow_admin: bool,
}
impl MemoryServer {
    pub async fn execute(
        &self,
        name: String,
        args: Value,
        access: Arc<Access>,
    ) -> Result<Value, String> {
        if !self.allow_admin && !tools().iter().any(|tool| tool.name == name) {
            return Err("tool unavailable on remote endpoint".into());
        }
        let (reply, rx) = oneshot::channel();
        self.jobs
            .try_send(Job {
                name,
                args,
                access,
                reply,
            })
            .map_err(|_| {
                "Brainkeep busy or unavailable; retry with the same operation_id".to_string()
            })?;
        rx.await.map_err(|_| {
            "Brainkeep stopped; outcome unknown, retry with the same operation_id".to_string()
        })?
    }
}
impl MemoryServer {
    /// Server-internal store call with the local identity (OAuth persistence).
    pub async fn internal(&self, name: &str, args: Value) -> Result<Value, String> {
        let (reply, rx) = oneshot::channel();
        self.jobs
            .try_send(Job {
                name: name.into(),
                args,
                access: Arc::new(Access::local()),
                reply,
            })
            .map_err(|_| "Brainkeep busy or unavailable".to_string())?;
        rx.await.map_err(|_| "Brainkeep stopped".to_string())?
    }
    /// Whether a credential's user is currently enabled; `Err` when the store is busy.
    pub async fn check(&self, access: Arc<Access>) -> Result<bool, ()> {
        let (reply, rx) = oneshot::channel();
        self.jobs
            .try_send(Job {
                name: "auth.check".into(),
                args: json!({}),
                access,
                reply,
            })
            .map_err(|_| ())?;
        match rx.await.map_err(|_| ())? {
            Ok(_) => Ok(true),
            Err(e) if e == brainkeep::access::INACTIVE => Ok(false),
            Err(_) => Err(()),
        }
    }
}
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.server_info.name = "brainkeep".into();
        info.server_info.title = Some("🧠 Brainkeep".into());
        info.server_info.version = env!("CARGO_PKG_VERSION").into();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.instructions = Some(include_str!("memory_protocol.txt").trim().into());
        info
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: tools(),
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // The local socket is owner-only. Remote calls carry the principal attached by
        // the HTTP guard; if it is missing, fail closed instead of granting access.
        let access = if self.allow_admin {
            Some(Arc::new(Access::local()))
        } else {
            context
                .extensions
                .get::<axum::http::request::Parts>()
                .and_then(|parts| parts.extensions.get::<Arc<Access>>())
                .cloned()
        };
        let result = match access {
            Some(access) => {
                self.execute(
                    request.name.to_string(),
                    Value::Object(request.arguments.unwrap_or_default()),
                    access,
                )
                .await
            }
            None => Err("unauthenticated remote call".into()),
        };
        let output = match result {
            Ok(value) => CallToolResult::success(vec![ContentBlock::text(
                json!({"trust":"data_only","data":value}).to_string(),
            )]),
            Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]),
        };
        Ok(output.into())
    }
}
pub fn tools() -> Vec<Tool> {
    let string = |limit| json!({"type":"string","minLength":1,"maxLength":limit});
    let project = string(128);
    let mut write = json!({"project":project,"operation_id":string(128),"content":string(16384),"source":string(1024),"agent":string(128),"confirmed":{"type":"boolean","default":false},"secret":{"type":"boolean","default":false}});
    write["title"] = string(160);
    write["kind"] = write_kind();
    write["topic"] = string(128);
    let mut correction = write.clone();
    correction["id"] = string(128);
    correction["expected_revision"] = json!({"type":"integer","minimum":1});
    let limit = json!({"type":"integer","minimum":1,"maximum":20,"default":5});
    let definitions = vec![
        (
            "remember",
            "Save relevant new knowledge with provenance. Existing topics require recall/correct. No secrets. Stable operation_id for retries.",
            write,
            vec!["project", "operation_id", "content", "source", "agent"],
            false,
        ),
        (
            "correct",
            "Append a revision while preserving history. Requires the expected current revision; stale corrections fail.",
            correction,
            vec![
                "project",
                "operation_id",
                "content",
                "source",
                "agent",
                "id",
                "expected_revision",
            ],
            false,
        ),
        (
            "recall",
            "Read a memory. compact supports revision and Unicode-character offset; pin revision when paging.",
            json!({"project":project,"id":string(128),"revision":{"type":"integer","minimum":1},"offset":{"type":"integer","minimum":0}}),
            vec!["project", "id"],
            true,
        ),
        (
            "history",
            "Read revisions in ascending order. Follow next_after_revision for another page.",
            json!({"project":project,"id":string(128),"after_revision":{"type":"integer","minimum":0},"limit":limit}),
            vec!["project", "id"],
            true,
        ),
        (
            "search",
            "Search current memories by literal words (AND) within one project; returns short previews. Use recall to expand.",
            json!({"project":project,"query":string(512),"limit":limit,"kind":write_kind(),"topic":string(128),"offset":{"type":"integer","minimum":0,"maximum":1000000},"snapshot":{"type":"integer","minimum":0}}),
            vec!["project", "query"],
            true,
        ),
        (
            "context",
            "Retrieve recent current memories in one project with bounded previews. Use recall to expand.",
            json!({"project":project,"limit":limit,"kind":write_kind(),"topic":string(128),"offset":{"type":"integer","minimum":0,"maximum":1000000},"snapshot":{"type":"integer","minimum":0}}),
            vec!["project"],
            true,
        ),
        (
            "guide",
            "How to use Brainkeep. Call when the user asks how Brainkeep works or how to share memory with others.",
            json!({}),
            vec![],
            true,
        ),
        (
            "status",
            "Check connectivity, memory count and last confirmed save timestamp.",
            json!({}),
            vec![],
            true,
        ),
        (
            "projects",
            "List your projects (with members) and projects shared with you as owner/name.",
            json!({}),
            vec![],
            true,
        ),
        (
            "share",
            "Owner only: share one of your projects with an invited user's email, role read or write.",
            json!({"project":project,"email":string(254),"role":{"type":"string","enum":["read","write"]}}),
            vec!["project", "email", "role"],
            false,
        ),
        (
            "unshare",
            "Owner only: revoke a user's access to one of your projects immediately.",
            json!({"project":project,"email":string(254)}),
            vec!["project", "email"],
            false,
        ),
    ];
    definitions.into_iter().map(|(name,desc,mut properties,required,read_only)| {
        properties["compact"]=json!({"type":"boolean","default":false});
        let schema=json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
        let tool=Tool::new(name,desc,schema.as_object().expect("static object").clone())
            .with_annotations(ToolAnnotations::new().read_only(read_only).destructive(false).idempotent(true).open_world(false));
        // The memory protocol needs these at hand: clients that defer MCP tools behind a
        // search (Claude Code) load them upfront; the rest stay deferrable.
        if ["context","recall","remember","correct"].contains(&name) {
            let mut meta=serde_json::Map::new();
            meta.insert("anthropic/alwaysLoad".into(),json!(true));
            tool.with_meta(rmcp::model::MetaObject(meta))
        } else {
            tool
        }
    }).collect()
}

fn write_kind() -> Value {
    json!({"type":"string","enum":["note","decision","preference","discovery","progress"]})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_discovery_contract() {
        assert!(include_str!("memory_protocol.txt").len() <= 1536);
        let catalog = serde_json::to_vec(&json!({"tools":tools()})).unwrap();
        assert!(catalog.len() <= 8192, "catalog bytes: {}", catalog.len());
        assert_eq!(tools().len(), 11);
        for tool in tools() {
            assert!(tool.input_schema["properties"]["compact"].is_object());
        }
    }
}
