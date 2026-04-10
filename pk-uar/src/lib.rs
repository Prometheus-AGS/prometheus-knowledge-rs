//! # pk-uar — UAR Tool Bridge
//!
//! Integrates prometheus-knowledge into the Universal Agent Runtime as a
//! first-class tool provider. Two integration paths:
//!
//! ## Path 1: In-process (zero-latency, preferred)
//! [`UarToolProvider`] wraps the [`Librarian`] directly — no HTTP round-trip.
//!
//! ## Path 2: Remote MCP (sidecar / K3s pod)
//! [`UarMcpClient`] sends JSON-RPC over HTTP to a running `pk-cherry` instance.

pub mod in_process;
pub mod mcp_client;
pub mod registry;

pub use in_process::UarToolProvider;
pub use mcp_client::UarMcpClient;
pub use registry::{AgentTool, KnowledgeTool, ToolCall, ToolResult};
