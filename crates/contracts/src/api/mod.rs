//! HTTP API DTOs, the error model and SSE events (`docs/contracts/http-api.md`). Deterministic facts and LLM
//! content are separate fields (`facts` / `llm`).

pub mod access_tokens;
pub mod analyses;
pub mod auth;
pub mod code;
pub mod comments;
pub mod dashboard;
pub mod error;
pub mod events;
pub mod feedback;
pub mod files;
pub mod pr;
pub mod questions;
pub mod review;
pub mod settings;
pub mod tour;
pub mod usage;
