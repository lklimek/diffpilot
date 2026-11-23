//! Wire types shared by server, runner and web: agent output, HTTP API, SSE events, runner protocol. Pure data, no IO.

pub mod agent;
pub mod api;
pub mod common;
pub mod generate;
pub mod ids;
pub mod job;
pub mod routes;
pub mod time;

/// Common gateway administration contracts.
pub mod gateway;
/// Common container runtime contracts.
pub mod runtime;

/// Private runner-to-launcher protocol.
pub mod launcher;
