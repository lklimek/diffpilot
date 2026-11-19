//! Repository for `questions`: Ask questions on steps or lines of an analysis and their LLM answers.

use contracts::agent::ask::AskOutput;
use contracts::api::questions::{AskAnchor, AskQuestion, AskState};
use contracts::common::LineRange;
use contracts::ids::{AnalysisId, JobId, QuestionId, StepId};
use contracts::time::Timestamp;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};

use super::Db;
use super::jobs::{parse_ts, parse_wire, ts, wire_name};
use crate::error::DbError;

/// Access to questions.
#[derive(Debug)]
pub struct Questions<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Ask questions and answers.
    pub fn questions(&self) -> Questions<'_> {
        Questions { pool: &self.pool }
    }
}

/// A question to store; the tenant is the analysis's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewQuestion {
    /// Question id.
    pub id: QuestionId,
    /// The analysis.
    pub analysis_id: AnalysisId,
    /// What the question is about (a step: its canonical Tour id).
    pub anchor: AskAnchor,
    /// Who asked.
    pub user_id: String,
    /// The question text (user input, stored as given).
    pub text: String,
}

/// Limits checked in the same statement as the insert, so concurrent requests cannot exceed them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuestionLimits {
    /// Questions one analysis may hold.
    pub per_analysis: u32,
    /// Questions one user may ask from `user_window_start` on.
    pub per_user: u32,
    /// Start of the per-user window.
    pub user_window_start: Timestamp,
}

/// Outcome of [`Questions::create`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// Stored as `pending`.
    Stored(Box<QuestionRecord>),
    /// The analysis holds `per_analysis` questions already.
    AnalysisLimit,
    /// The user asked `per_user` questions in the window already.
    UserLimit,
}

/// A stored question with what only the server needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionRecord {
    /// The question as the API shows it.
    pub question: AskQuestion,
    /// The analysis.
    pub analysis_id: AnalysisId,
    /// Who asked.
    pub user_id: String,
    /// The `ask` job, once submitted.
    pub job_id: Option<JobId>,
    /// When the job's result confirmed the answer; `None` while an `answered` question shows a ready output.
    pub answered_at: Option<Timestamp>,
    /// Tour steps the asked lines touch, chosen when the job was submitted; `None` until then (or for a step).
    pub touched_steps: Option<Vec<StepId>>,
}

/// A state change of a question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionUpdate {
    /// Queued (again), e.g. waiting for the usage limit.
    Pending,
    /// The agent works on it.
    Answering,
    /// Answered with a checked ready output before the job's result, which may still replace or withdraw it.
    Provisional(AskOutput),
    /// Answered with the checked output of the job's result: final.
    Answered(AskOutput),
    /// Failed with a plain sentence for the user.
    Failed(String),
}

/// Columns of a question with `resumes_at`: while pending, the reset time of its paused job or of its queued
/// job's paused credential, if still ahead (`?1` = now).
macro_rules! question_select {
    () => {
        "SELECT q.id, q.analysis_id, q.step_id, q.path, q.side, q.start_line, q.end_line, q.user_id, q.text, q.state, q.answer, q.failure, q.job_id,
             q.created_at, q.answered_at, q.touched_steps, u.login AS asked_by,
             CASE WHEN q.state = 'pending' THEN
                 CASE WHEN j.state = 'paused' AND j.resumes_at > ?1 THEN j.resumes_at
                      WHEN j.state = 'queued' AND c.paused_until > ?1 THEN c.paused_until END
             END AS resumes_at
         FROM questions q
         JOIN users u ON u.id = q.user_id
         LEFT JOIN jobs j ON j.id = q.job_id
         LEFT JOIN llm_credentials c ON c.id = j.credential_id"
    };
}

fn optional_ts(row: &SqliteRow, column: &'static str) -> Result<Option<Timestamp>, DbError> {
    row.try_get::<Option<String>, _>(column)?
        .map(|at| parse_ts(&at, column))
        .transpose()
}

fn record_from_row(row: &SqliteRow) -> Result<QuestionRecord, DbError> {
    let id: String = row.try_get("id")?;
    let analysis: String = row.try_get("analysis_id")?;
    let state: String = row.try_get("state")?;
    let created_at: String = row.try_get("created_at")?;
    let answer: Option<String> = row.try_get("answer")?;
    let job: Option<String> = row.try_get("job_id")?;
    let touched: Option<String> = row.try_get("touched_steps")?;
    let anchor = anchor_from_row(row)?;
    Ok(QuestionRecord {
        question: AskQuestion {
            asked_by: Some(row.try_get("asked_by")?),
            mine: None,
            id: parse_wire(&id, "questions.id")?,
            step_id: anchor.step_id().cloned(),
            anchor,
            text: row.try_get("text")?,
            state: parse_wire(&state, "questions.state")?,
            answer: answer.map(|text| serde_json::from_str(&text)).transpose()?,
            failure: row.try_get("failure")?,
            created_at: parse_ts(&created_at, "questions.created_at")?,
            resumes_at: optional_ts(row, "resumes_at")?,
        },
        analysis_id: parse_wire(&analysis, "questions.analysis_id")?,
        user_id: row.try_get("user_id")?,
        job_id: job
            .map(|job| parse_wire(&job, "questions.job_id"))
            .transpose()?,
        answered_at: optional_ts(row, "answered_at")?,
        touched_steps: touched
            .map(|ids| serde_json::from_str::<Vec<String>>(&ids))
            .transpose()?
            .map(|ids| ids.into_iter().map(StepId).collect()),
    })
}

/// The anchor columns of a question row (`CHECK`ed: exactly one of step and path; both line ends or none).
fn anchor_from_row(row: &SqliteRow) -> Result<AskAnchor, DbError> {
    let start: Option<u32> = row.try_get("start_line")?;
    let end: Option<u32> = row.try_get("end_line")?;
    let lines = start.zip(end).map(|(start_line, end_line)| LineRange {
        start_line,
        end_line,
    });
    let step: Option<String> = row.try_get("step_id")?;
    if let Some(step) = step {
        return Ok(AskAnchor::Step {
            step_id: StepId(step),
            lines,
        });
    }
    let path: Option<String> = row.try_get("path")?;
    let side: Option<String> = row.try_get("side")?;
    match (path, side) {
        (Some(path), Some(side)) => Ok(AskAnchor::Lines {
            path,
            side: parse_wire(&side, "questions.side")?,
            range: lines,
        }),
        _ => Err(DbError::Corrupt {
            column: "questions.path",
        }),
    }
}

/// Anchor columns (step, path, side, start line, end line) of `anchor`.
type AnchorColumns<'a> = (
    Option<&'a str>,
    Option<&'a str>,
    Option<String>,
    Option<u32>,
    Option<u32>,
);

fn anchor_columns(anchor: &AskAnchor) -> Result<AnchorColumns<'_>, DbError> {
    let ends = |lines: &Option<LineRange>| {
        (
            lines.map(|lines| lines.start_line),
            lines.map(|lines| lines.end_line),
        )
    };
    Ok(match anchor {
        AskAnchor::Step { step_id, lines } => {
            let (start, end) = ends(lines);
            (Some(step_id.0.as_str()), None, None, start, end)
        }
        AskAnchor::Lines { path, side, range } => {
            let (start, end) = ends(range);
            (
                None,
                Some(path.as_str()),
                Some(wire_name(side)?),
                start,
                end,
            )
        }
    })
}

impl Questions<'_> {
    /// Stores `question` as `pending` unless a limit is reached; `NotFound` for an unknown analysis.
    pub async fn create(
        &self,
        question: NewQuestion,
        limits: QuestionLimits,
        now: Timestamp,
    ) -> Result<Asked, DbError> {
        let (step, path, side, start, end) = anchor_columns(&question.anchor)?;
        let inserted = sqlx::query(
            "INSERT INTO questions (id, tenant_id, analysis_id, step_id, path, side, start_line, end_line, user_id,
                 text, state, created_at)
             SELECT ?1, a.tenant_id, a.id, ?2, ?10, ?11, ?12, ?13, ?3, ?4, 'pending', ?5 FROM analyses a WHERE a.id = ?6
                 AND (SELECT COUNT(*) FROM questions WHERE analysis_id = a.id AND user_id = ?3) < ?7
                 AND (SELECT COUNT(*) FROM questions WHERE user_id = ?3 AND created_at >= ?8) < ?9",
        )
        .bind(question.id.to_string())
        .bind(step)
        .bind(&question.user_id)
        .bind(&question.text)
        .bind(ts(now))
        .bind(question.analysis_id.to_string())
        .bind(limits.per_analysis)
        .bind(ts(limits.user_window_start))
        .bind(limits.per_user)
        .bind(path)
        .bind(side)
        .bind(start)
        .bind(end)
        .execute(self.pool)
        .await?;
        if inserted.rows_affected() == 1 {
            return Ok(Asked::Stored(Box::new(self.get(question.id, now).await?)));
        }
        let held: Option<u32> = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM questions q WHERE q.analysis_id = a.id AND q.user_id = ?2) FROM analyses a WHERE a.id = ?1",
        )
        .bind(question.analysis_id.to_string())
        .bind(&question.user_id)
        .fetch_optional(self.pool)
        .await?;
        match held {
            None => Err(DbError::NotFound),
            Some(held) if held >= limits.per_analysis => Ok(Asked::AnalysisLimit),
            Some(_) => Ok(Asked::UserLimit),
        }
    }

    /// The limit `user_id` has reached for a new question on `analysis` (`Asked::AnalysisLimit` or
    /// `Asked::UserLimit`), if any: a cheap check before costly validation; [`Self::create`] checks again.
    pub async fn limit_reached(
        &self,
        analysis: AnalysisId,
        user_id: &str,
        limits: QuestionLimits,
    ) -> Result<Option<Asked>, DbError> {
        let counts: Option<(u32, u32)> = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM questions WHERE analysis_id = a.id AND user_id = ?2),
                    (SELECT COUNT(*) FROM questions WHERE user_id = ?2 AND created_at >= ?3)
             FROM analyses a WHERE a.id = ?1",
        )
        .bind(analysis.to_string())
        .bind(user_id)
        .bind(ts(limits.user_window_start))
        .fetch_optional(self.pool)
        .await?;
        match counts {
            None => Err(DbError::NotFound),
            Some((held, _)) if held >= limits.per_analysis => Ok(Some(Asked::AnalysisLimit)),
            Some((_, recent)) if recent >= limits.per_user => Ok(Some(Asked::UserLimit)),
            Some(_) => Ok(None),
        }
    }

    /// One question; `resumes_at` as of `now`.
    pub async fn get(&self, id: QuestionId, now: Timestamp) -> Result<QuestionRecord, DbError> {
        let row = sqlx::query(concat!(question_select!(), " WHERE q.id = ?2"))
            .bind(ts(now))
            .bind(id.to_string())
            .fetch_one(self.pool)
            .await?;
        record_from_row(&row)
    }

    /// Every question of `analysis` in any state, oldest first; `resumes_at` as of `now`.
    pub async fn list(
        &self,
        analysis: AnalysisId,
        now: Timestamp,
    ) -> Result<Vec<AskQuestion>, DbError> {
        let rows = sqlx::query(concat!(
            question_select!(),
            " WHERE q.analysis_id = ?2 ORDER BY q.created_at, q.rowid"
        ))
        .bind(ts(now))
        .bind(analysis.to_string())
        .fetch_all(self.pool)
        .await?;
        rows.iter()
            .map(|row| record_from_row(row).map(|record| record.question))
            .collect()
    }

    /// Private questions of this reader only.
    pub async fn list_for_user(
        &self,
        analysis: AnalysisId,
        user: &str,
        now: Timestamp,
    ) -> Result<Vec<AskQuestion>, DbError> {
        let rows = sqlx::query(concat!(
            question_select!(),
            " WHERE q.analysis_id = ?2 AND q.user_id = ?3 ORDER BY q.created_at, q.rowid"
        ))
        .bind(ts(now))
        .bind(analysis.to_string())
        .bind(user)
        .fetch_all(self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let record = record_from_row(row)?;
                let mut question = record.question;
                question.mine = Some(record.user_id == user);
                Ok(question)
            })
            .collect()
    }

    /// Questions of every tenant whose end is not stored yet, oldest first: `pending`, `answering`, or answered
    /// provisionally (the job's result, possibly already recorded, decides).
    pub async fn unanswered(&self, now: Timestamp) -> Result<Vec<QuestionRecord>, DbError> {
        let rows = sqlx::query(concat!(
            question_select!(),
            " WHERE q.state IN ('pending', 'answering')
                 OR (q.state = 'answered' AND q.answered_at IS NULL)
             ORDER BY q.created_at, q.rowid"
        ))
        .bind(ts(now))
        .fetch_all(self.pool)
        .await?;
        rows.iter().map(record_from_row).collect()
    }

    /// Finally answered questions of `step` asked before `question`, oldest first, with their answers.
    pub async fn answered_before(
        &self,
        analysis: AnalysisId,
        step: &StepId,
        question: QuestionId,
    ) -> Result<Vec<(String, AskOutput)>, DbError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT q.text, q.answer FROM questions q, questions asked
             WHERE asked.id = ?3 AND q.user_id = asked.user_id AND q.analysis_id = ?1 AND q.step_id = ?2 AND q.state = 'answered'
                 AND q.answer IS NOT NULL AND q.answered_at IS NOT NULL AND q.rowid < asked.rowid
             ORDER BY q.rowid",
        )
        .bind(analysis.to_string())
        .bind(&step.0)
        .bind(question.to_string())
        .fetch_all(self.pool)
        .await?;
        rows.into_iter()
            .map(|(text, answer)| Ok((text, serde_json::from_str(&answer)?)))
            .collect()
    }

    /// The asker's earlier final answers across this PR, newest 20 only, returned chronologically.
    /// A foreign question id cannot select another user's history.
    pub async fn history_before(
        &self,
        question: QuestionId,
    ) -> Result<Vec<(String, AskOutput)>, DbError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT q.text, q.answer FROM questions q JOIN analyses a ON a.id = q.analysis_id,
                questions asked JOIN analyses current ON current.id = asked.analysis_id
             WHERE asked.id = ? AND q.user_id = asked.user_id AND a.pull_request_id = current.pull_request_id
                AND q.state = 'answered' AND q.answered_at IS NOT NULL AND q.answer IS NOT NULL
                AND q.rowid < asked.rowid ORDER BY q.rowid DESC LIMIT 20")
            .bind(question.to_string()).fetch_all(self.pool).await?;
        rows.into_iter()
            .rev()
            .map(|(text, answer)| Ok((text, serde_json::from_str(&answer)?)))
            .collect()
    }

    /// Links the question to its `ask` job.
    pub async fn set_job(&self, id: QuestionId, job: JobId) -> Result<(), DbError> {
        let updated = sqlx::query("UPDATE questions SET job_id = ? WHERE id = ?")
            .bind(job.to_string())
            .bind(id.to_string())
            .execute(self.pool)
            .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Stores the Tour steps the asked lines touch (before the job is submitted, so its claim finds them).
    pub async fn set_touched_steps(&self, id: QuestionId, steps: &[StepId]) -> Result<(), DbError> {
        let ids: Vec<&str> = steps.iter().map(|step| step.0.as_str()).collect();
        let updated = sqlx::query("UPDATE questions SET touched_steps = ? WHERE id = ?")
            .bind(serde_json::to_string(&ids)?)
            .bind(id.to_string())
            .execute(self.pool)
            .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Stores a state change; an answer replaces the failure and vice versa.
    pub async fn update(
        &self,
        id: QuestionId,
        update: &QuestionUpdate,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let (state, answer, failure, answered_at) = match update {
            QuestionUpdate::Pending => (AskState::Pending, None, None, None),
            QuestionUpdate::Answering => (AskState::Answering, None, None, None),
            QuestionUpdate::Provisional(output) => (
                AskState::Answered,
                Some(serde_json::to_string(output)?),
                None,
                None,
            ),
            QuestionUpdate::Answered(output) => (
                AskState::Answered,
                Some(serde_json::to_string(output)?),
                None,
                Some(ts(now)),
            ),
            QuestionUpdate::Failed(message) => {
                (AskState::Failed, None, Some(message.as_str()), None)
            }
        };
        let updated = sqlx::query(
            "UPDATE questions SET state = ?, answer = ?, failure = ?, answered_at = ? WHERE id = ?",
        )
        .bind(wire_name(state)?)
        .bind(answer)
        .bind(failure)
        .bind(answered_at)
        .bind(id.to_string())
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use contracts::agent::JobKind;
    use contracts::common::{Evidence, LineSide};
    use contracts::job::result::{JobOutcome, JobResult};
    use contracts::job::spec::{CredentialKind, JobPriority, LlmCredentialRef};
    use jiff::SignedDuration;

    use super::*;
    use crate::repo::DEFAULT_TENANT;
    use crate::repo::analyses::testing::{CREDENTIAL, job_spec, seed_analysis};
    use crate::repo::jobs::{ClaimParams, JobStage, NewJob};

    fn at(minute: i64) -> Timestamp {
        "2026-11-17T12:00:00Z"
            .parse::<Timestamp>()
            .unwrap_or_default()
            + SignedDuration::from_mins(minute)
    }

    const LIMITS: QuestionLimits = QuestionLimits {
        per_analysis: 3,
        per_user: 2,
        user_window_start: Timestamp::UNIX_EPOCH,
    };

    fn new_question(analysis: AnalysisId, step: &str, text: &str) -> NewQuestion {
        NewQuestion {
            id: QuestionId::new_random(),
            analysis_id: analysis,
            anchor: AskAnchor::Step {
                step_id: StepId(step.to_owned()),
                lines: None,
            },
            user_id: "u1".to_owned(),
            text: text.to_owned(),
        }
    }

    async fn ask(
        db: &Db,
        analysis: AnalysisId,
        step: &str,
        text: &str,
        now: Timestamp,
    ) -> QuestionId {
        match db
            .questions()
            .create(new_question(analysis, step, text), LIMITS, now)
            .await
            .expect("create")
        {
            Asked::Stored(record) => record.question.id,
            other => panic!("not stored: {other:?}"),
        }
    }

    fn output(text: &str) -> AskOutput {
        AskOutput {
            answer: text.to_owned(),
            evidence: vec![Evidence {
                file: "src/a.rs".to_owned(),
                side: LineSide::Head,
                line: 3,
            }],
        }
    }

    #[tokio::test]
    async fn private_question_quota_does_not_count_another_user() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        db.users()
            .upsert_member(
                DEFAULT_TENANT,
                "u2",
                &crate::repo::users::GithubProfile {
                    github_id: 2,
                    login: "colleague".to_owned(),
                    name: None,
                    avatar_url: None,
                },
                at(0),
            )
            .await
            .expect("user");
        let limits = QuestionLimits {
            per_analysis: 1,
            ..LIMITS
        };
        let a = new_question(analysis, "f1.1", "PRIVATE_A");
        assert!(matches!(
            db.questions().create(a, limits, at(0)).await.expect("A"),
            Asked::Stored(_)
        ));
        let mut b = new_question(analysis, "f1.1", "PRIVATE_B");
        b.user_id = "u2".to_owned();
        assert!(matches!(
            db.questions().create(b, limits, at(1)).await.expect("B"),
            Asked::Stored(_)
        ));
    }

    #[tokio::test]
    async fn private_questions_are_not_listed_for_another_user() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        ask(&db, analysis, "f1.1", "PRIVATE_MARKER", at(0)).await;
        assert!(
            db.questions()
                .list_for_user(analysis, "u2", at(1))
                .await
                .expect("list")
                .is_empty()
        );
        let own = db
            .questions()
            .list_for_user(analysis, "u1", at(1))
            .await
            .expect("list");
        assert_eq!(own.len(), 1);
        assert_eq!(own[0].mine, Some(true));
    }

    #[tokio::test]
    async fn question_is_stored_pending_per_tenant_and_text_kept_as_given() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let text = "<b>Is</b> this\nsent twice?";
        let id = ask(&db, analysis, "f1.2", text, at(0)).await;
        let record = db.questions().get(id, at(0)).await.expect("get");
        assert_eq!(record.question.state, AskState::Pending);
        assert_eq!(record.question.text, text);
        assert_eq!(record.question.step_id, Some(StepId("f1.2".to_owned())));
        assert_eq!(record.question.created_at, at(0));
        assert_eq!(record.user_id, "u1");
        assert_eq!(record.job_id, None);
        let tenant: String = sqlx::query_scalar("SELECT tenant_id FROM questions WHERE id = ?")
            .bind(id.to_string())
            .fetch_one(&db.pool)
            .await
            .expect("tenant");
        assert_eq!(tenant, DEFAULT_TENANT);
        let unknown = db
            .questions()
            .create(
                new_question(AnalysisId::new_random(), "f1.1", "q"),
                LIMITS,
                at(0),
            )
            .await;
        assert!(matches!(unknown, Err(DbError::NotFound)));
    }

    #[tokio::test]
    async fn anchors_round_trip_and_the_schema_refuses_mixed_ones() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let range = Some(LineRange {
            start_line: 100,
            end_line: 102,
        });
        let anchors = [
            AskAnchor::Step {
                step_id: StepId("f1.6".to_owned()),
                lines: range,
            },
            AskAnchor::Lines {
                path: "src/rpc/client.rs".to_owned(),
                side: LineSide::Base,
                range,
            },
            AskAnchor::Lines {
                path: "src/rpc/client.rs".to_owned(),
                side: LineSide::Head,
                range: None,
            },
        ];
        let roomy = QuestionLimits {
            per_analysis: 10,
            per_user: 10,
            ..LIMITS
        };
        for anchor in anchors {
            let mut question = new_question(analysis, "unused", "q");
            question.anchor = anchor.clone();
            let Asked::Stored(record) = db
                .questions()
                .create(question, roomy, at(0))
                .await
                .expect("create")
            else {
                panic!("not stored")
            };
            assert_eq!(record.question.anchor, anchor);
            assert_eq!(record.question.step_id, anchor.step_id().cloned());
        }
        for (columns, case) in [
            ("'f1.1', 'a.rs', 'head', NULL, NULL", "step and path"),
            ("NULL, NULL, NULL, NULL, NULL", "neither step nor path"),
            ("NULL, 'a.rs', NULL, NULL, NULL", "path without side"),
            ("'f1.1', NULL, NULL, 3, NULL", "one line end"),
            ("NULL, 'a.rs', 'head', 5, 4", "inverted range"),
        ] {
            let insert = format!(
                "INSERT INTO questions (id, tenant_id, analysis_id, step_id, path, side, start_line, end_line, user_id,
                     text, state, created_at)
                 VALUES ('{}', 'default', '{analysis}', {columns}, 'u1', 'q', 'pending', 't')",
                QuestionId::new_random()
            );
            assert!(
                sqlx::query(sqlx::AssertSqlSafe(insert))
                    .execute(&db.pool)
                    .await
                    .is_err(),
                "{case} is refused"
            );
        }
    }

    #[tokio::test]
    async fn limits_per_user_window_and_per_analysis() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        ask(&db, analysis, "f1.1", "a", at(0)).await;
        ask(&db, analysis, "f1.1", "b", at(1)).await;
        let third = db
            .questions()
            .create(new_question(analysis, "f1.1", "c"), LIMITS, at(2))
            .await
            .expect("create");
        assert_eq!(third, Asked::UserLimit);
        let later = QuestionLimits {
            user_window_start: at(1),
            ..LIMITS
        };
        let after_window = db
            .questions()
            .create(new_question(analysis, "f1.1", "c"), later, at(61))
            .await
            .expect("create");
        assert!(matches!(after_window, Asked::Stored(_)), "{after_window:?}");
        let full = QuestionLimits {
            user_window_start: at(100),
            ..LIMITS
        };
        let fourth = db
            .questions()
            .create(new_question(analysis, "f1.1", "d"), full, at(120))
            .await
            .expect("create");
        assert_eq!(fourth, Asked::AnalysisLimit);
    }

    #[tokio::test]
    async fn touched_steps_are_stored_with_the_question() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let id = ask(&db, analysis, "f1.1", "a", at(0)).await;
        let questions = db.questions();
        let read = || async { questions.get(id, at(1)).await.expect("get").touched_steps };
        assert_eq!(read().await, None);
        let steps = [StepId("f1.2".to_owned()), StepId("f2.1".to_owned())];
        questions
            .set_touched_steps(id, &steps)
            .await
            .expect("store");
        assert_eq!(read().await, Some(steps.to_vec()));
        questions
            .set_touched_steps(id, &[])
            .await
            .expect("store none");
        assert_eq!(read().await, Some(Vec::new()));
        assert!(matches!(
            questions
                .set_touched_steps(QuestionId::new_random(), &steps)
                .await,
            Err(DbError::NotFound)
        ));
    }

    #[tokio::test]
    async fn limit_reached_tells_the_limit_before_any_insert() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let questions = db.questions();
        let reached = |limits| questions.limit_reached(analysis, "u1", limits);
        assert_eq!(reached(LIMITS).await.expect("check"), None);
        ask(&db, analysis, "f1.1", "a", at(0)).await;
        ask(&db, analysis, "f1.1", "b", at(1)).await;
        assert_eq!(
            reached(LIMITS).await.expect("check"),
            Some(Asked::UserLimit)
        );
        let full = QuestionLimits {
            per_analysis: 2,
            user_window_start: at(100),
            ..LIMITS
        };
        assert_eq!(
            reached(full).await.expect("check"),
            Some(Asked::AnalysisLimit)
        );
        assert!(matches!(
            questions
                .limit_reached(AnalysisId::new_random(), "u1", LIMITS)
                .await,
            Err(DbError::NotFound)
        ));
    }

    #[tokio::test]
    async fn updates_store_answer_or_failure_and_list_is_oldest_first() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let other = seed_analysis(&db).await.expect("seed");
        let first = ask(&db, analysis, "f1.1", "first", at(0)).await;
        let second = ask(&db, analysis, "f2.1", "second", at(1)).await;
        let questions = db.questions();
        let elsewhere = questions
            .create(
                new_question(other, "f1.1", "elsewhere"),
                QuestionLimits {
                    per_user: 10,
                    ..LIMITS
                },
                at(1),
            )
            .await
            .expect("create");
        assert!(matches!(elsewhere, Asked::Stored(_)));
        questions
            .update(first, &QuestionUpdate::Answering, at(2))
            .await
            .expect("answering");
        questions
            .update(first, &QuestionUpdate::Answered(output("Does it?")), at(3))
            .await
            .expect("answered");
        questions
            .update(
                second,
                &QuestionUpdate::Failed("No answer.".to_owned()),
                at(3),
            )
            .await
            .expect("failed");
        let list = questions.list(analysis, at(4)).await.expect("list");
        assert_eq!(
            list.iter().map(|q| q.id).collect::<Vec<_>>(),
            [first, second]
        );
        assert_eq!(list[0].state, AskState::Answered);
        assert_eq!(list[0].answer, Some(output("Does it?")));
        assert_eq!(list[0].failure, None);
        assert_eq!(list[1].state, AskState::Failed);
        assert_eq!(list[1].failure.as_deref(), Some("No answer."));
        assert_eq!(list[1].answer, None);
        let record = questions.get(first, at(4)).await.expect("get");
        assert_eq!(record.answered_at, Some(at(3)));
        // A requeued attempt returns it to answering: the provisional answer goes.
        questions
            .update(first, &QuestionUpdate::Answering, at(5))
            .await
            .expect("answering again");
        let record = questions.get(first, at(5)).await.expect("get");
        assert_eq!((record.question.answer, record.answered_at), (None, None));
        let unanswered = questions.unanswered(at(5)).await.expect("unanswered");
        assert_eq!(
            unanswered.len(),
            2,
            "first (answering) and the other analysis's question"
        );
        assert!(matches!(
            questions
                .update(QuestionId::new_random(), &QuestionUpdate::Pending, at(5))
                .await,
            Err(DbError::NotFound)
        ));
    }

    #[tokio::test]
    async fn answered_before_lists_earlier_answers_of_the_step_only() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let questions = db.questions();
        let limits = QuestionLimits {
            per_user: 10,
            per_analysis: 10,
            ..LIMITS
        };
        let mut ids = Vec::new();
        for (step, text) in [
            ("f1.1", "one"),
            ("f1.2", "other step"),
            ("f1.1", "two"),
            ("f1.1", "now"),
        ] {
            let Asked::Stored(record) = questions
                .create(new_question(analysis, step, text), limits, at(0))
                .await
                .expect("create")
            else {
                panic!("stored");
            };
            ids.push(record.question.id);
        }
        for id in &ids[..3] {
            questions
                .update(*id, &QuestionUpdate::Answered(output("A?")), at(1))
                .await
                .expect("answered");
        }
        let earlier = questions
            .answered_before(analysis, &StepId("f1.1".to_owned()), ids[3])
            .await
            .expect("earlier");
        assert_eq!(
            earlier,
            [
                ("one".to_owned(), output("A?")),
                ("two".to_owned(), output("A?"))
            ]
        );
        let none = questions
            .answered_before(analysis, &StepId("f1.1".to_owned()), ids[0])
            .await
            .expect("earlier");
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn resumes_at_while_the_job_or_its_credential_is_paused() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let id = ask(&db, analysis, "f1.1", "q", at(0)).await;
        let resumes = |now| {
            let db = db.clone();
            async move {
                db.questions()
                    .get(id, now)
                    .await
                    .expect("get")
                    .question
                    .resumes_at
            }
        };
        assert_eq!(resumes(at(0)).await, None, "no job yet");
        let job = db
            .jobs()
            .enqueue(
                NewJob {
                    spec: job_spec(analysis, JobKind::Ask),
                    priority: JobPriority::Interactive,
                    stage: JobStage::Ask(id),
                    generation: 0,
                },
                at(0),
            )
            .await
            .expect("enqueue");
        db.questions().set_job(id, job.id).await.expect("link");
        let credential = LlmCredentialRef {
            id: CREDENTIAL.to_owned(),
            kind: CredentialKind::CodexChatgpt,
        };
        db.llm_credentials()
            .register(DEFAULT_TENANT, &credential, true, at(0))
            .await
            .expect("register");
        db.llm_credentials()
            .pause(CREDENTIAL, at(30))
            .await
            .expect("pause");
        assert_eq!(
            resumes(at(1)).await,
            Some(at(30)),
            "queued behind the paused credential"
        );
        assert_eq!(
            db.questions().list(analysis, at(1)).await.expect("list")[0].resumes_at,
            Some(at(30))
        );
        assert_eq!(resumes(at(31)).await, None, "the pause is over");

        // Its own job hits the usage limit.
        let claimed = db
            .jobs()
            .claim(ClaimParams {
                credential_ids: &[CREDENTIAL.to_owned()],
                job_token_hash: b"hash",
                lease_until: at(40),
                now: at(31),
            })
            .await
            .expect("claim")
            .expect("the ask job");
        db.jobs()
            .complete(
                &JobResult {
                    job_id: claimed.id,
                    attempt: claimed.attempt,
                    outcome: JobOutcome::UsageLimited {
                        resets_at: Some(at(90)),
                    },
                    session: None,
                    usage: None,
                    skills_commit: None,
                    llm_call: None,
                },
                at(32),
            )
            .await
            .expect("complete");
        assert_eq!(resumes(at(33)).await, Some(at(90)), "the paused job");
        db.questions()
            .update(id, &QuestionUpdate::Answering, at(33))
            .await
            .expect("answering");
        assert_eq!(resumes(at(33)).await, None, "only a pending question waits");
    }

    #[tokio::test]
    async fn provisional_answer_stays_open_and_out_of_context_until_the_result_is_applied() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let first = ask(&db, analysis, "f1.1", "first", at(0)).await;
        let later = ask(&db, analysis, "f1.1", "later", at(1)).await;
        let questions = db.questions();
        let open = || async {
            let records = questions.unanswered(at(5)).await.expect("open");
            records
                .iter()
                .map(|record| record.question.id)
                .collect::<Vec<_>>()
        };
        let earlier = || async {
            let step = StepId("f1.1".to_owned());
            let before = questions.answered_before(analysis, &step, later);
            before.await.expect("earlier")
        };
        questions
            .update(first, &QuestionUpdate::Provisional(output("Ready?")), at(2))
            .await
            .expect("provisional");
        let record = questions.get(first, at(2)).await.expect("get");
        assert_eq!(record.question.state, AskState::Answered, "shown at once");
        assert_eq!(record.answered_at, None, "not final");
        // Its job may have ended already (a restart between the job result and the question update).
        assert_eq!(open().await, [first, later]);
        assert!(
            earlier().await.is_empty(),
            "not context for later questions"
        );
        questions
            .update(first, &QuestionUpdate::Answered(output("Final?")), at(3))
            .await
            .expect("final");
        assert_eq!(open().await, [later]);
        assert_eq!(earlier().await, [("first".to_owned(), output("Final?"))]);
    }
}
