//! Repository for `analyses`. Frozen signatures (cross-WP); bodies are filled in by WP2-JOBS / WP2-INGEST.

use chrono::{DateTime, Utc};
use contracts::api::analyses::{AnalysisState, AnalysisSummary, FlowRef};
use contracts::api::pr::PrSummary;
use contracts::common::{LlmLanguage, RepoRef};
use contracts::ids::{AnalysisId, CommitSha, FlowId};
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

use crate::error::DbError;
use crate::repo::jobs::{parse_ts, parse_wire, ts, wire_name};

/// Access to analyses.
#[derive(Debug)]
pub struct Analyses<'a> {
    pub(crate) pool: &'a SqlitePool,
}

/// A new analysis in state `planning`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAnalysis {
    /// Analysis id.
    pub id: AnalysisId,
    /// Tenant.
    pub tenant_id: String,
    /// The stored pull request.
    pub pull_request_id: i64,
    /// Base commit.
    pub base_sha: CommitSha,
    /// Head commit.
    pub head_sha: CommitSha,
    /// Language of the LLM texts.
    pub language: LlmLanguage,
    /// User who requested it.
    pub created_by: String,
}

/// A stored analysis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisRecord {
    /// Analysis id.
    pub id: AnalysisId,
    /// Tenant.
    pub tenant_id: String,
    /// The stored pull request.
    pub pull_request_id: i64,
    /// Base commit.
    pub base_sha: CommitSha,
    /// Head commit.
    pub head_sha: CommitSha,
    /// Language of the LLM texts.
    pub language: LlmLanguage,
    /// State and progress.
    pub state: AnalysisState,
    /// Agent session (Codex thread) id, once a job reported one.
    pub agent_thread_id: Option<String>,
    /// Lifecycle of the session files on the runner.
    pub session_state: SessionState,
    /// claudius commit whose skills the agent used.
    pub skills_commit: Option<String>,
    /// User who requested it.
    pub created_by: String,
    /// When it was requested.
    pub created_at: DateTime<Utc>,
    /// When it finished, failed or was cancelled.
    pub finished_at: Option<DateTime<Utc>>,
}

/// Lifecycle of an analysis's agent-session files on the runner (ADR-004).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionState {
    /// No job has run yet.
    None,
    /// The session exists and is resumed by later jobs.
    Active,
    /// Cancelled or superseded: the runner is asked to delete the files.
    DropRequested,
    /// The runner confirmed the deletion.
    Dropped,
}

impl SessionState {
    fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "none" => Ok(Self::None),
            "active" => Ok(Self::Active),
            "drop_requested" => Ok(Self::DropRequested),
            "dropped" => Ok(Self::Dropped),
            _ => Err(DbError::Corrupt {
                column: "analyses.session_state",
            }),
        }
    }
}

/// Columns of [`AnalysisRecord`]; queries alias `analyses` as `a`.
macro_rules! columns {
    () => {
        "a.id, a.tenant_id, a.pull_request_id, a.base_sha, a.head_sha, a.language, a.state,
    a.flows_total, a.flows_done, a.paused_until, a.failure_flow_id, a.failure_flow_title, a.failure_message,
    a.agent_thread_id, a.session_state, a.skills_commit, a.created_by, a.created_at, a.finished_at"
    };
}

/// `AnalysisState` spread over its columns.
struct StateColumns {
    state: &'static str,
    flows_total: Option<u32>,
    flows_done: u32,
    paused_until: Option<String>,
    failure_flow_id: Option<String>,
    failure_flow_title: Option<String>,
    failure_message: Option<String>,
}

impl StateColumns {
    fn of(state: &AnalysisState) -> Self {
        let mut columns = Self {
            state: "planning",
            flows_total: None,
            flows_done: 0,
            paused_until: None,
            failure_flow_id: None,
            failure_flow_title: None,
            failure_message: None,
        };
        match state {
            AnalysisState::Planning => {}
            AnalysisState::Running { done, total } => {
                columns.state = "running";
                columns.flows_done = *done;
                columns.flows_total = Some(*total);
            }
            AnalysisState::Paused { resumes_at } => {
                columns.state = "paused";
                columns.paused_until = resumes_at.map(ts);
            }
            AnalysisState::Failed { flow, message } => {
                columns.state = "failed";
                columns.failure_flow_id = flow.as_ref().map(|flow| flow.id.0.clone());
                columns.failure_flow_title = flow.as_ref().map(|flow| flow.title.clone());
                columns.failure_message = Some(message.clone());
            }
            AnalysisState::Finished { total } => {
                columns.state = "finished";
                columns.flows_total = Some(*total);
            }
            AnalysisState::Cancelled => columns.state = "cancelled",
        }
        columns
    }

    fn is_terminal(&self) -> bool {
        matches!(self.state, "failed" | "finished" | "cancelled")
    }
}

fn state_from_row(row: &SqliteRow) -> Result<AnalysisState, DbError> {
    let state: String = row.try_get("state")?;
    let total: Option<u32> = row.try_get("flows_total")?;
    let corrupt = DbError::Corrupt {
        column: "analyses.flows_total",
    };
    Ok(match state.as_str() {
        "planning" => AnalysisState::Planning,
        "running" => AnalysisState::Running {
            done: row.try_get("flows_done")?,
            total: total.ok_or(corrupt)?,
        },
        "paused" => AnalysisState::Paused {
            resumes_at: row
                .try_get::<Option<String>, _>("paused_until")?
                .map(|at| parse_ts(&at, "analyses.paused_until"))
                .transpose()?,
        },
        "failed" => {
            let id: Option<String> = row.try_get("failure_flow_id")?;
            let title: Option<String> = row.try_get("failure_flow_title")?;
            AnalysisState::Failed {
                flow: id.zip(title).map(|(id, title)| FlowRef {
                    id: FlowId(id),
                    title,
                }),
                message: row
                    .try_get::<Option<String>, _>("failure_message")?
                    .unwrap_or_default(),
            }
        }
        "finished" => AnalysisState::Finished {
            total: total.ok_or(corrupt)?,
        },
        "cancelled" => AnalysisState::Cancelled,
        _ => {
            return Err(DbError::Corrupt {
                column: "analyses.state",
            });
        }
    })
}

fn record_from_row(row: &SqliteRow) -> Result<AnalysisRecord, DbError> {
    let id: String = row.try_get("id")?;
    let language: String = row.try_get("language")?;
    let session_state: String = row.try_get("session_state")?;
    let created_at: String = row.try_get("created_at")?;
    let finished_at: Option<String> = row.try_get("finished_at")?;
    Ok(AnalysisRecord {
        id: parse_wire(&id, "analyses.id")?,
        tenant_id: row.try_get("tenant_id")?,
        pull_request_id: row.try_get("pull_request_id")?,
        base_sha: CommitSha(row.try_get("base_sha")?),
        head_sha: CommitSha(row.try_get("head_sha")?),
        language: parse_wire(&language, "analyses.language")?,
        state: state_from_row(row)?,
        agent_thread_id: row.try_get("agent_thread_id")?,
        session_state: SessionState::parse(&session_state)?,
        skills_commit: row.try_get("skills_commit")?,
        created_by: row.try_get("created_by")?,
        created_at: parse_ts(&created_at, "analyses.created_at")?,
        finished_at: finished_at
            .map(|at| parse_ts(&at, "analyses.finished_at"))
            .transpose()?,
    })
}

impl Analyses<'_> {
    /// Creates an analysis in state `planning`; `Conflict` if a live one exists for the head and language.
    pub async fn create(
        &self,
        analysis: NewAnalysis,
        now: DateTime<Utc>,
    ) -> Result<AnalysisRecord, DbError> {
        sqlx::query(
            "INSERT INTO analyses (id, tenant_id, pull_request_id, base_sha, head_sha, language, state, created_by,
                 created_at)
             VALUES (?, ?, ?, ?, ?, ?, 'planning', ?, ?)",
        )
        .bind(analysis.id.to_string())
        .bind(&analysis.tenant_id)
        .bind(analysis.pull_request_id)
        .bind(&analysis.base_sha.0)
        .bind(&analysis.head_sha.0)
        .bind(wire_name(analysis.language)?)
        .bind(&analysis.created_by)
        .bind(ts(now))
        .execute(self.pool)
        .await?;
        self.get(analysis.id).await
    }

    /// An analysis.
    pub async fn get(&self, id: AnalysisId) -> Result<AnalysisRecord, DbError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM analyses a WHERE a.id = ?"
        ))
        .bind(id.to_string())
        .fetch_one(self.pool)
        .await?;
        record_from_row(&row)
    }

    /// The analysis as the API shows it: state plus the pull request at the analysed head.
    pub async fn summary(&self, id: AnalysisId) -> Result<AnalysisSummary, DbError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            ", r.owner, r.name, p.number, p.title, p.author, p.html_url
             FROM analyses a JOIN pull_requests p ON p.id = a.pull_request_id JOIN repos r ON r.id = p.repo_id
             WHERE a.id = ?"
        ))
        .bind(id.to_string())
        .fetch_one(self.pool)
        .await?;
        let record = record_from_row(&row)?;
        Ok(AnalysisSummary {
            id: record.id,
            pr: PrSummary {
                repo: RepoRef {
                    owner: row.try_get("owner")?,
                    name: row.try_get("name")?,
                },
                number: row.try_get("number")?,
                title: row.try_get("title")?,
                author: row.try_get("author")?,
                html_url: row.try_get("html_url")?,
                base_sha: record.base_sha,
                head_sha: record.head_sha,
            },
            language: record.language,
            state: record.state,
            created_at: record.created_at,
            finished_at: record.finished_at,
        })
    }

    /// The newest analysis of a pull request in a language, any head and state.
    pub async fn latest_for_pr(
        &self,
        pull_request_id: i64,
        language: LlmLanguage,
    ) -> Result<Option<AnalysisRecord>, DbError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM analyses a WHERE a.pull_request_id = ? AND a.language = ?
             ORDER BY a.created_at DESC, a.rowid DESC LIMIT 1"
        ))
        .bind(pull_request_id)
        .bind(wire_name(language)?)
        .fetch_optional(self.pool)
        .await?;
        row.as_ref().map(record_from_row).transpose()
    }

    /// Analyses still in `planning`, `running` or `paused`, oldest first: the pipeline resumes them at start.
    pub async fn unfinished(&self) -> Result<Vec<AnalysisRecord>, DbError> {
        sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM analyses a WHERE a.state IN ('planning', 'running', 'paused')
             ORDER BY a.created_at, a.rowid"
        ))
        .fetch_all(self.pool)
        .await?
        .iter()
        .map(record_from_row)
        .collect()
    }

    /// Stores a new state (progress included); terminal states set `finished_at`.
    pub async fn set_state(
        &self,
        id: AnalysisId,
        state: &AnalysisState,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let columns = StateColumns::of(state);
        let updated = sqlx::query(
            "UPDATE analyses SET state = ?, flows_total = ?, flows_done = ?, paused_until = ?, failure_flow_id = ?,
                 failure_flow_title = ?, failure_message = ?, finished_at = ?
             WHERE id = ?",
        )
        .bind(columns.state)
        .bind(columns.flows_total)
        .bind(columns.flows_done)
        .bind(&columns.paused_until)
        .bind(&columns.failure_flow_id)
        .bind(&columns.failure_flow_title)
        .bind(&columns.failure_message)
        .bind(columns.is_terminal().then(|| ts(now)))
        .bind(id.to_string())
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Replaces the base commit (the merge base found when the analysis is prepared); `Conflict` once the
    /// analysis has a job or a document, since those were made against the stored base.
    pub async fn set_base_sha(&self, id: AnalysisId, sha: &CommitSha) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE analyses SET base_sha = ?
             WHERE id = ?
               AND NOT EXISTS (SELECT 1 FROM jobs WHERE analysis_id = analyses.id)
               AND NOT EXISTS (SELECT 1 FROM documents WHERE analysis_id = analyses.id)",
        )
        .bind(&sha.0)
        .bind(id.to_string())
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 1 {
            return Ok(());
        }
        let exists = sqlx::query("SELECT 1 FROM analyses WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(self.pool)
            .await?;
        Err(if exists.is_some() {
            DbError::Conflict
        } else {
            DbError::NotFound
        })
    }

    /// Records the agent session reported by a job and marks it active.
    pub async fn set_session(
        &self,
        id: AnalysisId,
        thread_id: &str,
        skills_commit: Option<&str>,
    ) -> Result<(), DbError> {
        // A session already asked to be dropped stays dropped: a late job report must not revive it.
        let updated = sqlx::query(
            "UPDATE analyses SET agent_thread_id = ?, skills_commit = COALESCE(?, skills_commit),
                 session_state = CASE WHEN session_state IN ('none', 'active') THEN 'active' ELSE session_state END
             WHERE id = ?",
        )
        .bind(thread_id)
        .bind(skills_commit)
        .bind(id.to_string())
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Asks the runner to delete the session files (analysis cancelled or superseded).
    pub async fn request_session_drop(&self, id: AnalysisId) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE analyses SET session_state = CASE WHEN session_state = 'dropped' THEN 'dropped'
                 ELSE 'drop_requested' END
             WHERE id = ?",
        )
        .bind(id.to_string())
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Analyses whose session files the runner must delete.
    pub async fn pending_session_drops(&self) -> Result<Vec<AnalysisId>, DbError> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM analyses WHERE session_state = 'drop_requested' ORDER BY created_at, rowid",
        )
        .fetch_all(self.pool)
        .await?;
        ids.iter().map(|id| parse_wire(id, "analyses.id")).collect()
    }

    /// Marks session files as deleted, as acknowledged by a runner.
    pub async fn ack_session_drops(&self, ids: &[AnalysisId]) -> Result<(), DbError> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "UPDATE analyses SET session_state = 'dropped' WHERE session_state = 'drop_requested' AND id IN (",
        );
        let mut list = query.separated(", ");
        for id in ids {
            list.push_bind(id.to_string());
        }
        query.push(")");
        query.build().execute(self.pool).await?;
        Ok(())
    }
}

/// Seed rows for tests of this crate and of the server (`test-util` feature); never in release builds.
#[cfg(any(test, feature = "test-util"))]
pub mod testing {
    use contracts::agent::JobKind;
    use contracts::ids::{AnalysisId, CommitSha, JobId};
    use contracts::job::access::AgentAccess;
    use contracts::job::spec::{
        CredentialKind, JobLimits, JobSpec, LlmCredentialRef, ModelChoice, ReasoningEffort,
        RepoCheckout,
    };

    use crate::error::DbError;
    use crate::repo::{DEFAULT_TENANT, Db};

    /// Credential id used by [`job_spec`].
    pub const CREDENTIAL: &str = "codex-default";

    /// The in-memory database's only connection, held until dropped: every repository call waits meanwhile.
    #[derive(Debug)]
    pub struct HeldConnection(
        #[expect(dead_code, reason = "held only for its drop")]
        sqlx::pool::PoolConnection<sqlx::Sqlite>,
    );

    /// Takes the connection of a [`Db::in_memory`] database, to stall repository calls in tests.
    pub async fn hold_connection(db: &Db) -> Result<HeldConnection, DbError> {
        Ok(HeldConnection(db.pool.acquire().await?))
    }

    /// Inserts a user, the repository `acme/node-client`, PR #482 (once) and a new `planning` analysis of it.
    pub async fn seed_analysis(db: &Db) -> Result<AnalysisId, DbError> {
        let id = AnalysisId::new_random();
        sqlx::raw_sql(
            "INSERT OR IGNORE INTO users (id, github_id, login, created_at, updated_at)
                 VALUES ('u1', 1, 'lklimek', '2026-11-16T00:00:00.000000Z', '2026-11-16T00:00:00.000000Z');
             INSERT OR IGNORE INTO repos (id, tenant_id, owner, name) VALUES (1, 'default', 'acme', 'node-client');
             INSERT OR IGNORE INTO pull_requests (id, tenant_id, repo_id, number, title, author, html_url,
                 base_sha, head_sha, fetched_at)
               VALUES (1, 'default', 1, 482, 'Retry RPC calls', 'alice',
                 'https://github.com/acme/node-client/pull/482', 'b', 'h', '2026-11-16T00:00:00.000000Z');",
        )
        .execute(&db.pool)
        .await?;
        sqlx::query(
            "INSERT INTO analyses (id, tenant_id, pull_request_id, base_sha, head_sha, language, state, created_by,
                 created_at)
             VALUES (?, ?, 1, ?, ?, 'en', 'planning', 'u1', '2026-11-16T00:00:00.000000Z')",
        )
        .bind(id.to_string())
        .bind(DEFAULT_TENANT)
        .bind("1".repeat(40))
        .bind(format!("{}00000000", id.0.simple()))
        .execute(&db.pool)
        .await?;
        Ok(id)
    }

    /// Inserts a tenant (only `default` is seeded by the schema).
    pub async fn seed_tenant(db: &Db, id: &str) -> Result<(), DbError> {
        sqlx::query(
            "INSERT OR IGNORE INTO tenants (id, name, created_at) VALUES (?, ?, '2026-11-16T00:00:00.000000Z')",
        )
        .bind(id)
        .bind(id)
        .execute(&db.pool)
        .await?;
        Ok(())
    }

    /// A spec of `kind` for `analysis` without credentials, running with [`CREDENTIAL`].
    pub fn job_spec(analysis: AnalysisId, kind: JobKind) -> JobSpec {
        JobSpec {
            job_id: JobId::new_random(),
            attempt: 0,
            kind,
            analysis_id: analysis,
            repo: RepoCheckout {
                clone_url: "https://github.com/acme/node-client.git".to_owned(),
                base_sha: CommitSha("1".repeat(40)),
                head_sha: CommitSha("9".repeat(40)),
            },
            prompt: "Analyse the flow.".to_owned(),
            output_schema: serde_json::json!({"type": "object"}),
            model: ModelChoice {
                model: "gpt-6.1-sol".to_owned(),
                reasoning_effort: ReasoningEffort::High,
            },
            limits: JobLimits::default(),
            credential: LlmCredentialRef {
                id: CREDENTIAL.to_owned(),
                kind: CredentialKind::CodexChatgpt,
            },
            session: None,
            context_pack: None,
            skills: Vec::new(),
            access: AgentAccess::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use contracts::agent::JobKind;
    use contracts::job::spec::JobPriority;

    use super::testing::{self, seed_analysis};
    use super::*;
    use crate::repo::Db;
    use crate::repo::documents::DocumentKind;
    use crate::repo::jobs::{JobStage, NewJob};

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, 12, minute, 0)
            .single()
            .unwrap_or_default()
    }

    async fn db() -> Db {
        Db::in_memory().await.expect("in-memory database")
    }

    fn new_analysis(head: &str) -> NewAnalysis {
        NewAnalysis {
            id: AnalysisId::new_random(),
            tenant_id: "default".to_owned(),
            pull_request_id: 1,
            base_sha: CommitSha("1".repeat(40)),
            head_sha: CommitSha(head.repeat(40)),
            language: LlmLanguage::Pl,
            created_by: "u1".to_owned(),
        }
    }

    #[tokio::test]
    async fn create_starts_planning_and_conflicts_for_a_live_head() {
        let db = db().await;
        seed_analysis(&db).await.expect("seed");
        let created = db
            .analyses()
            .create(new_analysis("a"), at(1))
            .await
            .expect("create");
        assert_eq!(created.state, AnalysisState::Planning);
        assert_eq!(created.language, LlmLanguage::Pl);
        assert_eq!(created.session_state, SessionState::None);
        assert_eq!(created.created_at, at(1));
        assert_eq!(created.finished_at, None);
        assert!(matches!(
            db.analyses().create(new_analysis("a"), at(2)).await,
            Err(DbError::Conflict)
        ));
        db.analyses()
            .set_state(created.id, &AnalysisState::Cancelled, at(3))
            .await
            .expect("cancel");
        db.analyses()
            .create(new_analysis("a"), at(4))
            .await
            .expect("a cancelled analysis may be analysed again");
    }

    #[tokio::test]
    async fn base_sha_changes_only_before_jobs_and_documents() {
        let db = db().await;
        let merge_base = CommitSha("7".repeat(40));
        let id = seed_analysis(&db).await.expect("seed");
        db.analyses()
            .set_base_sha(id, &merge_base)
            .await
            .expect("set before any job");
        assert_eq!(
            db.analyses().get(id).await.expect("get").base_sha,
            merge_base
        );

        db.documents()
            .put(id, DocumentKind::Plan, "", &serde_json::json!({}), at(1))
            .await
            .expect("document");
        assert!(matches!(
            db.analyses().set_base_sha(id, &merge_base).await,
            Err(DbError::Conflict)
        ));

        let with_job = seed_analysis(&db).await.expect("seed");
        db.jobs()
            .enqueue(
                NewJob {
                    spec: testing::job_spec(with_job, JobKind::Plan),
                    priority: JobPriority::Batch,
                    stage: JobStage::Plan { batch: 1 },
                    generation: 0,
                },
                at(2),
            )
            .await
            .expect("enqueue");
        assert!(matches!(
            db.analyses().set_base_sha(with_job, &merge_base).await,
            Err(DbError::Conflict)
        ));
        assert!(matches!(
            db.analyses()
                .set_base_sha(AnalysisId::new_random(), &merge_base)
                .await,
            Err(DbError::NotFound)
        ));
    }

    #[tokio::test]
    async fn every_state_round_trips_and_terminal_states_set_finished_at() {
        let db = db().await;
        let id = seed_analysis(&db).await.expect("seed");
        let states = [
            (AnalysisState::Planning, false),
            (AnalysisState::Running { done: 2, total: 5 }, false),
            (
                AnalysisState::Paused {
                    resumes_at: Some(at(30)),
                },
                false,
            ),
            (AnalysisState::Paused { resumes_at: None }, false),
            (
                AnalysisState::Failed {
                    flow: Some(FlowRef {
                        id: FlowId("f2".to_owned()),
                        title: "Retry".to_owned(),
                    }),
                    message: "The agent output was not valid.".to_owned(),
                },
                true,
            ),
            (
                AnalysisState::Failed {
                    flow: None,
                    message: "Planning failed.".to_owned(),
                },
                true,
            ),
            (AnalysisState::Finished { total: 5 }, true),
            (AnalysisState::Cancelled, true),
        ];
        for (state, terminal) in states {
            db.analyses()
                .set_state(id, &state, at(5))
                .await
                .expect("set state");
            let record = db.analyses().get(id).await.expect("get");
            assert_eq!(record.state, state);
            assert_eq!(record.finished_at, terminal.then_some(at(5)), "{state:?}");
        }
        assert!(matches!(
            db.analyses()
                .set_state(AnalysisId::new_random(), &AnalysisState::Planning, at(5))
                .await,
            Err(DbError::NotFound)
        ));
    }

    #[tokio::test]
    async fn unfinished_and_latest_for_pr() {
        let db = db().await;
        let first = seed_analysis(&db).await.expect("seed");
        let second = db
            .analyses()
            .create(new_analysis("b"), at(1))
            .await
            .expect("create");
        let done = db
            .analyses()
            .create(new_analysis("c"), at(2))
            .await
            .expect("create");
        db.analyses()
            .set_state(done.id, &AnalysisState::Finished { total: 1 }, at(3))
            .await
            .expect("finish");
        let unfinished: Vec<AnalysisId> = db
            .analyses()
            .unfinished()
            .await
            .expect("unfinished")
            .into_iter()
            .map(|record| record.id)
            .collect();
        assert_eq!(unfinished, [first, second.id]);
        let latest = db
            .analyses()
            .latest_for_pr(1, LlmLanguage::Pl)
            .await
            .expect("latest");
        assert_eq!(latest.map(|record| record.id), Some(done.id));
        assert_eq!(
            db.analyses()
                .latest_for_pr(2, LlmLanguage::En)
                .await
                .expect("none"),
            None
        );
    }

    #[tokio::test]
    async fn session_lifecycle_is_acknowledged_once_and_never_revived() {
        let db = db().await;
        let id = seed_analysis(&db).await.expect("seed");
        db.analyses()
            .set_session(id, "thread-1", Some("abc123"))
            .await
            .expect("session");
        db.analyses()
            .set_session(id, "thread-1", None)
            .await
            .expect("session again");
        let record = db.analyses().get(id).await.expect("get");
        assert_eq!(record.session_state, SessionState::Active);
        assert_eq!(record.agent_thread_id.as_deref(), Some("thread-1"));
        assert_eq!(record.skills_commit.as_deref(), Some("abc123"));

        db.analyses().request_session_drop(id).await.expect("drop");
        assert_eq!(
            db.analyses()
                .pending_session_drops()
                .await
                .expect("pending"),
            [id]
        );
        db.analyses()
            .set_session(id, "thread-2", None)
            .await
            .expect("late report");
        assert_eq!(
            db.analyses().get(id).await.expect("get").session_state,
            SessionState::DropRequested
        );
        db.analyses()
            .ack_session_drops(&[id, AnalysisId::new_random()])
            .await
            .expect("ack");
        db.analyses()
            .ack_session_drops(&[id])
            .await
            .expect("re-ack");
        db.analyses()
            .request_session_drop(id)
            .await
            .expect("drop again");
        assert_eq!(
            db.analyses().get(id).await.expect("get").session_state,
            SessionState::Dropped
        );
        assert!(
            db.analyses()
                .pending_session_drops()
                .await
                .expect("pending")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn summary_joins_the_pull_request_at_the_analysed_head() {
        let db = db().await;
        let id = seed_analysis(&db).await.expect("seed");
        db.analyses()
            .set_state(id, &AnalysisState::Running { done: 1, total: 3 }, at(2))
            .await
            .expect("state");
        let summary = db.analyses().summary(id).await.expect("summary");
        let record = db.analyses().get(id).await.expect("get");
        assert_eq!(summary.id, id);
        assert_eq!(summary.state, AnalysisState::Running { done: 1, total: 3 });
        assert_eq!(summary.pr.repo.owner, "acme");
        assert_eq!(summary.pr.repo.name, "node-client");
        assert_eq!(summary.pr.number, 482);
        assert_eq!(summary.pr.title, "Retry RPC calls");
        assert_eq!(summary.pr.author, "alice");
        assert_eq!(summary.pr.head_sha, record.head_sha);
        assert_eq!(summary.pr.base_sha, record.base_sha);
        assert_eq!(summary.language, LlmLanguage::En);
        assert!(matches!(
            db.analyses().summary(AnalysisId::new_random()).await,
            Err(DbError::NotFound)
        ));
    }
}
