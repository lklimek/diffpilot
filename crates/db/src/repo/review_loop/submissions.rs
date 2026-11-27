//! Durable agent submission identities and supplemental annotations.

use contracts::api::access_tokens::AccessTokenId;
use contracts::api::review_loop::AgentAnnotation;
use contracts::api::review_loop::ItemState;
use sqlx::{Row, Sqlite, sqlite::SqliteRow};
use uuid::Uuid;

use super::{ObservedItem, ReviewLoop, UpsertOutcome, ts_col, upsert_item_in};
use crate::error::DbError;
use crate::repo::{sessions::timestamp, write_tx};
use contracts::time::Timestamp;

/// A submission's persisted destination and request identity.
#[derive(Debug)]
pub struct AgentSubmission {
    /// Public identifier of this submission.
    pub id: Uuid,
    /// The allocated C number for a standalone item.
    pub item_no: Option<u32>,
    /// The GitHub conversation of an annotation.
    pub conversation: Option<String>,
    /// Digest of the original request.
    pub fingerprint: String,
}

/// The content to assign to a submission key.
pub enum NewAgentComment<'a> {
    /// A new decision-queue item.
    Standalone(Box<ObservedItem>),
    /// Supplemental text, separate from its destination's decision state.
    Annotation {
        /// Exact GitHub conversation, scoped to this PR.
        conversation: &'a str,
        /// Exact original comment's canonical URL.
        target_url: &'a str,
        /// Agent's text.
        body: &'a str,
    },
}

/// Authenticated submission metadata.
pub struct NewAgentSubmission<'a> {
    /// Tenant of the authenticated agent token.
    pub tenant: &'a str,
    /// Admitted pull request.
    pub pr: i64,
    /// Token identifier, independent of the token's editable display name.
    pub token_id: &'a AccessTokenId,
    /// Client-generated retry key.
    pub key: Uuid,
    /// Digest of the original request.
    pub fingerprint: &'a str,
    /// Authenticated token name as of submission.
    pub agent: &'a str,
    /// Submission time.
    pub now: Timestamp,
}

fn submission_from_row(row: SqliteRow) -> Result<AgentSubmission, DbError> {
    let no: Option<i64> = row.try_get("item_no")?;
    Ok(AgentSubmission {
        id: uuid_col(&row)?,
        item_no: no
            .map(u32::try_from)
            .transpose()
            .map_err(|_| DbError::Corrupt { column: "item_no" })?,
        conversation: row.try_get("conversation")?,
        fingerprint: row.try_get("fingerprint")?,
    })
}

fn uuid_col(row: &SqliteRow) -> Result<Uuid, DbError> {
    Uuid::parse_str(row.try_get("id")?).map_err(|_| DbError::Corrupt { column: "id" })
}

async fn find(
    db: impl sqlx::Executor<'_, Database = Sqlite>,
    input: &NewAgentSubmission<'_>,
) -> Result<Option<AgentSubmission>, DbError> {
    sqlx::query(
        "SELECT s.id, s.conversation, s.fingerprint, i.item_no FROM agent_comment_submissions s
         LEFT JOIN comment_items i ON i.id = s.item_id
         WHERE s.tenant_id = ? AND s.pull_request_id = ? AND s.token_id = ? AND s.submission_key = ?",
    )
    .bind(input.tenant)
    .bind(input.pr)
    .bind(&input.token_id.0)
    .bind(input.key.to_string())
    .fetch_optional(db)
    .await?
    .map(submission_from_row)
    .transpose()
}

impl ReviewLoop<'_> {
    /// Finds a prior submission by tenant, PR, token and client key.
    pub async fn agent_submission(
        &self,
        input: &NewAgentSubmission<'_>,
    ) -> Result<Option<AgentSubmission>, DbError> {
        find(self.pool, input).await
    }

    /// Atomically fences duplicate submissions and assigns a C number only for a new standalone item.
    pub async fn submit_agent_comment(
        &self,
        input: &NewAgentSubmission<'_>,
        comment: NewAgentComment<'_>,
    ) -> Result<(AgentSubmission, bool), DbError> {
        let mut tx = write_tx(self.pool).await?;
        if let Some(previous) = find(&mut *tx, input).await? {
            tx.commit().await?;
            return Ok((previous, false));
        }
        let admitted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM comment_prs WHERE pull_request_id = ? AND tenant_id = ?)",
        )
        .bind(input.pr)
        .bind(input.tenant)
        .fetch_one(&mut *tx)
        .await?;
        if !admitted {
            return Err(DbError::NotFound);
        }
        let id = Uuid::new_v4();
        let (item_id, item_no, conversation, target_url, annotation) = match comment {
            NewAgentComment::Standalone(mut item) => {
                item.stable_key = format!("api:{id}");
                let UpsertOutcome::New(item) = upsert_item_in(
                    &mut tx,
                    input.tenant,
                    input.pr,
                    &item,
                    ItemState::Undecided,
                    input.now,
                )
                .await?
                else {
                    return Err(DbError::Conflict);
                };
                (Some(item.id.to_string()), Some(item.no), None, None, None)
            }
            NewAgentComment::Annotation {
                conversation,
                target_url,
                body,
            } => (None, None, Some(conversation), Some(target_url), Some(body)),
        };
        sqlx::query(
            "INSERT INTO agent_comment_submissions
             (id, tenant_id, pull_request_id, token_id, submission_key, fingerprint, agent, created_at,
              item_id, conversation, target_url, annotation) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(input.tenant)
        .bind(input.pr)
        .bind(&input.token_id.0)
        .bind(input.key.to_string())
        .bind(input.fingerprint)
        .bind(input.agent)
        .bind(timestamp(input.now))
        .bind(item_id)
        .bind(conversation)
        .bind(target_url)
        .bind(annotation)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((
            AgentSubmission {
                id,
                item_no,
                conversation: conversation.map(str::to_owned),
                fingerprint: input.fingerprint.to_owned(),
            },
            true,
        ))
    }

    /// Reads the PR's supplemental annotations without altering their destination.
    pub async fn agent_annotations(
        &self,
        tenant: &str,
        pr: i64,
    ) -> Result<Vec<(String, AgentAnnotation)>, DbError> {
        let rows = sqlx::query(
            "SELECT id, conversation, target_url, annotation, agent, created_at
             FROM agent_comment_submissions
             WHERE tenant_id = ? AND pull_request_id = ? AND conversation IS NOT NULL
             ORDER BY created_at, id",
        )
        .bind(tenant)
        .bind(pr)
        .fetch_all(self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.try_get("conversation")?,
                    AgentAnnotation {
                        id: uuid_col(&row)?,
                        target_url: row.try_get("target_url")?,
                        agent: row.try_get("agent")?,
                        body: row.try_get("annotation")?,
                        created_at: ts_col(&row, "created_at")?,
                    },
                ))
            })
            .collect()
    }
}
