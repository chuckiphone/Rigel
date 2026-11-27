//! MCP helpers owned by the core (roadmap 69).
//!
//! The transport rules live here rather than in a connector so every caller applies the
//! same checks: a scrubbed stdio environment, no unauthenticated HTTP, no private-network
//! URL, and a bounded message.

pub mod health;
pub mod pin;
pub mod resources;
pub mod transport;

pub use health::{
    McpHealth, McpServerSpec, ServerHealth, ServerSpec, ServerState, probe, probe_all,
    probe_servers, refresh,
};
pub use pin::{Catalog, PinError, PinObservation, PinStore, SchemaDiff};
pub use resources::{
    McpPrompt, McpPromptArgument, McpResource, PromptGetResult, PromptMessage, ResourceContent,
    ResourceReadRecord, ResourceReadResult, get_prompt, list_prompts, list_resources,
    read_resource, record_read,
};

pub use transport::{
    ENVIRONMENT_ALLOWLIST, MAX_MESSAGE_BYTES, TransportError, check_http, check_message,
    check_stdio_environment, http_client, scrub_environment,
};
