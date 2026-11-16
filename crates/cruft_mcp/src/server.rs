//! rmcp server running on its own tokio runtime thread. Every tool call is
//! forwarded to Bevy through `McpClient`; nothing here can reach the World.
use crate::bridge::McpClient;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::Value;
use std::{net::SocketAddr, sync::Arc};

#[derive(Clone)]
pub struct CruftServer {
    client: McpClient,
}

const INSTRUCTIONS: &str = "Serveur MCP de l'éditeur Cruft (Bevy). Par défaut l'agent PROPOSE : \
spawn_entity / set_component / despawn_entity / propose_changes créent une proposition en attente \
(fantômes jaunes) que l'utilisateur applique ou rejette. Regroupe les changements liés dans un seul \
propose_changes. Les IDs d'entité sont des chaînes décimales issues de list_entities. Les composants \
s'écrivent en JSON avec leur nom court (Transform : translation/scale [x,y,z], rotation [x,y,z,w]).";

impl ServerHandler for CruftServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("cruft", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: crate::tools::all(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let client = context
            .peer
            .peer_info()
            .map(|info| info.client_info.name.clone());
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let result = match self.client.call(&request.name, arguments, client).await {
            Ok(value) => {
                if let Some(png) = value.get("png_base64").and_then(Value::as_str) {
                    CallToolResult::success(vec![ContentBlock::image(png, "image/png")])
                } else {
                    let text = serde_json::to_string_pretty(&value).unwrap_or_default();
                    CallToolResult::success(vec![ContentBlock::text(text)])
                }
            }
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
        };
        Ok(result.into())
    }
}

/// Transports to start. Both may run at once.
#[derive(Clone, Debug)]
pub struct McpTransports {
    pub stdio: bool,
    pub http: Option<SocketAddr>,
}

pub(crate) fn spawn(
    client: McpClient,
    transports: McpTransports,
    running: Arc<std::sync::atomic::AtomicBool>,
) {
    let result = std::thread::Builder::new()
        .name("cruft-mcp".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    bevy::log::error!("MCP : runtime tokio indisponible : {error}");
                    return;
                }
            };
            runtime.block_on(async move {
                running.store(true, std::sync::atomic::Ordering::Relaxed);
                let mut tasks = Vec::new();
                if let Some(addr) = transports.http {
                    let client = client.clone();
                    tasks.push(tokio::spawn(async move {
                        if let Err(error) = serve_http(client, addr).await {
                            bevy::log::error!("MCP HTTP sur {addr} : {error}");
                        }
                    }));
                }
                if transports.stdio {
                    let server = CruftServer { client };
                    tasks.push(tokio::spawn(async move {
                        match server.serve(rmcp::transport::stdio()).await {
                            Ok(service) => {
                                let _ = service.waiting().await;
                                bevy::log::info!("MCP stdio : client déconnecté");
                            }
                            Err(error) => bevy::log::error!("MCP stdio : {error}"),
                        }
                    }));
                }
                for task in tasks {
                    let _ = task.await;
                }
                running.store(false, std::sync::atomic::Ordering::Relaxed);
            });
        });
    if let Err(error) = result {
        bevy::log::error!("MCP : thread non démarré : {error}");
    }
}

async fn serve_http(client: McpClient, addr: SocketAddr) -> std::io::Result<()> {
    let service = StreamableHttpService::new(
        move || {
            Ok(CruftServer {
                client: client.clone(),
            })
        },
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    bevy::log::info!("MCP Streamable HTTP : http://{addr}/mcp");
    axum::serve(listener, router).await
}
