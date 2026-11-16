//! MCP server for Cruft. Architecture:
//! - `server`: rmcp handler + stdio / Streamable HTTP transports on a tokio thread.
//! - `bridge`: bounded crossbeam channel of requests, serviced by ONE exclusive
//!   Bevy system per frame. Tokio never accesses the World; replies use oneshot
//!   channels with a timeout, so neither side can block the other.
mod bridge;
mod server;
mod tools;

pub use bridge::{McpBridge, McpClient, McpScreenshotTarget, Reply};
pub use server::{CruftServer, McpTransports};

use bevy::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, atomic::AtomicBool};

pub const DEFAULT_HTTP_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 7457);

pub struct CruftMcpPlugin {
    /// `None`: no server thread (tests, headless tools).
    pub transports: Option<McpTransports>,
}

impl Default for CruftMcpPlugin {
    /// Streamable HTTP on 127.0.0.1:7457; stdio only with `--mcp-stdio`.
    fn default() -> Self {
        Self {
            transports: Some(McpTransports {
                stdio: std::env::args().any(|arg| arg == "--mcp-stdio"),
                http: Some(DEFAULT_HTTP_ADDR),
            }),
        }
    }
}

impl CruftMcpPlugin {
    /// Bridge only, no network / stdio server.
    pub fn headless() -> Self {
        Self { transports: None }
    }
}

impl Plugin for CruftMcpPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<McpBridge>()
            .init_resource::<McpScreenshotTarget>()
            .init_resource::<cruft_core::AgentActivity>()
            .add_systems(Update, bridge::process_requests);
        app.world_mut()
            .resource_mut::<cruft_core::AgentActivity>()
            .tool_count = tools::all().len();
        if let Some(transports) = self.transports.clone() {
            let mut endpoint: Vec<String> = transports
                .http
                .map(|addr| format!("http://{addr}/mcp"))
                .into_iter()
                .collect();
            if transports.stdio {
                endpoint.push("stdio".into());
            }
            app.world_mut()
                .resource_mut::<cruft_core::AgentActivity>()
                .endpoint = Some(endpoint.join(" · "));
            let client = app.world().resource::<McpBridge>().client.clone();
            let running = Arc::new(AtomicBool::new(false));
            server::spawn(client, transports, running.clone());
            app.insert_resource(McpServerStatus(running))
                .add_systems(Update, sync_status);
        }
    }
}

#[derive(Resource)]
struct McpServerStatus(Arc<AtomicBool>);

fn sync_status(status: Res<McpServerStatus>, mut activity: ResMut<cruft_core::AgentActivity>) {
    let running = status.0.load(std::sync::atomic::Ordering::Relaxed);
    if activity.server_running != running {
        activity.server_running = running;
    }
}

/// Lets binaries write logs to stderr when stdio carries the MCP protocol.
pub fn stdio_requested() -> bool {
    std::env::args().any(|arg| arg == "--mcp-stdio")
}
