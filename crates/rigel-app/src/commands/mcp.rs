//! MCP health for the window's on-demand panel (roadmap 144).
//!
//! The project's `.mcp.json` is the declaration the agents already use. Reading it and probing
//! every declared stdio server goes through the same core functions the tests use; nothing here
//! makes a network request, and one failed handshake never cancels the other probes.

use std::path::Path;
use std::time::Duration;

use rigel_core::mcp::{ServerHealth, ServerSpec};

/// One server that does not answer must not hold the panel open forever.
const PROBE_WAIT: Duration = Duration::from_secs(8);

#[tauri::command]
pub(crate) async fn health(project: String) -> Result<Vec<ServerHealth>, String> {
    let root = Path::new(&project);
    let mut servers: Vec<ServerSpec> = rigel_mcp::project_mcp::servers(root)?
        .into_values()
        .map(|server| ServerSpec {
            name: server.name,
            command: server.command,
            args: server.args,
            env: server.env,
        })
        .collect();
    servers.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(rigel_core::mcp::probe_all(&servers, PROBE_WAIT).await)
}
