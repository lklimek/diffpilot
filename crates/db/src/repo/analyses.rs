//! Repository for `analyses`. Frozen signatures (cross-WP); bodies are filled in by WP2-JOBS / WP2-INGEST.

use contracts::api::analyses::{
    AnalysisStage, AnalysisState, AnalysisSummary, FlowRef, MyAnalysis, StageFlow, StageKind,
};
use contracts::api::pr::PrSummary;
use contracts::common::{LlmLanguage, PrRef, RepoRef};
use contracts::ids::{AnalysisId, CommitSha, FlowId};
use contracts::job::result::JobOutcome;
use contracts::time::Timestamp;
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

use crate::error::DbError;
use crate::repo::jobs::{parse_ts, parse_wire, server_result, ts, wire_name};

/// Access to analyses.
#[derive(Debug)]
pub struct Analyses<'a> {
    pub(crate) pool: &'a SqlitePool,
    pub(crate) db: &'a super::Db,
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
    /// User currently paying for its analytic turns.
    pub payer_id: String,
    /// User who requested it.
    pub created_by: String,
    /// When it was requested.
    pub created_at: Timestamp,
    /// When it finished, failed or was cancelled.
    pub finished_at: Option<Timestamp>,
}

/// Lifecycle of a pull request's agent-session files on the runner (ADR-004).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionState {
    /// No job has run yet.
    None,
    /// The session exists and is resumed by later jobs.
    Active,
    /// Explicit removal: the runner is asked to delete the files.
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
                column: "pull_requests.session_state",
            }),
        }
    }
}

/// Columns of [`AnalysisRecord`]; queries alias `analyses` as `a`.
macro_rules! columns {
    () => {
        "a.id, a.tenant_id, a.pull_request_id, a.base_sha, a.head_sha, a.language, a.state,
    a.flows_total, a.flows_done, a.paused_until, a.user_paused_by, a.user_paused_at, a.failure_flow_id, a.failure_flow_title, a.failure_message, a.failure_reason,
    (SELECT thread_id FROM analytic_threads WHERE pull_request_id=a.pull_request_id AND user_id=COALESCE(a.payer_id,a.created_by)) AS agent_thread_id,
    COALESCE((SELECT state FROM analytic_threads WHERE pull_request_id=a.pull_request_id AND user_id=COALESCE(a.payer_id,a.created_by)), 'none') AS session_state, a.skills_commit, a.created_by, COALESCE(a.payer_id,a.created_by) AS payer_id, a.created_at, a.finished_at"
    };
}

/// The latest pause end (after `?1`, now) among the credentials of the analysis's waiting pipeline jobs.
macro_rules! credential_pause {
    () => {
        "(SELECT MAX(c.paused_until) FROM jobs j JOIN llm_credentials c ON c.user_id=j.payer_id
          WHERE j.analysis_id=analyses.id AND j.round=analyses.round AND j.kind<>'ask' AND j.state='queued'
            AND julianday(c.paused_until)>julianday(?1))"
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
    failure_reason: Option<contracts::api::error::ApiErrorCode>,
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
            failure_reason: None,
        };
        match state {
            AnalysisState::Planning { .. } => {}
            AnalysisState::Running { done, total, .. } => {
                columns.state = "running";
                columns.flows_done = *done;
                columns.flows_total = Some(*total);
            }
            AnalysisState::Paused {
                resumes_at,
                done,
                total,
                ..
            } => {
                columns.state = "paused";
                columns.paused_until = resumes_at.map(ts);
                columns.flows_done = *done;
                columns.flows_total = *total;
            }
            AnalysisState::Failed {
                reason,
                flow,
                message,
            } => {
                columns.state = "failed";
                columns.failure_reason = *reason;
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
        "planning" => AnalysisState::Planning { stage: None },
        "running" => AnalysisState::Running {
            done: row.try_get("flows_done")?,
            total: total.ok_or(corrupt)?,
            idle: false,
            stage: None,
        },
        "paused" => AnalysisState::Paused {
            reason: Some(
                if row
                    .try_get::<Option<String>, _>("user_paused_at")?
                    .is_some()
                {
                    contracts::api::analyses::PauseReason::User
                } else {
                    contracts::api::analyses::PauseReason::UsageLimit
                },
            ),
            paused_by: row.try_get("user_paused_by")?,
            paused_at: row
                .try_get::<Option<String>, _>("user_paused_at")?
                .map(|s| parse_ts(&s, "analyses.user_paused_at"))
                .transpose()?,
            done: row.try_get("flows_done")?,
            total,
            resumes_at: if row
                .try_get::<Option<String>, _>("user_paused_at")?
                .is_some()
            {
                None
            } else {
                row.try_get::<Option<String>, _>("paused_until")?
                    .map(|at| parse_ts(&at, "analyses.paused_until"))
                    .transpose()?
            },
            stage: None,
        },
        "failed" => {
            let id: Option<String> = row.try_get("failure_flow_id")?;
            let title: Option<String> = row.try_get("failure_flow_title")?;
            AnalysisState::Failed {
                reason: row
                    .try_get::<Option<String>, _>("failure_reason")?
                    .map(|kind| parse_wire(&kind, "analyses.failure_reason"))
                    .transpose()?,
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
        payer_id: row.try_get("payer_id")?,
        created_by: row.try_get("created_by")?,
        created_at: parse_ts(&created_at, "analyses.created_at")?,
        finished_at: finished_at
            .map(|at| parse_ts(&at, "analyses.finished_at"))
            .transpose()?,
    })
}

// Invalidate private seeds at admission, before a provider turn can publish the next checkpoint.
async fn advance_generation(
    tx: &mut sqlx::SqliteConnection,
    analysis: AnalysisId,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO analytic_threads(tenant_id,pull_request_id,user_id,generation,state)
        SELECT a.tenant_id,a.pull_request_id,COALESCE(a.payer_id,a.created_by),
            (SELECT COALESCE(MAX(generation),0)+1 FROM analytic_threads WHERE pull_request_id=a.pull_request_id),'none'
        FROM analyses a WHERE a.id=?
        ON CONFLICT(pull_request_id,user_id) DO UPDATE SET generation=excluded.generation,last_context_tokens=NULL,last_context_generation=NULL
        WHERE analytic_threads.state IN ('none','active')")
        .bind(analysis.to_string()).execute(tx).await?;
    Ok(())
}

impl Analyses<'_> {
    /// Creates an analysis in state `planning` with background analysis off (flows are analysed on request);
    /// `Conflict` if a live one exists for the head and language.
    pub async fn create(
        &self,
        analysis: NewAnalysis,
        now: Timestamp,
    ) -> Result<AnalysisRecord, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        sqlx::query(
            "INSERT INTO analyses (id, tenant_id, pull_request_id, base_sha, head_sha, language, state, created_by,
                 created_at, background_analysis,payer_id)
             VALUES (?, ?, ?, ?, ?, ?, 'planning', ?, ?, 0,?)",
        )
        .bind(analysis.id.to_string())
        .bind(&analysis.tenant_id)
        .bind(analysis.pull_request_id)
        .bind(&analysis.base_sha.0)
        .bind(&analysis.head_sha.0)
        .bind(wire_name(analysis.language)?)
        .bind(&analysis.created_by)
        .bind(ts(now))
        .bind(&analysis.created_by)
        .execute(&mut *tx)
        .await?;
        advance_generation(&mut tx, analysis.id).await?;
        tx.commit().await?;
        self.get(analysis.id).await
    }

    /// Changes the analysis payer only while no claimed/running job owns the current container.
    pub async fn set_payer(&self, id: AnalysisId, user: &str) -> Result<(), DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let changed=sqlx::query("UPDATE analyses SET payer_id=? WHERE id=? AND NOT EXISTS(SELECT 1 FROM jobs WHERE analysis_id=analyses.id AND state IN ('claimed','running'))")
            .bind(user).bind(id.to_string()).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(DbError::Conflict);
        }
        tx.commit().await?;
        Ok(())
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

    /// Analysis summary projected for a reader; shared storage never carries personal review state.
    pub async fn summary_for_user(
        &self,
        id: AnalysisId,
        user: &str,
    ) -> Result<AnalysisSummary, DbError> {
        let mut summary = self.summary(id).await?;
        summary.has_my_state = Some(super::review_state::exists(self.pool, user, id).await?);
        Ok(summary)
    }

    /// The analyses `user` requested in `tenant_id`, the latest per pull request, newest first, at most `limit`.
    /// Read-only.
    pub async fn mine(
        &self,
        tenant_id: &str,
        user: &str,
        limit: u32,
    ) -> Result<Vec<MyAnalysis>, DbError> {
        let rows = sqlx::query(
            "SELECT a.id, a.state, a.created_at, r.owner, r.name, p.number, p.title
             FROM (SELECT id, pull_request_id, state, created_at,
                       ROW_NUMBER() OVER (PARTITION BY pull_request_id ORDER BY created_at DESC, id DESC) AS n
                   FROM analyses WHERE tenant_id = ? AND created_by = ?) a
             JOIN pull_requests p ON p.id = a.pull_request_id JOIN repos r ON r.id = p.repo_id
             WHERE a.n = 1
             ORDER BY a.created_at DESC, a.id DESC
             LIMIT ?",
        )
        .bind(tenant_id)
        .bind(user)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let id: String = row.try_get("id")?;
                let state: String = row.try_get("state")?;
                let created_at: String = row.try_get("created_at")?;
                Ok(MyAnalysis {
                    id: parse_wire(&id, "analyses.id")?,
                    pr: PrRef {
                        repo: RepoRef {
                            owner: row.try_get("owner")?,
                            name: row.try_get("name")?,
                        },
                        number: row.try_get("number")?,
                    },
                    title: row.try_get("title")?,
                    status: parse_wire(&state, "analyses.state")?,
                    created_at: parse_ts(&created_at, "analyses.created_at")?,
                })
            })
            .collect()
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
        let mut record = record_from_row(&row)?;
        if let AnalysisState::Planning { stage }
        | AnalysisState::Running { stage, .. }
        | AnalysisState::Paused { stage, .. } = &mut record.state
        {
            *stage = self.live_stage(id).await?;
        }
        if let AnalysisState::Running { idle, .. } = &mut record.state {
            *idle = self.db.flow_work().parked(id).await?;
        }
        let mut summary = AnalysisSummary {
            review_ready: Some(sqlx::query_scalar("SELECT state <> 'cancelled' AND EXISTS(SELECT 1 FROM documents WHERE analysis_id=a.id AND kind='plan' AND key='') AND EXISTS(SELECT 1 FROM documents WHERE analysis_id=a.id AND kind='pr_context' AND key='') FROM analyses a WHERE id=?").bind(id.to_string()).fetch_one(self.pool).await?),
            background_analysis: Some(sqlx::query_scalar("SELECT background_analysis FROM analyses WHERE id=?").bind(id.to_string()).fetch_one(self.pool).await?),
            models_used: Some(self.db.jobs().models_used(id).await?),
            usage: self.db.jobs().usage(id).await?,
            progress: None,
            has_my_state: None,
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
            created_at: record.created_at,
            // Kept while a finished analysis runs again (cancelling returns to it), shown only once it ended.
            finished_at: record.finished_at.filter(|_| {
                !matches!(
                    record.state,
                    AnalysisState::Planning { .. }
                        | AnalysisState::Running { .. }
                        | AnalysisState::Paused { .. }
                )
            }),
            state: record.state,
        };
        self.add_progress(&mut summary).await?;
        let awaiting_retry: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM flow_work WHERE analysis_id=? AND error IS NOT NULL AND requested_at IS NULL)")
            .bind(id.to_string()).fetch_one(self.pool).await?;
        // With flows on request the wait ends when the review is ready: no whole-run figure afterwards.
        let on_request = summary.background_analysis == Some(false)
            && summary.review_ready == Some(true)
            && !matches!(summary.state, AnalysisState::Finished { .. });
        if let Some(progress) = &mut summary.progress {
            if on_request || awaiting_retry {
                progress.estimate = None;
            }
            if on_request {
                progress.percent = None;
            }
        }
        Ok(summary)
    }

    /// The stage of the analysis's live job: the claimed or running pipeline job of its current round, else the
    /// next queued or paused one in claim order (then not with the runner: no `started_at`). Ask jobs are no
    /// stage. A flow's index and title come from the stored plan.
    async fn live_stage(&self, id: AnalysisId) -> Result<Option<AnalysisStage>, DbError> {
        let row = sqlx::query(
            "SELECT j.kind, j.state, j.claimed_at, plan_flow.position, plan_flow.title
             FROM jobs j JOIN analyses a ON a.id = j.analysis_id AND a.round = j.round
             LEFT JOIN (SELECT CAST(f.key AS INTEGER) + 1 AS position, json_extract(f.value, '$.id') AS flow_id,
                               json_extract(f.value, '$.title') AS title
                        FROM documents d, json_each(d.body, '$.flows') f
                        WHERE d.analysis_id = ?1 AND d.kind = 'plan' AND d.key = '') plan_flow
                 ON plan_flow.flow_id = j.flow_id
             WHERE j.analysis_id = ?1 AND j.kind <> 'ask' AND j.state IN ('queued', 'claimed', 'running', 'paused')
             ORDER BY j.state IN ('claimed', 'running') DESC, j.priority, j.created_at, j.rowid
             LIMIT 1",
        )
        .bind(id.to_string())
        .fetch_optional(self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let kind = match row.try_get::<String, _>("kind")?.as_str() {
            "plan" => StageKind::Plan,
            "pr_context" => StageKind::PrContext,
            "analyse_flow" => StageKind::Flow,
            _ => {
                return Err(DbError::Corrupt {
                    column: "jobs.kind",
                });
            }
        };
        let with_runner = matches!(
            row.try_get::<String, _>("state")?.as_str(),
            "claimed" | "running"
        );
        let started_at = match row.try_get::<Option<String>, _>("claimed_at")? {
            Some(at) if with_runner => Some(parse_ts(&at, "jobs.claimed_at")?),
            _ => None,
        };
        let position: Option<u32> = row.try_get("position")?;
        let title: Option<String> = row.try_get("title")?;
        Ok(Some(AnalysisStage {
            part: None,
            kind,
            flow: position
                .zip(title)
                .filter(|_| kind == StageKind::Flow)
                .map(|(index, title)| StageFlow { index, title }),
            started_at,
        }))
    }

    /// The analysis of a pull request a reader is shown, any head: usable ones (not cancelled; a failed one can be
    /// retried) before cancelled ones, then `language` before another, then the newest.
    pub async fn latest_for_pr(
        &self,
        pull_request_id: i64,
        language: LlmLanguage,
    ) -> Result<Option<AnalysisRecord>, DbError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM analyses a WHERE a.pull_request_id = ?
             ORDER BY a.state <> 'cancelled' DESC, a.language = ? DESC, a.created_at DESC, a.rowid DESC LIMIT 1"
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

    /// Starts a new round of a failed analysis (explicit Retry): state `planning` and the round incremented, and in
    /// the same transaction every unfinished pipeline job of earlier rounds cancelled (queued and paused ones at
    /// once, claimed and running ones flagged), so no submit of the new round re-uses them; `ask` jobs are left
    /// alone (a question does not depend on the failed work); a `not_checked` MemCan profile
    /// lookup is deleted too, so the new round looks the profile up again. `false` when the analysis is not failed
    /// (nothing changed).
    pub async fn retry(&self, id: AnalysisId, now: Timestamp) -> Result<bool, DbError> {
        self.retry_inner(id, None, now).await
    }

    /// Atomically assigns the latest Retry's payer and starts its new round.
    pub async fn retry_as(
        &self,
        id: AnalysisId,
        payer: &str,
        now: Timestamp,
    ) -> Result<bool, DbError> {
        self.retry_inner(id, Some(payer), now).await
    }

    async fn retry_inner(
        &self,
        id: AnalysisId,
        payer: Option<&str>,
        now: Timestamp,
    ) -> Result<bool, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        if let Some(payer) = payer {
            let linked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM llm_credentials WHERE user_id=? AND state='linked')",
            )
            .bind(payer)
            .fetch_one(&mut *tx)
            .await?;
            if !linked {
                return Err(DbError::CredentialUnlinked);
            }
        }
        let tenant: Option<String> = sqlx::query_scalar(
            "UPDATE analyses SET state = 'planning', flows_total = NULL, flows_done = 0, paused_until = NULL,
                 failure_flow_id = NULL, failure_flow_title = NULL, failure_message = NULL, failure_reason = NULL, finished_at = NULL,
                 round = round + 1, payer_id = COALESCE(?,payer_id)
             WHERE id = ? AND state = 'failed'
                 AND (? IS NULL OR EXISTS(SELECT 1 FROM llm_credentials WHERE user_id=? AND state='linked'))
             RETURNING tenant_id",
        )
        .bind(payer)
        .bind(id.to_string())
        .bind(payer)
        .bind(payer)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(tenant) = tenant else {
            return Ok(false);
        };
        if payer.is_some() {
            advance_generation(&mut tx, id).await?;
        }
        let waiting: Vec<(String, u32)> = sqlx::query_as(
            "SELECT id, attempt FROM jobs WHERE analysis_id = ? AND kind <> 'ask' AND state IN ('queued', 'paused')",
        )
        .bind(id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        for (job, attempt) in waiting {
            let result =
                server_result(parse_wire(&job, "jobs.id")?, attempt, JobOutcome::Cancelled);
            sqlx::query(
                "UPDATE jobs SET state = 'cancelled', cancel_requested = 1, result = ?, finished_at = ?,
                     resumes_at = NULL, ready_output = NULL, ready_output_attempt = NULL
                 WHERE id = ?",
            )
            .bind(serde_json::to_string(&result)?)
            .bind(ts(now))
            .bind(&job)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "UPDATE jobs SET cancel_requested = 1
             WHERE analysis_id = ? AND kind <> 'ask' AND state IN ('claimed', 'running')",
        )
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM documents WHERE analysis_id = ? AND kind = 'memcan_profile'
                 AND json_extract(body, '$.memcan') = 'not_checked'",
        )
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.db.job_changes.publish(&tenant);
        Ok(true)
    }

    /// Cancelled analyses whose clean-up may be unfinished (a crash after the state was stored): an unfinished
    /// job not yet flagged for cancellation, or a stored document.
    pub async fn cancellations_to_finish(&self) -> Result<Vec<AnalysisId>, DbError> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT a.id FROM analyses a WHERE a.state = 'cancelled' AND (EXISTS (SELECT 1 FROM documents d WHERE d.analysis_id = a.id)
                 OR EXISTS (SELECT 1 FROM jobs j WHERE j.analysis_id = a.id AND j.cancel_requested = 0
                     AND j.state IN ('queued', 'claimed', 'running', 'paused')))
             ORDER BY a.created_at, a.rowid",
        )
        .fetch_all(self.pool)
        .await?;
        ids.iter().map(|id| parse_wire(id, "analyses.id")).collect()
    }

    /// User pause is persistent and does not cancel or change an in-flight job.
    pub async fn pause_by_user(
        &self,
        id: AnalysisId,
        login: &str,
        now: Timestamp,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query("UPDATE analyses SET state='paused', user_paused_by=COALESCE(user_paused_by, ?), user_paused_at=COALESCE(user_paused_at, ?) WHERE id=? AND state IN ('planning','running','paused')")
            .bind(login).bind(ts(now)).bind(id.to_string()).execute(self.pool).await?.rows_affected() > 0)
    }

    /// Clears only the user pause; the provider-limit gate remains in force: a paused job of the analysis, or a
    /// waiting pipeline job whose credential is paused (by any job), keeps the analysis `paused` until that time.
    pub async fn resume_by_user(&self, id: AnalysisId, now: Timestamp) -> Result<bool, DbError> {
        Ok(sqlx::query(concat!(
            "UPDATE analyses SET user_paused_at=NULL, user_paused_by=NULL,
            paused_until=COALESCE(", credential_pause!(), ", paused_until),
            state=CASE WHEN EXISTS (SELECT 1 FROM jobs j WHERE j.analysis_id=analyses.id AND j.round=analyses.round AND j.state='paused' AND (j.resumes_at IS NULL OR julianday(j.resumes_at)>julianday(?1)))
                OR ", credential_pause!(), " IS NOT NULL THEN 'paused' WHEN flows_total IS NULL THEN 'planning' ELSE 'running' END
            WHERE id=?2 AND user_paused_at IS NOT NULL AND state='paused'"))
            .bind(ts(now)).bind(id.to_string()).execute(self.pool).await?.rows_affected() > 0)
    }

    /// Stores a new state (progress included); terminal states set `finished_at` (the last finish), a live state
    /// keeps it (set only while an analysis that finished runs again for a flow).
    pub async fn set_state(
        &self,
        id: AnalysisId,
        state: &AnalysisState,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let columns = StateColumns::of(state);
        let updated = sqlx::query(
            "UPDATE analyses SET state = CASE WHEN user_paused_at IS NOT NULL AND ?1 NOT IN ('failed','cancelled') THEN 'paused' ELSE ?1 END,
                 flows_total = ?2, flows_done = ?3, paused_until = ?4, failure_flow_id = ?5,
                 failure_flow_title = ?6, failure_message = ?7, failure_reason = ?8,
                 finished_at = CASE WHEN ?9 IS NULL OR (user_paused_at IS NOT NULL AND ?1 = 'finished') THEN finished_at ELSE ?9 END,
                 user_paused_at = CASE WHEN ?1 IN ('failed','cancelled') THEN NULL ELSE user_paused_at END,
                 user_paused_by = CASE WHEN ?1 IN ('failed','cancelled') THEN NULL ELSE user_paused_by END
             WHERE id = ?10",
        )
        .bind(columns.state)
        .bind(columns.flows_total)
        .bind(columns.flows_done)
        .bind(&columns.paused_until)
        .bind(&columns.failure_flow_id)
        .bind(&columns.failure_flow_title)
        .bind(&columns.failure_message)
        .bind(columns.failure_reason.map(wire_name).transpose()?)
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
        let mut tx = super::write_tx(self.pool).await?;
        let updated = sqlx::query(
            "INSERT INTO analytic_threads(tenant_id,pull_request_id,user_id,thread_id,generation,state)
             SELECT tenant_id,pull_request_id,COALESCE(payer_id,created_by),?,1,'active' FROM analyses WHERE id=?
             ON CONFLICT(pull_request_id,user_id) DO UPDATE SET thread_id=excluded.thread_id,state='active',generation=analytic_threads.generation+1,
                 last_context_tokens=NULL,last_context_generation=NULL
             WHERE analytic_threads.state IN ('none','active')",
        ).bind(thread_id).bind(id.to_string()).execute(&mut *tx).await?;
        if updated.rows_affected() == 0 {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM analyses WHERE id=?)")
                    .bind(id.to_string())
                    .fetch_one(&mut *tx)
                    .await?;
            if !exists {
                return Err(DbError::NotFound);
            }
        }
        sqlx::query("UPDATE analyses SET skills_commit = COALESCE(?, skills_commit) WHERE id = ?")
            .bind(skills_commit)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Requests retirement of every payer container of the explicitly removed PR.
    pub async fn request_session_drop(&self, id: i64) -> Result<(), DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let updated = sqlx::query(
            "INSERT INTO analytic_threads(tenant_id,pull_request_id,user_id,generation,state)
             SELECT tenant_id,pull_request_id,COALESCE(payer_id,created_by),0,'drop_requested' FROM analyses WHERE pull_request_id=? GROUP BY COALESCE(payer_id,created_by)
             ON CONFLICT(pull_request_id,user_id) DO UPDATE SET state=CASE WHEN state='dropped' THEN state ELSE 'drop_requested' END",
        )
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }

    /// PRs ready for container retirement, with no attempt still running.
    pub async fn pending_session_drops(&self) -> Result<Vec<i64>, DbError> {
        let mut tx = super::read_tx(self.pool).await?;
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT t.pull_request_id FROM analytic_threads t WHERE t.state='drop_requested' AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.pull_request_id=t.pull_request_id AND j.state IN ('claimed','running')) ORDER BY t.pull_request_id",
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ids)
    }

    /// Acknowledges completed container/key retirement.
    pub async fn ack_session_drops(&self, ids: &[i64]) -> Result<(), DbError> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut tx = super::write_tx(self.pool).await?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "UPDATE analytic_threads SET state = 'dropped' WHERE state = 'drop_requested' AND pull_request_id IN (",
        );
        let mut list = query.separated(", ");
        for id in ids {
            list.push_bind(id.to_string());
        }
        query.push(")");
        query.build().execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Seed rows for tests of this crate and of the server (`test-util` feature); never in release builds.
#[cfg(any(test, feature = "test-util"))]
pub mod testing {
    use contracts::agent::JobKind;
    use contracts::ids::{AnalysisId, CommitSha, JobId, PassId, RefreshId};
    use contracts::job::access::AgentAccess;
    use contracts::job::spec::{
        JobLimits, JobSpec, JobTarget, ModelChoice, PullRequestCommentsTarget, PullRequestTarget,
        ReasoningEffort, RepoCheckout,
    };

    use crate::error::DbError;
    use crate::repo::{DEFAULT_TENANT, Db};

    /// Forgets that `id` ever finished: a fixture built as finished then set live models an analysis still
    /// running for the first time (not one running again after its finish).
    pub async fn never_finished(db: &Db, id: AnalysisId) -> Result<(), DbError> {
        sqlx::query("UPDATE analyses SET finished_at = NULL WHERE id = ?")
            .bind(id.to_string())
            .execute(&db.pool)
            .await?;
        Ok(())
    }

    /// Credential id used by [`job_spec`].
    pub const CREDENTIAL: &str = "u1";

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

    /// Inserts a user, the repository `acme/node-client`, PR #482 (once) and a new `planning` analysis of it with
    /// background analysis off, as created in production (flows run on request).
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
                 created_at, background_analysis)
             VALUES (?, ?, 1, ?, ?, 'en', 'planning', 'u1', '2026-11-16T00:00:00.000000Z', 0)",
        )
        .bind(id.to_string())
        .bind(DEFAULT_TENANT)
        .bind("1".repeat(40))
        .bind(format!("{}00000000", id.0.simple()))
        .execute(&db.pool)
        .await?;
        Ok(id)
    }

    /// Seeds a fake payer for queue/status tests that previously used opaque credential ids.
    pub async fn seed_payer(db: &Db, user: &str) -> Result<(), DbError> {
        sqlx::query("INSERT OR IGNORE INTO users(id,github_id,login,created_at,updated_at) SELECT ?,COALESCE(MAX(github_id),0)+1,?,'2026-11-23T00:00:00Z','2026-11-23T00:00:00Z' FROM users")
            .bind(user).bind(user).execute(&db.pool).await?;
        Ok(())
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
            target: JobTarget::PullRequest(PullRequestTarget {
                analysis_id: analysis,
                pull_request_id: 1,
                repo: RepoCheckout {
                    clone_url: "https://github.com/acme/node-client.git".to_owned(),
                    base_sha: CommitSha("1".repeat(40)),
                    head_sha: CommitSha("9".repeat(40)),
                },
            }),
            prompt: "Analyse the flow.".to_owned(),
            output_schema: serde_json::json!({"type": "object"}),
            flow_file: None,
            assessment: None,
            pr_data: None,
            model: ModelChoice {
                model: "gpt-6.1-sol".to_owned(),
                reasoning_effort: ReasoningEffort::High,
            },
            limits: JobLimits::default(),
            payer: contracts::ids::UserId(CREDENTIAL.to_owned()),
            session: None,
            working_session: None,
            context_pack: None,
            skills: Vec::new(),
            access: AgentAccess::default(),
        }
    }

    /// A `judge_work` spec for `refresh` without credentials, running with [`CREDENTIAL`].
    pub fn judge_spec(refresh: RefreshId) -> JobSpec {
        JobSpec {
            kind: JobKind::JudgeWork,
            target: JobTarget::Dashboard {
                refresh_id: refresh,
            },
            prompt: "Judge the items.".to_owned(),
            ..job_spec(AnalysisId::new_random(), JobKind::JudgeWork)
        }
    }

    /// A `verify_comments` spec for `pass` of PR #482 (id 1) without credentials, running with [`CREDENTIAL`].
    pub fn verify_spec(pass: PassId) -> JobSpec {
        let base = job_spec(AnalysisId::new_random(), JobKind::VerifyComments);
        let repo = base
            .target
            .checkout()
            .cloned()
            .unwrap_or_else(|| RepoCheckout {
                clone_url: String::new(),
                base_sha: CommitSha(String::new()),
                head_sha: CommitSha(String::new()),
            });
        JobSpec {
            target: JobTarget::PullRequestComments(PullRequestCommentsTarget {
                pull_request_id: 1,
                repo,
                pass_id: pass,
            }),
            prompt: "Assess the comments.".to_owned(),
            ..base
        }
    }

    /// Inserts user `u1`, PR #482 (id 1, once), its review-loop row and a queued pass charged to `u1`.
    pub async fn seed_pass(db: &Db) -> Result<PassId, DbError> {
        use crate::repo::review_loop::{NewPass, PassCause, PassStart};
        seed_analysis(db).await?;
        let now = contracts::time::Timestamp::now();
        db.review_loop().ensure_pr(DEFAULT_TENANT, 1, now).await?;
        let pass = NewPass {
            pull_request_id: 1,
            user_id: Some("u1".to_owned()),
            cause: PassCause::Opened,
            head: CommitSha("9".repeat(40)),
            sent: Vec::new(),
            question: None,
        };
        match db
            .review_loop()
            .start_pass(DEFAULT_TENANT, &pass, now)
            .await?
        {
            PassStart::Started(id) => Ok(id),
            PassStart::Busy(_) | PassStart::MarkedItem => Err(DbError::Conflict),
        }
    }

    /// Inserts user `u1` (once) and a refresh of it in `ranking` with an empty snapshot.
    pub async fn seed_ranking_refresh(db: &Db) -> Result<RefreshId, DbError> {
        let id = RefreshId::new_random();
        sqlx::raw_sql(
            "INSERT OR IGNORE INTO users (id, github_id, login, created_at, updated_at)
                 VALUES ('u1', 1, 'lklimek', '2026-11-16T00:00:00.000000Z', '2026-11-16T00:00:00.000000Z');",
        )
        .execute(&db.pool)
        .await?;
        sqlx::query(
            "INSERT INTO dashboard_refreshes (id, tenant_id, user_id, trigger, state, ranking_state, snapshot,
                 started_at, scanned_at)
             VALUES (?, ?, 'u1', 'manual', 'ranking', 'running', '{}', '2026-11-16T00:00:00.000000Z',
                 '2026-11-16T00:00:00.000000Z')",
        )
        .bind(id.to_string())
        .bind(DEFAULT_TENANT)
        .execute(&db.pool)
        .await?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {

    use contracts::agent::JobKind;
    use contracts::job::spec::JobPriority;

    use super::testing::{self, seed_analysis};
    use super::*;
    use crate::repo::documents::DocumentKind;
    use crate::repo::jobs::{JobStage, NewJob};
    use crate::repo::{DEFAULT_TENANT, Db};

    fn at(minute: u32) -> Timestamp {
        format!("2026-11-16T12:{minute:02}:00Z")
            .parse::<Timestamp>()
            .expect("time")
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

    /// Nothing is analysed until the user asks: a new analysis has background analysis off, whatever the
    /// column's default.
    #[tokio::test]
    async fn new_analysis_starts_with_background_analysis_off() {
        let db = db().await;
        seed_analysis(&db).await.expect("seed");
        let created = db
            .analyses()
            .create(new_analysis("a"), at(1))
            .await
            .expect("create");
        assert!(!db.flow_work().background(created.id).await.expect("read"));
        let summary = db.analyses().summary(created.id).await.expect("summary");
        assert_eq!(summary.background_analysis, Some(false));
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
        assert_eq!(created.state, AnalysisState::Planning { stage: None });
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

    fn failed() -> AnalysisState {
        AnalysisState::Failed {
            reason: None,
            flow: None,
            message: "Planning failed.".to_owned(),
        }
    }

    async fn profile_lookup(db: &Db, id: AnalysisId) -> Option<serde_json::Value> {
        db.documents()
            .get(id, DocumentKind::MemcanProfile, "")
            .await
            .expect("read")
    }

    #[tokio::test]
    async fn analysis_retry_publishes_after_cancelling_the_old_queued_job() {
        let db = db().await;
        let id = seed_analysis(&db).await.unwrap();
        enqueue(&db, id, JobStage::Plan { batch: 1 }, JobKind::Plan, 0).await;
        db.analyses().set_state(id, &failed(), at(1)).await.unwrap();
        let activity = db.jobs().subscribe_activity(DEFAULT_TENANT);

        assert!(db.analyses().retry(id, at(2)).await.unwrap());
        assert!(activity.has_changed().unwrap());
        assert!(
            db.jobs()
                .activity(DEFAULT_TENANT, "u1")
                .await
                .unwrap()
                .jobs
                .is_empty()
        );
    }

    /// A not-checked MemCan profile lookup is gone when the Retry's `planning` becomes visible, so no driver of
    /// the new round can read it; a checked one is kept.
    #[tokio::test]
    async fn retry_forgets_a_not_checked_profile_lookup_in_its_transaction() {
        let db = db().await;
        let unchecked = seed_analysis(&db).await.expect("seed");
        let checked = db
            .analyses()
            .create(new_analysis("c"), at(0))
            .await
            .expect("second")
            .id;
        let kept = serde_json::json!({"memcan": "checked", "text": null});
        for (id, body) in [
            (unchecked, serde_json::json!({"memcan": "not_checked"})),
            (checked, kept.clone()),
        ] {
            db.documents()
                .put(id, DocumentKind::MemcanProfile, "", &body, at(1))
                .await
                .expect("lookup");
            db.analyses()
                .set_state(id, &failed(), at(2))
                .await
                .expect("failed");
            assert!(db.analyses().retry(id, at(3)).await.expect("retry"));
        }
        assert_eq!(profile_lookup(&db, unchecked).await, None);
        assert_eq!(profile_lookup(&db, checked).await, Some(kept));
    }

    #[tokio::test]
    async fn retry_rolls_back_when_the_profile_lookup_cannot_be_forgotten() {
        let db = db().await;
        let id = seed_analysis(&db).await.expect("seed");
        let lookup = serde_json::json!({"memcan": "not_checked"});
        db.documents()
            .put(id, DocumentKind::MemcanProfile, "", &lookup, at(1))
            .await
            .expect("lookup");
        db.analyses()
            .set_state(id, &failed(), at(2))
            .await
            .expect("failed");
        sqlx::query(
            "CREATE TRIGGER keep_documents BEFORE DELETE ON documents BEGIN SELECT RAISE(ABORT, 'kept'); END",
        )
        .execute(&db.pool)
        .await
        .expect("trigger");
        let error = db.analyses().retry(id, at(3)).await.expect_err("aborted");
        assert!(error.sqlite_code().is_some(), "{error:?}");
        assert_eq!(db.analyses().get(id).await.expect("get").state, failed());
        assert_eq!(profile_lookup(&db, id).await, Some(lookup));
    }

    #[tokio::test]
    async fn every_state_round_trips_and_terminal_states_set_finished_at() {
        let db = db().await;
        let id = seed_analysis(&db).await.expect("seed");
        let states = [
            (AnalysisState::Planning { stage: None }, false),
            (
                AnalysisState::Running {
                    done: 2,
                    total: 5,
                    idle: false,
                    stage: None,
                },
                false,
            ),
            (
                AnalysisState::Paused {
                    reason: Some(contracts::api::analyses::PauseReason::UsageLimit),
                    paused_by: None,
                    paused_at: None,
                    resumes_at: Some(at(30)),
                    done: 2,
                    total: Some(5),
                    stage: None,
                },
                false,
            ),
            (
                AnalysisState::Paused {
                    reason: Some(contracts::api::analyses::PauseReason::UsageLimit),
                    paused_by: None,
                    paused_at: None,
                    resumes_at: None,
                    done: 0,
                    total: None,
                    stage: None,
                },
                false,
            ),
            (
                AnalysisState::Failed {
                    reason: None,
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
                    reason: Some(contracts::api::error::ApiErrorCode::Configuration),
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
                .set_state(
                    AnalysisId::new_random(),
                    &AnalysisState::Planning { stage: None },
                    at(5)
                )
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

    /// The lookup shows a pull request's analysis whatever language it was made in, and prefers the asked one.
    #[tokio::test]
    async fn latest_for_pr_prefers_the_language_and_falls_back_to_another() {
        let db = &db().await;
        let english = seed_analysis(db).await.expect("seed");
        let latest = |language| async move {
            db.analyses()
                .latest_for_pr(1, language)
                .await
                .expect("latest")
                .map(|record| record.id)
        };
        assert_eq!(latest(LlmLanguage::En).await, Some(english));
        assert_eq!(
            latest(LlmLanguage::Pl).await,
            Some(english),
            "no analysis in the asked language: the one that exists"
        );
        let polish = db
            .analyses()
            .create(new_analysis("b"), at(1))
            .await
            .expect("create");
        assert_eq!(polish.language, LlmLanguage::Pl);
        assert_eq!(latest(LlmLanguage::Pl).await, Some(polish.id));
        assert_eq!(
            latest(LlmLanguage::En).await,
            Some(english),
            "the older one in the asked language"
        );
        db.analyses()
            .set_state(polish.id, &AnalysisState::Cancelled, at(2))
            .await
            .expect("cancel");
        assert_eq!(
            latest(LlmLanguage::Pl).await,
            Some(english),
            "a usable analysis in another language before a cancelled one in the asked language"
        );
        let failed = AnalysisState::Failed {
            reason: None,
            flow: None,
            message: "The analysis stopped because of a server error. Retry the analysis.".into(),
        };
        db.analyses()
            .set_state(english, &failed, at(3))
            .await
            .expect("fail");
        assert_eq!(
            latest(LlmLanguage::Pl).await,
            Some(english),
            "a failed analysis can be retried: it is usable"
        );
        db.analyses()
            .set_state(english, &AnalysisState::Cancelled, at(4))
            .await
            .expect("cancel");
        assert_eq!(
            latest(LlmLanguage::Pl).await,
            Some(polish.id),
            "only cancelled ones: the asked language"
        );
    }

    #[tokio::test]
    async fn session_is_shared_by_new_heads_and_languages() {
        let db = db().await;
        let first = seed_analysis(&db).await.unwrap();
        db.analyses()
            .set_session(first, "shared-thread", None)
            .await
            .unwrap();
        let mut next = new_analysis("b");
        next.language = LlmLanguage::Pl;
        let second = db.analyses().create(next, at(1)).await.unwrap();
        assert_eq!(second.agent_thread_id.as_deref(), Some("shared-thread"));
        db.analyses()
            .set_session(second.id, "new-thread", None)
            .await
            .unwrap();
        assert_eq!(
            db.analyses()
                .get(first)
                .await
                .unwrap()
                .agent_thread_id
                .as_deref(),
            Some("new-thread")
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

        db.analyses().request_session_drop(1).await.expect("drop");
        assert_eq!(
            db.analyses()
                .pending_session_drops()
                .await
                .expect("pending"),
            [1]
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
            .ack_session_drops(&[1, 999])
            .await
            .expect("ack");
        db.analyses().ack_session_drops(&[1]).await.expect("re-ack");
        db.analyses()
            .request_session_drop(1)
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

    /// The stage of `summary`'s state, if the state carries one.
    fn stage_of(state: &AnalysisState) -> Option<&AnalysisStage> {
        match state {
            AnalysisState::Planning { stage }
            | AnalysisState::Running { stage, .. }
            | AnalysisState::Paused { stage, .. } => stage.as_ref(),
            _ => None,
        }
    }

    async fn stage(db: &Db, id: AnalysisId) -> Option<AnalysisStage> {
        let summary = db.analyses().summary(id).await.expect("summary");
        stage_of(&summary.state).cloned()
    }

    async fn enqueue(db: &Db, id: AnalysisId, stage: JobStage, kind: JobKind, minute: u32) {
        db.jobs()
            .enqueue(
                NewJob {
                    spec: testing::job_spec(id, kind),
                    priority: JobPriority::Batch,
                    stage,
                    generation: 0,
                },
                at(minute),
            )
            .await
            .expect("enqueue");
    }

    async fn claim(db: &Db, minute: u32) {
        db.jobs()
            .claim(crate::repo::jobs::ClaimParams {
                payer_ids: &[testing::CREDENTIAL.to_owned()],
                job_token_hash: b"hash",
                lease_until: at(minute + 30),
                now: at(minute),
            })
            .await
            .expect("claim")
            .expect("a job to claim");
    }

    /// Ends the claimed job as succeeded, as the runner's result would.
    async fn end_claimed(db: &Db, id: AnalysisId) {
        sqlx::query(
            "UPDATE jobs SET state = 'succeeded' WHERE analysis_id = ? AND state = 'claimed'",
        )
        .bind(id.to_string())
        .execute(&db.pool)
        .await
        .expect("end job");
    }

    fn waiting(kind: StageKind, flow: Option<(u32, &str)>) -> AnalysisStage {
        AnalysisStage {
            part: None,
            kind,
            flow: flow.map(|(index, title)| StageFlow {
                index,
                title: title.to_owned(),
            }),
            started_at: None,
        }
    }

    fn started(kind: StageKind, flow: Option<(u32, &str)>, minute: u32) -> AnalysisStage {
        AnalysisStage {
            started_at: Some(at(minute)),
            ..waiting(kind, flow)
        }
    }

    #[tokio::test]
    async fn summary_reports_the_stage_of_the_live_job() {
        let db = db().await;
        let id = seed_analysis(&db).await.expect("seed");
        assert_eq!(stage(&db, id).await, None, "no job yet");
        enqueue(&db, id, JobStage::Plan { batch: 1 }, JobKind::Plan, 1).await;
        assert_eq!(
            stage(&db, id).await,
            Some(waiting(StageKind::Plan, None)),
            "queued: waiting for the runner"
        );
        claim(&db, 2).await;
        assert_eq!(
            stage(&db, id).await,
            Some(started(StageKind::Plan, None, 2))
        );
        end_claimed(&db, id).await;
        let plan = serde_json::json!({"flows": [
            {"id": "f1", "title": "Parse the input"},
            {"id": "f2", "title": "Retry the send"},
        ], "unassigned": []});
        db.documents()
            .put(id, DocumentKind::Plan, "", &plan, at(3))
            .await
            .expect("plan");
        db.analyses()
            .set_state(
                id,
                &AnalysisState::Running {
                    done: 0,
                    total: 2,
                    idle: false,
                    stage: None,
                },
                at(3),
            )
            .await
            .expect("running");
        enqueue(&db, id, JobStage::PrContext, JobKind::PrContext, 4).await;
        for (minute, flow) in [(5, "f1"), (6, "f2")] {
            let flow = JobStage::Flow(FlowId(flow.to_owned()));
            enqueue(&db, id, flow, JobKind::AnalyseFlow, minute).await;
        }
        assert_eq!(
            stage(&db, id).await,
            Some(waiting(StageKind::PrContext, None))
        );
        claim(&db, 7).await;
        assert_eq!(
            stage(&db, id).await,
            Some(started(StageKind::PrContext, None, 7))
        );
        end_claimed(&db, id).await;
        claim(&db, 8).await;
        end_claimed(&db, id).await;
        claim(&db, 9).await;
        let flow_2 = started(StageKind::Flow, Some((2, "Retry the send")), 9);
        assert_eq!(stage(&db, id).await, Some(flow_2.clone()));
        let summary = db.analyses().summary(id).await.expect("summary");
        assert!(matches!(
            summary.state,
            AnalysisState::Running {
                done: 0,
                total: 2,
                ..
            }
        ));
        sqlx::query("UPDATE jobs SET state = 'paused' WHERE analysis_id = ? AND state = 'claimed'")
            .bind(id.to_string())
            .execute(&db.pool)
            .await
            .expect("pause");
        db.analyses()
            .set_state(
                id,
                &AnalysisState::Paused {
                    reason: Some(contracts::api::analyses::PauseReason::UsageLimit),
                    paused_by: None,
                    paused_at: None,
                    resumes_at: None,
                    done: 0,
                    total: Some(2),
                    stage: None,
                },
                at(10),
            )
            .await
            .expect("paused");
        assert_eq!(
            stage(&db, id).await,
            Some(waiting(StageKind::Flow, Some((2, "Retry the send")))),
            "a paused job is not with the runner"
        );
        sqlx::query("UPDATE analyses SET round = round + 1 WHERE id = ?")
            .bind(id.to_string())
            .execute(&db.pool)
            .await
            .expect("next round");
        assert_eq!(
            stage(&db, id).await,
            None,
            "jobs of an earlier round are not the stage"
        );
    }

    #[tokio::test]
    async fn finished_analysis_has_no_stage_and_unplanned_flow_has_no_title() {
        let db = db().await;
        let id = seed_analysis(&db).await.expect("seed");
        let flow = JobStage::Flow(FlowId("f9".to_owned()));
        enqueue(&db, id, flow, JobKind::AnalyseFlow, 1).await;
        db.analyses()
            .set_state(
                id,
                &AnalysisState::Running {
                    done: 0,
                    total: 1,
                    idle: false,
                    stage: None,
                },
                at(2),
            )
            .await
            .expect("running");
        assert_eq!(stage(&db, id).await, Some(waiting(StageKind::Flow, None)));
        db.analyses()
            .set_state(id, &AnalysisState::Finished { total: 1 }, at(3))
            .await
            .expect("finished");
        let summary = db.analyses().summary(id).await.expect("summary");
        assert_eq!(summary.state, AnalysisState::Finished { total: 1 });
    }

    #[tokio::test]
    async fn summary_joins_the_pull_request_at_the_analysed_head() {
        let db = db().await;
        let id = seed_analysis(&db).await.expect("seed");
        db.analyses()
            .set_state(
                id,
                &AnalysisState::Running {
                    done: 1,
                    total: 3,
                    idle: false,
                    stage: None,
                },
                at(2),
            )
            .await
            .expect("state");
        let summary = db.analyses().summary(id).await.expect("summary");
        let record = db.analyses().get(id).await.expect("get");
        assert_eq!(summary.id, id);
        assert_eq!(
            summary.state,
            AnalysisState::Running {
                done: 1,
                total: 3,
                idle: false,
                stage: None,
            }
        );
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

    /// Resume keeps the usage-limit pause of the shared credential: the analysis shows it with its reset time
    /// instead of an active state in which nothing can run.
    #[tokio::test]
    async fn resume_keeps_the_usage_limit_pause_of_the_shared_credential() {
        let db = db().await;
        let id = seed_analysis(&db).await.expect("seed");
        enqueue(&db, id, JobStage::Plan { batch: 1 }, JobKind::Plan, 0).await;
        db.llm_credentials()
            .register(
                "default",
                &contracts::ids::UserId(testing::job_spec(id, JobKind::Plan).payer.0),
                true,
                at(0),
            )
            .await
            .expect("credential");
        // Another analysis's job hit the limit of the credential both use.
        db.llm_credentials()
            .pause(testing::CREDENTIAL, at(50))
            .await
            .expect("pause");
        assert!(
            db.analyses()
                .pause_by_user(id, "alice", at(1))
                .await
                .expect("pause")
        );
        assert!(
            db.analyses()
                .resume_by_user(id, at(2))
                .await
                .expect("resume")
        );
        let summary = db.analyses().summary(id).await.expect("summary");
        assert!(
            matches!(
                summary.state,
                AnalysisState::Paused {
                    reason: Some(contracts::api::analyses::PauseReason::UsageLimit),
                    resumes_at: Some(until),
                    ..
                } if until == at(50)
            ),
            "{:?}",
            summary.state
        );
        // Once the limit has reset, Resume shows the active state.
        assert!(
            db.analyses()
                .pause_by_user(id, "alice", at(51))
                .await
                .expect("pause")
        );
        assert!(
            db.analyses()
                .resume_by_user(id, at(52))
                .await
                .expect("resume")
        );
        let summary = db.analyses().summary(id).await.expect("summary");
        assert!(
            matches!(summary.state, AnalysisState::Planning { .. }),
            "{:?}",
            summary.state
        );
    }
}

#[cfg(test)]
mod usage_tests {
    use jiff::SignedDuration;

    use super::*;
    use crate::repo::{
        Db,
        jobs::{JobStage, NewJob},
    };
    use contracts::ids::QuestionId;
    use contracts::job::spec::JobPriority;

    #[tokio::test]
    async fn summary_sums_stored_usage_across_rounds_and_ask_without_unfinished_jobs() {
        let db = Db::in_memory().await.unwrap();
        let id = testing::seed_analysis(&db).await.unwrap();
        for (round, stage, finished) in [
            (0, JobStage::Plan { batch: 1 }, true),
            (1, JobStage::Ask(QuestionId::new_random()), true),
            (1, JobStage::PrContext, false),
        ] {
            sqlx::query("UPDATE analyses SET round = ? WHERE id = ?")
                .bind(round)
                .bind(id.to_string())
                .execute(&db.pool)
                .await
                .unwrap();
            let job = db
                .jobs()
                .enqueue(
                    NewJob {
                        spec: testing::job_spec(id, stage.kind()),
                        stage,
                        generation: 0,
                        priority: JobPriority::Batch,
                    },
                    Timestamp::now(),
                )
                .await
                .unwrap();
            if finished {
                let result = super::super::jobs::server_result(
                    job.id,
                    1,
                    contracts::job::result::JobOutcome::Succeeded {
                        output: serde_json::json!({}),
                    },
                );
                let mut result = serde_json::to_value(result).unwrap();
                result["usage"] = serde_json::json!({"input_tokens": 100, "cached_input_tokens": 40, "output_tokens": 12});
                sqlx::query(
                    "UPDATE jobs SET state = 'succeeded', result = ?, finished_at = ? WHERE id = ?",
                )
                .bind(result.to_string())
                .bind(Timestamp::now().to_rfc3339_offset())
                .bind(job.id.to_string())
                .execute(&db.pool)
                .await
                .unwrap();
            }
        }
        let wire = serde_json::to_value(db.analyses().summary(id).await.unwrap()).unwrap();
        assert_eq!(
            wire["usage"],
            serde_json::json!({"input_tokens": 200, "cached_input_tokens": 80, "output_tokens": 24, "reasoning_output_tokens": null, "jobs": 2})
        );
        let empty = testing::seed_analysis(&db).await.unwrap();
        let wire = serde_json::to_value(db.analyses().summary(empty).await.unwrap()).unwrap();
        assert!(wire.get("usage").is_some_and(serde_json::Value::is_null));
    }
    #[tokio::test]
    async fn user_pause_survives_reopen_and_usage_limit_updates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pause.sqlite");
        let db = Db::open(&path).await.unwrap();
        let id = testing::seed_analysis(&db).await.unwrap();
        db.analyses()
            .pause_by_user(id, "alice", Timestamp::now())
            .await
            .unwrap();
        let reset = Timestamp::now() + SignedDuration::from_hours(3);
        db.analyses()
            .set_state(
                id,
                &AnalysisState::Paused {
                    reason: Some(contracts::api::analyses::PauseReason::UsageLimit),
                    paused_by: None,
                    paused_at: None,
                    resumes_at: Some(reset),
                    done: 1,
                    total: Some(5),
                    stage: None,
                },
                Timestamp::now(),
            )
            .await
            .unwrap();
        db.pool.close().await;
        let db = Db::open(&path).await.unwrap();
        let summary = db.analyses().summary(id).await.unwrap();
        let wire = serde_json::to_value(summary).unwrap();
        assert_eq!(wire["state"]["reason"], "user");
        assert_eq!(wire["state"]["paused_by"], "alice");
        assert_eq!(wire["state"]["resumes_at"], serde_json::Value::Null);
        assert_eq!(wire["state"]["done"], 1);
        assert!(
            db.analyses()
                .resume_by_user(id, Timestamp::now())
                .await
                .unwrap()
        );
        assert!(
            !db.analyses()
                .resume_by_user(id, Timestamp::now())
                .await
                .unwrap()
        );
    }
}

#[cfg(test)]
mod mine_tests {
    use contracts::api::analyses::AnalysisStatus;

    use super::testing::{seed_analysis, seed_tenant};
    use super::*;
    use crate::repo::Db;

    /// Seeds users u1 (via `seed_analysis`) and u2, tenant `other` with its own PR #7, and PR #483; returns
    /// `(pr_482, pr_483, other_tenant_pr)` row ids.
    async fn seed(db: &Db) -> (i64, i64, i64) {
        seed_analysis(db).await.unwrap();
        sqlx::query("DELETE FROM analyses")
            .execute(&db.pool)
            .await
            .unwrap();
        seed_tenant(db, "other").await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO users (id, github_id, login, created_at, updated_at)
                 VALUES ('u2', 2, 'colleague', '2026-11-16T00:00:00.000000Z', '2026-11-16T00:00:00.000000Z');
             INSERT INTO pull_requests (id, tenant_id, repo_id, number, title, author, html_url, base_sha, head_sha,
                 fetched_at)
               VALUES (2, 'default', 1, 483, 'Private: rotate keys', 'alice',
                 'https://github.com/acme/node-client/pull/483', 'b', 'h', '2026-11-16T00:00:00.000000Z');
             INSERT INTO repos (id, tenant_id, owner, name) VALUES (2, 'other', 'acme', 'secret');
             INSERT INTO pull_requests (id, tenant_id, repo_id, number, title, author, html_url, base_sha, head_sha,
                 fetched_at)
               VALUES (3, 'other', 2, 7, 'Other tenant', 'alice',
                 'https://github.com/acme/secret/pull/7', 'b', 'h', '2026-11-16T00:00:00.000000Z');",
        )
        .execute(&db.pool)
        .await
        .unwrap();
        (1, 2, 3)
    }

    async fn insert(
        db: &Db,
        tenant: &str,
        pr: i64,
        user: &str,
        state: &str,
        minute: u32,
    ) -> AnalysisId {
        let id = AnalysisId::new_random();
        sqlx::query(
            "INSERT INTO analyses (id, tenant_id, pull_request_id, base_sha, head_sha, language, state, flows_total,
                 created_by, created_at)
             VALUES (?, ?, ?, ?, ?, 'en', ?, CASE WHEN ? IN ('planning', 'cancelled') THEN NULL ELSE 3 END, ?, ?)",
        )
        .bind(id.to_string())
        .bind(tenant)
        .bind(pr)
        .bind("1".repeat(40))
        .bind(format!("{}00000000", id.0.simple()))
        .bind(state)
        .bind(state)
        .bind(user)
        .bind(format!("2026-11-16T12:{minute:02}:00.000000Z"))
        .execute(&db.pool)
        .await
        .unwrap();
        id
    }

    #[tokio::test]
    async fn lists_only_the_users_own_analyses_latest_per_pr_newest_first() {
        let db = Db::in_memory().await.unwrap();
        let (pr_482, pr_483, other_pr) = seed(&db).await;
        let older = insert(&db, "default", pr_482, "u1", "cancelled", 1).await;
        let latest = insert(&db, "default", pr_482, "u1", "finished", 5).await;
        let second = insert(&db, "default", pr_483, "u1", "running", 3).await;
        // u2's analyses: a newer one of the same PR and one of a PR u1 never analysed.
        insert(&db, "default", pr_482, "u2", "running", 9).await;
        let theirs = insert(&db, "default", pr_483, "u2", "failed", 8).await;
        // u1 in another tenant.
        insert(&db, "other", other_pr, "u1", "finished", 7).await;

        let mine = db.analyses().mine("default", "u1", 20).await.unwrap();
        let ids: Vec<_> = mine.iter().map(|row| row.id).collect();
        assert_eq!(ids, [latest, second]);
        assert!(!ids.contains(&older));
        assert_eq!(
            mine[0].pr,
            PrRef {
                repo: RepoRef {
                    owner: "acme".into(),
                    name: "node-client".into()
                },
                number: 482
            }
        );
        assert_eq!(mine[0].title, "Retry RPC calls");
        assert_eq!(mine[0].status, AnalysisStatus::Finished);
        assert_eq!(mine[1].status, AnalysisStatus::Running);
        assert_eq!(
            mine[0].created_at,
            "2026-11-16T12:05:00Z".parse::<Timestamp>().unwrap()
        );

        let colleague = db.analyses().mine("default", "u2", 20).await.unwrap();
        let ids: Vec<_> = colleague.iter().map(|row| row.id).collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[1], theirs);
        assert!(!ids.contains(&latest) && !ids.contains(&second));

        assert_eq!(
            db.analyses().mine("default", "u1", 1).await.unwrap().len(),
            1
        );
        assert!(
            db.analyses()
                .mine("default", "nobody", 20)
                .await
                .unwrap()
                .is_empty()
        );
        let other = db.analyses().mine("other", "u1", 20).await.unwrap();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].title, "Other tenant");
    }

    #[tokio::test]
    async fn every_stored_state_maps_to_a_status() {
        let db = Db::in_memory().await.unwrap();
        seed(&db).await;
        let states = [
            ("planning", AnalysisStatus::Planning),
            ("running", AnalysisStatus::Running),
            ("paused", AnalysisStatus::Paused),
            ("failed", AnalysisStatus::Failed),
            ("finished", AnalysisStatus::Finished),
            ("cancelled", AnalysisStatus::Cancelled),
        ];
        for (index, (state, status)) in states.into_iter().enumerate() {
            sqlx::query("DELETE FROM analyses")
                .execute(&db.pool)
                .await
                .unwrap();
            insert(
                &db,
                "default",
                1,
                "u1",
                state,
                u32::try_from(index).unwrap(),
            )
            .await;
            let mine = db.analyses().mine("default", "u1", 20).await.unwrap();
            assert_eq!(mine[0].status, status, "{state}");
        }
    }
}
