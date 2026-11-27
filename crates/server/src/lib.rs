//! diffpilot server: HTTP API and SSE, sessions, GitHub integration, pipeline, job queue and runner
//! protocol, MemCan client and the embedded web UI.

pub mod api;
pub mod ask;
pub mod auth;
pub mod config;
pub mod dashboard;
pub mod frontend;
pub mod gateway;
pub mod github;
pub mod jobs;
pub mod memcan;
pub mod model;
pub mod pipeline;
pub mod prompts;
pub mod review;
pub mod review_loop;
pub mod secrets;
pub mod state;
#[cfg(test)]
pub(crate) mod test_support;
pub mod webhooks;
