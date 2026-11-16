//! Shared test helpers: the fixture PR (`fixtures/pr-482`), its git repository and job specs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use contracts::agent::{JobKind, output_schema};
use contracts::ids::{AnalysisId, CommitSha, JobId};
use contracts::job::access::AgentAccess;
use contracts::job::spec::{
    CredentialKind, JobLimits, JobSpec, LlmCredentialRef, ModelChoice, ReasoningEffort,
    RepoCheckout,
};
use serde::Deserialize;
use tempfile::TempDir;

use crate::execute::RunnerDirs;

/// Commit ids of the fixture repository (`repo/commits.json`).
#[derive(Deserialize)]
pub(crate) struct Commits {
    pub(crate) base: String,
    pub(crate) head: String,
}

/// A path below `fixtures/pr-482`.
pub(crate) fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/pr-482")
        .join(rel)
}

pub(crate) fn commits() -> Commits {
    serde_json::from_str(&fs::read_to_string(fixture("repo/commits.json")).unwrap()).unwrap()
}

/// Builds the fixture repository at `dest` with `build-repo.sh`.
pub(crate) fn build_repo(dest: &Path) {
    let status = Command::new("bash")
        .arg(fixture("repo/build-repo.sh"))
        .arg(dest)
        .stdout(Stdio::null())
        .status()
        .expect("run build-repo.sh");
    assert!(status.success(), "build-repo.sh failed");
}

/// The fixture repository, built once per test process and only read afterwards.
pub(crate) fn shared_repo() -> &'static Path {
    static REPO: OnceLock<PathBuf> = OnceLock::new();
    REPO.get_or_init(|| {
        let dir = TempDir::new().unwrap().keep().join("node-client");
        build_repo(&dir);
        dir
    })
}

pub(crate) fn credential() -> LlmCredentialRef {
    LlmCredentialRef {
        id: "default".to_owned(),
        kind: CredentialKind::CodexChatgpt,
    }
}

/// A job of `kind` on the fixture repository at `repo`.
pub(crate) fn spec(kind: JobKind, repo: &Path) -> JobSpec {
    let commits = commits();
    JobSpec {
        job_id: JobId::new_random(),
        attempt: 1,
        kind,
        analysis_id: AnalysisId::new_random(),
        repo: RepoCheckout {
            clone_url: format!("file://{}", repo.display()),
            base_sha: CommitSha(commits.base),
            head_sha: CommitSha(commits.head),
        },
        prompt: "Job: analyse_flow f1.".to_owned(),
        output_schema: output_schema(kind).unwrap(),
        model: ModelChoice {
            model: "gpt-6.1-sol".to_owned(),
            reasoning_effort: ReasoningEffort::High,
        },
        limits: JobLimits::default(),
        credential: credential(),
        session: None,
        context_pack: None,
        skills: Vec::new(),
        access: AgentAccess::default(),
    }
}

/// Runner directories below `root`.
pub(crate) fn dirs(root: &Path) -> RunnerDirs {
    RunnerDirs {
        work: root.join("work"),
        codex_homes: root.join("state/codex-homes"),
        mirrors: root.join("state/mirrors"),
        sessions: root.join("state/sessions"),
    }
}
