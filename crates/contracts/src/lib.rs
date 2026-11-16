//! Wire types shared by server, runner and web: agent output, HTTP API, SSE events, runner protocol. Pure data, no IO.

pub mod agent;
pub mod api;
pub mod common;
pub mod ids;
pub mod job;
pub mod routes;
