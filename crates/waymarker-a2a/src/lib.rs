//! A2A (Agent2Agent) server: lets other agents hand Waymarker a form to inspect, fill or submit.
//!
//! One A2A `contextId` is one Waymarker session: its own Chrome, its own tabs and its own shared
//! `context` notes, so a conversation with a remote agent keeps its state across messages while two
//! different conversations can never see each other's pages.
//!
//! Speaks A2A v1.0, JSON-RPC binding, over HTTP with SSE streaming. See `proto` for the wire shapes.

mod intent;
mod proto;
mod server;
mod store;

pub use server::{router, serve, A2aConfig, AppFactory};
