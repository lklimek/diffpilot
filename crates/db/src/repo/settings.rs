//! Repository for `settings`: what the Settings screen reads and writes besides the LLM call outcome
//! (`llm_credentials`): the user's language, the tenant's analysed repositories and credential usability.

use contracts::api::settings::ControlledOwners;
use contracts::common::{LlmLanguage, RepoRef};
use contracts::ids::AnalysisId;
use contracts::job::result::{JobFailureKind, JobOutcome, LlmCallOutcome};
use contracts::job::spec::{ModelChoice, ReasoningEffort};
use contracts::time::Timestamp;
use sqlx::{Row, SqlitePool};

use super::Db;
use super::jobs::{parse_ts, parse_wire, ts, wire_name};
use super::llm_credentials::last_call_from_row;
use crate::error::DbError;

/// Access to settings.
#[derive(Debug)]
pub struct SettingsRepo<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Settings: language, analysed repositories, credential usability.
    pub fn settings(&self) -> SettingsRepo<'_> {
        SettingsRepo { pool: &self.pool }
    }
}

/// A repository the tenant has analysed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysedRepo {
    /// Owner and name as GitHub spells them.
    pub repo: RepoRef,
    /// The newest analysis of the repository with a stored pre-pass document, if any.
    pub prepass_analysis: Option<AnalysisId>,
}

/// Typed cause of a failed LLM call, recorded with the call's outcome from the job result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallFailure {
    /// The provider's usage limit was reached.
    UsageLimited,
    /// The attempt failed with this kind.
    Failed(JobFailureKind),
}

impl CallFailure {
    /// The typed failure of a job outcome; `None` for a success or a cancel.
    pub fn of(outcome: &JobOutcome) -> Option<Self> {
        match outcome {
            JobOutcome::UsageLimited { .. } => Some(Self::UsageLimited),
            JobOutcome::Failed { kind, .. } => Some(Self::Failed(*kind)),
            JobOutcome::Succeeded { .. } | JobOutcome::Cancelled => None,
        }
    }

    /// The stored `llm_credentials.last_call_failure` value.
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::UsageLimited => "usage_limited",
            Self::Failed(JobFailureKind::ProviderFailed) => "provider_failed",
            Self::Failed(JobFailureKind::OutputNotJson) => "output_not_json",
            Self::Failed(JobFailureKind::CredentialUnusable) => "credential_unusable",
            Self::Failed(JobFailureKind::CheckoutFailed) => "checkout_failed",
            Self::Failed(JobFailureKind::Timeout) => "timeout",
            Self::Failed(JobFailureKind::Internal) => "internal",
            Self::Failed(JobFailureKind::Configuration) => "configuration",
            Self::Failed(JobFailureKind::WorkerUnavailable) => "worker_unavailable",
            Self::Failed(JobFailureKind::WorkerLost) => "worker_lost",
            Self::Failed(JobFailureKind::CredentialMissing) => "credential_missing",
            Self::Failed(JobFailureKind::CredentialRelink) => "credential_relink",
            Self::Failed(JobFailureKind::ModelUnavailable) => "model_unavailable",
            Self::Failed(JobFailureKind::UpstreamRejected) => "upstream_rejected",
            Self::Failed(JobFailureKind::ReportVersionUnsupported) => "report_version_unsupported",
            Self::Failed(JobFailureKind::InvalidJsonSchema) => "invalid_json_schema",
            Self::Failed(JobFailureKind::AskCompactionUnconfirmed) => "ask_compaction_unconfirmed",
        }
    }

    fn parse(value: &str) -> Result<Self, DbError> {
        Ok(match value {
            "usage_limited" => Self::UsageLimited,
            kind => Self::Failed(parse_wire(kind, "llm_credentials.last_call_failure")?),
        })
    }
}

/// The most recent LLM call of a tenant: its outcome and, when it failed, the typed cause if one was recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastCall {
    /// Time and outcome; a failure's message is the provider's raw text, for logs only.
    pub outcome: LlmCallOutcome,
    /// Typed cause of a failure.
    pub failure: Option<CallFailure>,
}

/// Ranking effort of the dashboard (`judge_work`) before anyone saved one (the column default).
pub const DEFAULT_JUDGE_EFFORT: ReasoningEffort = ReasoningEffort::Low;

/// Reasoning effort of Ask jobs before anyone saved one (the column default).
pub const DEFAULT_ASK_EFFORT: ReasoningEffort = ReasoningEffort::Medium;

/// Persisted installation selection; model allowlist belongs to the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredLlmSelection {
    /// Choice copied into new job specs.
    pub choice: ModelChoice,
    /// Reasoning effort of the dashboard ranking (`judge_work`).
    pub judge_effort: ReasoningEffort,
    /// Reasoning effort of Ask jobs.
    pub ask_effort: ReasoningEffort,
    /// Editor's login of the model, effort, ranking effort or Ask effort.
    pub changed_by: String,
    /// Save time of any of them.
    pub changed_at: Timestamp,
}

const SELECTION_SQL: &str = "SELECT * FROM llm_selection WHERE singleton = 1";

fn selection(row: &sqlx::sqlite::SqliteRow) -> Result<StoredLlmSelection, DbError> {
    Ok(StoredLlmSelection {
        choice: ModelChoice {
            model: row.try_get("model")?,
            reasoning_effort: parse_wire(
                &row.try_get::<String, _>("reasoning_effort")?,
                "llm_selection.reasoning_effort",
            )?,
        },
        judge_effort: parse_wire(
            &row.try_get::<String, _>("judge_effort")?,
            "llm_selection.judge_effort",
        )?,
        ask_effort: parse_wire(
            &row.try_get::<String, _>("ask_effort")?,
            "llm_selection.ask_effort",
        )?,
        changed_by: row.try_get("changed_by")?,
        changed_at: parse_ts(
            &row.try_get::<String, _>("changed_at")?,
            "llm_selection.changed_at",
        )?,
    })
}

impl SettingsRepo<'_> {
    /// The tenant's controlled GitHub owners, sorted case-insensitively, with who saved them last.
    pub async fn controlled_owners(&self, tenant_id: &str) -> Result<ControlledOwners, DbError> {
        let mut tx = super::read_tx(self.pool).await?;
        let owners = read_controlled_owners(&mut tx, tenant_id).await?;
        tx.commit().await?;
        Ok(owners)
    }

    /// Replaces the tenant's controlled owners (duplicates differing only in case are kept once) and records the
    /// editor; returns the previous and the saved list.
    pub async fn set_controlled_owners(
        &self,
        tenant_id: &str,
        owners: &[String],
        login: &str,
        now: Timestamp,
    ) -> Result<(ControlledOwners, ControlledOwners), DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let old = read_controlled_owners(&mut tx, tenant_id).await?;
        sqlx::query("DELETE FROM controlled_owners WHERE tenant_id = ?")
            .bind(tenant_id)
            .execute(&mut *tx)
            .await?;
        for owner in owners {
            // A case-only duplicate is kept once; a CHECK failure still aborts (`OR IGNORE` would skip it).
            sqlx::query(
                "INSERT INTO controlled_owners (tenant_id, owner) VALUES (?, ?) ON CONFLICT (tenant_id, owner) DO NOTHING",
            )
                .bind(tenant_id)
                .bind(owner)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "INSERT INTO controlled_owners_changed (tenant_id, changed_by, changed_at) VALUES (?, ?, ?)
             ON CONFLICT (tenant_id) DO UPDATE SET changed_by = excluded.changed_by, changed_at = excluded.changed_at",
        )
        .bind(tenant_id)
        .bind(login)
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;
        let saved = read_controlled_owners(&mut tx, tenant_id).await?;
        tx.commit().await?;
        Ok((old, saved))
    }

    /// Installation-wide choice, independent of tenant or requesting user.
    pub async fn llm_selection(&self) -> Result<Option<StoredLlmSelection>, DbError> {
        sqlx::query(SELECTION_SQL)
            .fetch_optional(self.pool)
            .await?
            .as_ref()
            .map(selection)
            .transpose()
    }

    /// Saves a choice (keeping the ranking effort) and returns the previous and new values under one write
    /// transaction.
    pub async fn set_llm_selection(
        &self,
        choice: &ModelChoice,
        login: &str,
        now: Timestamp,
    ) -> Result<(Option<StoredLlmSelection>, StoredLlmSelection), DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let old = read_selection(&mut tx).await?;
        sqlx::query("INSERT INTO llm_selection (singleton, model, reasoning_effort, changed_by, changed_at) VALUES (1, ?, ?, ?, ?) ON CONFLICT(singleton) DO UPDATE SET model=excluded.model, reasoning_effort=excluded.reasoning_effort, changed_by=excluded.changed_by, changed_at=excluded.changed_at")
            .bind(&choice.model).bind(wire_name(choice.reasoning_effort)?).bind(login).bind(ts(now)).execute(&mut *tx).await?;
        let new = read_selection(&mut tx).await?.ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok((old, new))
    }

    /// The installation-wide ranking effort of the dashboard; [`DEFAULT_JUDGE_EFFORT`] before the first save.
    pub async fn judge_effort(&self) -> Result<ReasoningEffort, DbError> {
        Ok(self
            .llm_selection()
            .await?
            .map_or(DEFAULT_JUDGE_EFFORT, |saved| saved.judge_effort))
    }

    /// Saves the ranking effort (R7) and returns the previous and new selection under one write transaction; the
    /// model choice is kept, or `default_choice` is stored with it when none was saved yet (the row needs one).
    /// That first save pins the shown default model as the saved choice.
    pub async fn set_judge_effort(
        &self,
        effort: ReasoningEffort,
        default_choice: &ModelChoice,
        login: &str,
        now: Timestamp,
    ) -> Result<(Option<StoredLlmSelection>, StoredLlmSelection), DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let old = read_selection(&mut tx).await?;
        sqlx::query(
            "INSERT INTO llm_selection (singleton, model, reasoning_effort, judge_effort, changed_by, changed_at)
             VALUES (1, ?, ?, ?, ?, ?)
             ON CONFLICT (singleton) DO UPDATE SET judge_effort = excluded.judge_effort,
                 changed_by = excluded.changed_by, changed_at = excluded.changed_at",
        )
        .bind(&default_choice.model)
        .bind(wire_name(default_choice.reasoning_effort)?)
        .bind(wire_name(effort)?)
        .bind(login)
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;
        let new = read_selection(&mut tx).await?.ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok((old, new))
    }

    /// The installation-wide reasoning effort of Ask jobs; [`DEFAULT_ASK_EFFORT`] before the first save.
    pub async fn ask_effort(&self) -> Result<ReasoningEffort, DbError> {
        Ok(self
            .llm_selection()
            .await?
            .map_or(DEFAULT_ASK_EFFORT, |saved| saved.ask_effort))
    }

    /// Saves the Ask effort and returns the previous and new selection under one write transaction; the model
    /// choice and ranking effort are kept, or `default_choice` is stored when none was saved yet (as
    /// [`Self::set_judge_effort`]).
    pub async fn set_ask_effort(
        &self,
        effort: ReasoningEffort,
        default_choice: &ModelChoice,
        login: &str,
        now: Timestamp,
    ) -> Result<(Option<StoredLlmSelection>, StoredLlmSelection), DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let old = read_selection(&mut tx).await?;
        sqlx::query(
            "INSERT INTO llm_selection (singleton, model, reasoning_effort, ask_effort, changed_by, changed_at)
             VALUES (1, ?, ?, ?, ?, ?)
             ON CONFLICT (singleton) DO UPDATE SET ask_effort = excluded.ask_effort,
                 changed_by = excluded.changed_by, changed_at = excluded.changed_at",
        )
        .bind(&default_choice.model)
        .bind(wire_name(default_choice.reasoning_effort)?)
        .bind(wire_name(effort)?)
        .bind(login)
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;
        let new = read_selection(&mut tx).await?.ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok((old, new))
    }
}

async fn read_selection(
    tx: &mut sqlx::Transaction<'static, sqlx::Sqlite>,
) -> Result<Option<StoredLlmSelection>, DbError> {
    sqlx::query(SELECTION_SQL)
        .fetch_optional(&mut **tx)
        .await?
        .as_ref()
        .map(selection)
        .transpose()
}

impl SettingsRepo<'_> {
    /// The tenant's most recent LLM call over all credentials; `None` before the first call.
    pub async fn last_call(&self, tenant_id: &str) -> Result<Option<LastCall>, DbError> {
        let row = sqlx::query(
            "SELECT last_call_at, last_call_ok, last_call_message, last_call_failure FROM llm_credentials
             WHERE tenant_id = ? AND last_call_at IS NOT NULL ORDER BY last_call_at DESC LIMIT 1",
        )
        .bind(tenant_id)
        .fetch_optional(self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let Some(outcome) = last_call_from_row(&row)? else {
            return Ok(None);
        };
        let failure: Option<String> = row.try_get("last_call_failure")?;
        Ok(Some(LastCall {
            outcome,
            failure: failure.as_deref().map(CallFailure::parse).transpose()?,
        }))
    }

    /// Sets the language of the user's LLM texts (used by later analyses); `NotFound` for an unknown user.
    pub async fn set_language(
        &self,
        user_id: &str,
        language: LlmLanguage,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let updated = sqlx::query("UPDATE users SET language = ?, updated_at = ? WHERE id = ?")
            .bind(wire_name(language)?)
            .bind(ts(now))
            .bind(user_id)
            .execute(self.pool)
            .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// The tenant's repositories, newest analysis first, then owner and name.
    pub async fn analysed_repos(&self, tenant_id: &str) -> Result<Vec<RepoRef>, DbError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT r.owner, r.name FROM repos r
             WHERE r.tenant_id = ?1 AND EXISTS (
                 SELECT 1 FROM analyses a JOIN pull_requests p ON p.id = a.pull_request_id
                 WHERE p.repo_id = r.id AND a.tenant_id = ?1)
             ORDER BY (SELECT MAX(COALESCE(a.finished_at, a.created_at)) FROM analyses a JOIN pull_requests p ON p.id=a.pull_request_id WHERE p.repo_id=r.id AND a.tenant_id=r.tenant_id) DESC, r.owner, r.name",
        )
        .bind(tenant_id)
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(owner, name)| RepoRef { owner, name })
            .collect())
    }

    /// The tenant's repository `repo` (owner and name match case-insensitively) if it has an analysis there.
    pub async fn analysed_repo(
        &self,
        tenant_id: &str,
        repo: &RepoRef,
    ) -> Result<Option<AnalysedRepo>, DbError> {
        let row = sqlx::query(
            "SELECT r.owner, r.name, (
                 SELECT a.id FROM analyses a JOIN pull_requests p ON p.id = a.pull_request_id
                 WHERE p.repo_id = r.id AND a.tenant_id = r.tenant_id AND EXISTS (
                     SELECT 1 FROM documents d WHERE d.analysis_id = a.id AND d.kind = 'prepass')
                 ORDER BY a.created_at DESC, a.rowid DESC LIMIT 1) AS prepass_analysis
             FROM repos r
             WHERE r.tenant_id = ?1 AND r.owner = ?2 AND r.name = ?3 AND EXISTS (
                 SELECT 1 FROM analyses a JOIN pull_requests p ON p.id = a.pull_request_id
                 WHERE p.repo_id = r.id AND a.tenant_id = ?1)",
        )
        .bind(tenant_id)
        .bind(&repo.owner)
        .bind(&repo.name)
        .fetch_optional(self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let prepass: Option<String> = row.try_get("prepass_analysis")?;
        Ok(Some(AnalysedRepo {
            repo: RepoRef {
                owner: row.try_get("owner")?,
                name: row.try_get("name")?,
            },
            prepass_analysis: prepass
                .map(|id| parse_wire(&id, "analyses.id"))
                .transpose()?,
        }))
    }

    /// Since when the tenant's most recently blocked LLM credential is unusable; `None` while all are usable.
    pub async fn credential_unusable_since(
        &self,
        tenant_id: &str,
    ) -> Result<Option<Timestamp>, DbError> {
        let since: Option<String> = sqlx::query_scalar(
            "SELECT MAX(relink_needed_at) FROM llm_credentials WHERE tenant_id = ?",
        )
        .bind(tenant_id)
        .fetch_one(self.pool)
        .await?;
        since
            .map(|at| parse_ts(&at, "llm_credentials.unusable_since"))
            .transpose()
    }
}

async fn read_controlled_owners(
    tx: &mut sqlx::Transaction<'static, sqlx::Sqlite>,
    tenant_id: &str,
) -> Result<ControlledOwners, DbError> {
    let owners: Vec<String> = sqlx::query_scalar(
        "SELECT owner FROM controlled_owners WHERE tenant_id = ? ORDER BY owner COLLATE NOCASE",
    )
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await?;
    let changed: Option<(String, String)> = sqlx::query_as(
        "SELECT changed_by, changed_at FROM controlled_owners_changed WHERE tenant_id = ?",
    )
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await?;
    let (changed_by, changed_at) = match changed {
        Some((by, at)) => (
            Some(by),
            Some(parse_ts(&at, "controlled_owners_changed.changed_at")?),
        ),
        None => (None, None),
    };
    Ok(ControlledOwners {
        owners,
        changed_by,
        changed_at,
    })
}

#[cfg(test)]
mod tests {
    use contracts::job::spec::{CredentialKind, LlmCredentialRef};
    use jiff::SignedDuration;

    use super::*;
    use crate::repo::DEFAULT_TENANT;
    use crate::repo::analyses::testing::{seed_analysis, seed_tenant};
    use crate::repo::documents::DocumentKind;

    fn acme(name: &str) -> RepoRef {
        RepoRef {
            owner: "acme".to_owned(),
            name: name.to_owned(),
        }
    }

    #[tokio::test]
    async fn controlled_owners_round_trip_sorted_and_unique() {
        let db = Db::in_memory().await.expect("db");
        let empty = db
            .settings()
            .controlled_owners(DEFAULT_TENANT)
            .await
            .expect("read");
        assert_eq!(empty, ControlledOwners::default(), "nothing saved yet");
        let now = Timestamp::now();
        let owners = ["dashpay", "Acme", "DashPay", "lklimek"].map(str::to_owned);
        let (old, saved) = db
            .settings()
            .set_controlled_owners(DEFAULT_TENANT, &owners, "lklimek", now)
            .await
            .expect("save");
        assert_eq!(old, ControlledOwners::default());
        assert_eq!(
            saved.owners,
            ["Acme", "dashpay", "lklimek"],
            "case-insensitive, first spelling kept"
        );
        assert_eq!(saved.changed_by.as_deref(), Some("lklimek"));
        assert!(saved.changed_at.is_some());
        let (old, replaced) = db
            .settings()
            .set_controlled_owners(DEFAULT_TENANT, &["x".to_owned()], "other", now)
            .await
            .expect("replace");
        assert_eq!(old, saved, "the previous list is returned");
        assert_eq!(replaced.owners, ["x"]);
        assert_eq!(replaced.changed_by.as_deref(), Some("other"));
        assert_eq!(
            db.settings()
                .controlled_owners(DEFAULT_TENANT)
                .await
                .expect("read"),
            replaced
        );
    }

    #[tokio::test]
    async fn controlled_owners_refuse_an_overlong_login() {
        let db = Db::in_memory().await.expect("db");
        let result = db
            .settings()
            .set_controlled_owners(
                DEFAULT_TENANT,
                &["x".repeat(40)],
                "lklimek",
                Timestamp::now(),
            )
            .await;
        assert!(result.is_err(), "{result:?}");
        let kept = db
            .settings()
            .controlled_owners(DEFAULT_TENANT)
            .await
            .expect("read");
        assert!(
            kept.owners.is_empty() && kept.changed_by.is_none(),
            "rolled back"
        );
    }

    #[tokio::test]
    async fn set_language_changes_the_user_language() {
        let db = Db::in_memory().await.expect("db");
        seed_analysis(&db).await.expect("seed");
        db.settings()
            .set_language("u1", LlmLanguage::Pl, Timestamp::now())
            .await
            .expect("set");
        let user = db.users().get("u1").await.expect("user");
        assert_eq!(user.language, LlmLanguage::Pl);
        let unknown = db
            .settings()
            .set_language("nobody", LlmLanguage::En, Timestamp::now())
            .await;
        assert!(matches!(unknown, Err(DbError::NotFound)), "{unknown:?}");
    }

    #[tokio::test]
    async fn analysed_repos_are_the_tenants_repos_with_an_analysis() {
        let db = Db::in_memory().await.expect("db");
        let settings = db.settings();
        assert!(
            settings
                .analysed_repos(DEFAULT_TENANT)
                .await
                .expect("none")
                .is_empty()
        );
        let analysis = seed_analysis(&db).await.expect("seed");
        // A resolved repository without an analysis and another tenant's repository are not listed.
        db.repos()
            .upsert(DEFAULT_TENANT, &acme("resolved-only"), None)
            .await
            .expect("repo");
        seed_tenant(&db, "other").await.expect("tenant");
        db.repos()
            .upsert("other", &acme("elsewhere"), None)
            .await
            .expect("repo");
        assert_eq!(
            settings
                .analysed_repos(DEFAULT_TENANT)
                .await
                .expect("repos"),
            [acme("node-client")]
        );
        assert!(
            settings
                .analysed_repos("other")
                .await
                .expect("other")
                .is_empty()
        );

        let found = settings
            .analysed_repo(DEFAULT_TENANT, &acme("Node-Client"))
            .await
            .expect("lookup");
        assert_eq!(
            found,
            Some(AnalysedRepo {
                repo: acme("node-client"),
                prepass_analysis: None,
            })
        );
        for (tenant, repo) in [
            (DEFAULT_TENANT, acme("resolved-only")),
            ("other", acme("node-client")),
            (DEFAULT_TENANT, acme("unknown")),
        ] {
            let found = settings.analysed_repo(tenant, &repo).await.expect("lookup");
            assert_eq!(found, None, "{tenant} {repo:?}");
        }

        db.documents()
            .put(
                analysis,
                DocumentKind::Prepass,
                "",
                &serde_json::json!({}),
                Timestamp::now(),
            )
            .await
            .expect("prepass");
        let found = settings
            .analysed_repo(DEFAULT_TENANT, &acme("node-client"))
            .await
            .expect("lookup");
        assert_eq!(found.and_then(|repo| repo.prepass_analysis), Some(analysis));
    }

    #[tokio::test]
    async fn repository_picker_orders_latest_analysis_then_name() {
        let db = Db::in_memory().await.unwrap();
        seed_analysis(&db).await.unwrap();
        for (name, at) in [
            ("aaa", "2026-11-16T00:00:00Z"),
            ("zzz", "2026-11-17T00:00:00Z"),
            ("bbb", "2026-11-16T00:00:00Z"),
        ] {
            let repo = db
                .repos()
                .upsert(DEFAULT_TENANT, &acme(name), None)
                .await
                .unwrap();
            sqlx::query("INSERT INTO pull_requests (tenant_id, repo_id, number, title, author, base_sha, head_sha, html_url, fetched_at) SELECT tenant_id, ?, number, title, author, base_sha, head_sha, html_url, fetched_at FROM pull_requests LIMIT 1")
                .bind(repo.id).execute(&db.pool).await.unwrap();
            let pr: i64 = sqlx::query_scalar("SELECT id FROM pull_requests WHERE repo_id=?")
                .bind(repo.id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO analyses (id,tenant_id,pull_request_id,created_by,base_sha,head_sha,language,state,created_at,background_analysis) SELECT ?,tenant_id,?,created_by,base_sha,head_sha,language,'planning',?,background_analysis FROM analyses LIMIT 1")
                .bind(contracts::ids::AnalysisId::new_random().to_string()).bind(pr).bind(at).execute(&db.pool).await.unwrap();
        }
        let repos = db.settings().analysed_repos(DEFAULT_TENANT).await.unwrap();
        let names: Vec<_> = repos
            .iter()
            .filter(|r| r.name != "node-client")
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(names, ["zzz", "aaa", "bbb"]);
    }

    #[tokio::test]
    async fn credential_unusable_since_follows_the_runner_report() {
        let db = Db::in_memory().await.expect("db");
        sqlx::query("INSERT INTO users(id,github_id,login,created_at,updated_at) VALUES ('codex-1',1,'codex-1','t','t')").execute(&db.pool).await.unwrap();
        let settings = db.settings();
        assert_eq!(
            settings
                .credential_unusable_since(DEFAULT_TENANT)
                .await
                .expect("none"),
            None
        );
        let credential = LlmCredentialRef {
            id: "codex-1".to_owned(),
            kind: CredentialKind::CodexChatgpt,
        };
        let credentials = db.llm_credentials();
        let at = "2026-11-17T08:00:00Z".parse::<Timestamp>().expect("time");
        credentials
            .register(DEFAULT_TENANT, &credential, true, at)
            .await
            .expect("usable");
        assert_eq!(
            settings
                .credential_unusable_since(DEFAULT_TENANT)
                .await
                .expect("usable"),
            None
        );
        credentials
            .register(DEFAULT_TENANT, &credential, false, at)
            .await
            .expect("unusable");
        assert_eq!(
            settings
                .credential_unusable_since(DEFAULT_TENANT)
                .await
                .expect("unusable"),
            Some(at)
        );
        assert_eq!(
            settings
                .credential_unusable_since("other")
                .await
                .expect("other tenant"),
            None
        );
    }
    #[tokio::test]
    async fn installation_selection_is_global_and_survives_repository_handles() {
        use contracts::job::spec::{ModelChoice, ReasoningEffort};
        let db = Db::in_memory().await.unwrap();
        assert!(db.settings().llm_selection().await.unwrap().is_none());
        let first = ModelChoice {
            model: "gpt-6-astra".into(),
            reasoning_effort: ReasoningEffort::Low,
        };
        let (old, saved) = db
            .settings()
            .set_llm_selection(&first, "alice", Timestamp::now())
            .await
            .unwrap();
        assert!(old.is_none());
        assert_eq!(saved.choice, first);
        assert_eq!(
            db.settings().llm_selection().await.unwrap(),
            Some(saved.clone())
        );
        let next = ModelChoice {
            reasoning_effort: ReasoningEffort::High,
            ..first
        };
        let (old, saved) = db
            .settings()
            .set_llm_selection(&next, "bob", Timestamp::now())
            .await
            .unwrap();
        assert_eq!(old.unwrap().changed_by, "alice");
        assert_eq!(saved.changed_by, "bob");
        assert_eq!(saved.choice, next);
    }

    #[tokio::test]
    async fn judge_effort_defaults_to_low_and_shares_the_change_pair() {
        let db = Db::in_memory().await.expect("db");
        let settings = db.settings();
        let at = |minute: i64| {
            "2026-11-18T06:00:00Z".parse::<Timestamp>().expect("time")
                + SignedDuration::from_mins(minute)
        };
        assert_eq!(
            settings.judge_effort().await.expect("default"),
            ReasoningEffort::Low
        );
        let default = ModelChoice {
            model: "gpt-6.1-sol".to_owned(),
            reasoning_effort: ReasoningEffort::High,
        };
        let (old, saved) = settings
            .set_judge_effort(ReasoningEffort::Medium, &default, "alice", at(1))
            .await
            .expect("first save");
        assert_eq!(old, None);
        assert_eq!(
            saved,
            StoredLlmSelection {
                choice: default.clone(),
                judge_effort: ReasoningEffort::Medium,
                ask_effort: DEFAULT_ASK_EFFORT,
                changed_by: "alice".to_owned(),
                changed_at: at(1),
            }
        );
        assert_eq!(settings.llm_selection().await.expect("read"), Some(saved));

        let astra = ModelChoice {
            model: "gpt-6-astra".to_owned(),
            reasoning_effort: ReasoningEffort::Low,
        };
        let (_, saved) = settings
            .set_llm_selection(&astra, "bob", at(2))
            .await
            .expect("model");
        assert_eq!(
            (saved.judge_effort, saved.changed_by.as_str()),
            (ReasoningEffort::Medium, "bob"),
            "a model change keeps the ranking effort"
        );

        let (old, saved) = settings
            .set_judge_effort(ReasoningEffort::High, &default, "carol", at(3))
            .await
            .expect("effort");
        assert_eq!(old.map(|old| old.choice), Some(astra.clone()));
        assert_eq!(
            saved,
            StoredLlmSelection {
                choice: astra,
                judge_effort: ReasoningEffort::High,
                ask_effort: DEFAULT_ASK_EFFORT,
                changed_by: "carol".to_owned(),
                changed_at: at(3),
            },
            "an effort change keeps the saved model choice"
        );
        assert_eq!(
            settings.judge_effort().await.expect("saved"),
            ReasoningEffort::High
        );
    }
    #[tokio::test]
    async fn ask_effort_defaults_to_medium_and_keeps_the_other_choices() {
        let db = Db::in_memory().await.expect("db");
        let settings = db.settings();
        let at = |minute: i64| {
            "2026-11-18T06:00:00Z".parse::<Timestamp>().expect("time")
                + SignedDuration::from_mins(minute)
        };
        assert_eq!(
            settings.ask_effort().await.expect("default"),
            ReasoningEffort::Medium
        );
        let default = ModelChoice {
            model: "gpt-6.1-sol".to_owned(),
            reasoning_effort: ReasoningEffort::High,
        };
        let (old, saved) = settings
            .set_ask_effort(ReasoningEffort::Low, &default, "alice", at(1))
            .await
            .expect("first save");
        assert_eq!(old, None);
        assert_eq!(
            saved,
            StoredLlmSelection {
                choice: default.clone(),
                judge_effort: DEFAULT_JUDGE_EFFORT,
                ask_effort: ReasoningEffort::Low,
                changed_by: "alice".to_owned(),
                changed_at: at(1),
            }
        );
        let astra = ModelChoice {
            model: "gpt-6-astra".to_owned(),
            reasoning_effort: ReasoningEffort::Medium,
        };
        settings
            .set_llm_selection(&astra, "bob", at(2))
            .await
            .expect("model");
        let (_, saved) = settings
            .set_judge_effort(ReasoningEffort::High, &default, "bob", at(3))
            .await
            .expect("ranking effort");
        assert_eq!(
            saved.ask_effort,
            ReasoningEffort::Low,
            "other saves keep the Ask effort"
        );
        let (_, saved) = settings
            .set_ask_effort(ReasoningEffort::High, &default, "carol", at(4))
            .await
            .expect("ask effort");
        assert_eq!(
            saved,
            StoredLlmSelection {
                choice: astra,
                judge_effort: ReasoningEffort::High,
                ask_effort: ReasoningEffort::High,
                changed_by: "carol".to_owned(),
                changed_at: at(4),
            },
            "an Ask effort change keeps the model choice and the ranking effort"
        );
        assert_eq!(
            settings.ask_effort().await.expect("saved"),
            ReasoningEffort::High
        );
    }
}
