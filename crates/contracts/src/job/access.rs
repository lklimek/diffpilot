//! Credentials the agent may hold for one job, filled in at claim (ADR-005).

use std::fmt;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::RepoRef;

/// A secret string: serialised as is, never shown by `Debug`. Opaque, without length limit.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Secret(String);

impl Secret {
    /// Wraps a secret value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret value; use only where it is sent or written to its single destination.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

/// What the agent may reach during one job. Empty in the stored spec; filled when a runner claims the job.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentAccess {
    /// Read-only GitHub App installation token for `github-mcp-server`; none when the App is not installed on
    /// the PR repository (no `github-mcp-server` for the job, related-work lookup skipped).
    pub github: Option<InstallationToken>,
    /// MemCan MCP endpoint (search and `add_memory` only, by runner configuration; accepted risk T6).
    pub memcan: Option<McpHttpEndpoint>,
}

/// A read-only GitHub App installation token minted for one job (1 hour, revoked at job end). It covers the PR
/// repository only; related repositories are read as public data (ADR-005).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstallationToken {
    /// The token.
    pub token: Secret,
    /// Expiry set by GitHub.
    pub expires_at: DateTime<Utc>,
    /// The repository the token covers: the PR repository.
    pub repository: RepoRef,
}

/// A streamable-HTTP MCP endpoint with a bearer token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpHttpEndpoint {
    /// Endpoint URL.
    pub url: String,
    /// Bearer token.
    pub token: Secret,
}
