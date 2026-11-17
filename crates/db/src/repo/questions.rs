//! Repository for `questions`: Ask questions on steps of an analysis and their LLM answers.

use chrono::{DateTime, Utc};
use contracts::agent::ask::AskOutput;
use contracts::api::questions::{AskQuestion, AskState};
use contracts::ids::{AnalysisId, JobId, QuestionId, StepId};
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
    /// The step asked about (canonical Tour id).
    pub step_id: StepId,
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
    pub user_window_start: DateTime<Utc>,
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
    pub answered_at: Option<DateTime<Utc>>,
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
        "SELECT q.id, q.analysis_id, q.step_id, q.user_id, q.text, q.state, q.answer, q.failure, q.job_id,
             q.created_at, q.answered_at,
             CASE WHEN q.state = 'pending' THEN
                 CASE WHEN j.state = 'paused' AND j.resumes_at > ?1 THEN j.resumes_at
                      WHEN j.state = 'queued' AND c.paused_until > ?1 THEN c.paused_until END
             END AS resumes_at
         FROM questions q
         LEFT JOIN jobs j ON j.id = q.job_id
         LEFT JOIN llm_credentials c ON c.id = j.credential_id"
    };
}

fn optional_ts(row: &SqliteRow, column: &'static str) -> Result<Option<DateTime<Utc>>, DbError> {
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
    Ok(QuestionRecord {
        question: AskQuestion {
            id: parse_wire(&id, "questions.id")?,
            step_id: StepId(row.try_get("step_id")?),
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
    })
}

impl Questions<'_> {
    /// Stores `question` as `pending` unless a limit is reached; `NotFound` for an unknown analysis.
    pub async fn create(
        &self,
        question: NewQuestion,
        limits: QuestionLimits,
        now: DateTime<Utc>,
    ) -> Result<Asked, DbError> {
        let inserted = sqlx::query(
            "INSERT INTO questions (id, tenant_id, analysis_id, step_id, user_id, text, state, created_at)
             SELECT ?1, a.tenant_id, a.id, ?2, ?3, ?4, 'pending', ?5 FROM analyses a WHERE a.id = ?6
                 AND (SELECT COUNT(*) FROM questions WHERE analysis_id = a.id) < ?7
                 AND (SELECT COUNT(*) FROM questions WHERE user_id = ?3 AND created_at >= ?8) < ?9",
        )
        .bind(question.id.to_string())
        .bind(&question.step_id.0)
        .bind(&question.user_id)
        .bind(&question.text)
        .bind(ts(now))
        .bind(question.analysis_id.to_string())
        .bind(limits.per_analysis)
        .bind(ts(limits.user_window_start))
        .bind(limits.per_user)
        .execute(self.pool)
        .await?;
        if inserted.rows_affected() == 1 {
            return Ok(Asked::Stored(Box::new(self.get(question.id, now).await?)));
        }
        let held: Option<u32> = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM questions q WHERE q.analysis_id = a.id) FROM analyses a WHERE a.id = ?",
        )
        .bind(question.analysis_id.to_string())
        .fetch_optional(self.pool)
        .await?;
        match held {
            None => Err(DbError::NotFound),
            Some(held) if held >= limits.per_analysis => Ok(Asked::AnalysisLimit),
            Some(_) => Ok(Asked::UserLimit),
        }
    }

    /// One question; `resumes_at` as of `now`.
    pub async fn get(&self, id: QuestionId, now: DateTime<Utc>) -> Result<QuestionRecord, DbError> {
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
        now: DateTime<Utc>,
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

    /// Questions of every tenant whose end is not stored yet, oldest first: `pending`, `answering`, or answered
    /// provisionally (the job's result, possibly already recorded, decides).
    pub async fn unanswered(&self, now: DateTime<Utc>) -> Result<Vec<QuestionRecord>, DbError> {
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
             WHERE asked.id = ?3 AND q.analysis_id = ?1 AND q.step_id = ?2 AND q.state = 'answered'
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

    /// Stores a state change; an answer replaces the failure and vice versa.
    pub async fn update(
        &self,
        id: QuestionId,
        update: &QuestionUpdate,
        now: DateTime<Utc>,
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
    use chrono::{TimeDelta, TimeZone};
    use contracts::agent::JobKind;
    use contracts::common::{Evidence, LineSide};
    use contracts::job::result::{JobOutcome, JobResult};
    use contracts::job::spec::{CredentialKind, JobPriority, LlmCredentialRef};

    use super::*;
    use crate::repo::DEFAULT_TENANT;
    use crate::repo::analyses::testing::{CREDENTIAL, job_spec, seed_analysis};
    use crate::repo::jobs::{ClaimParams, JobStage, NewJob};

    fn at(minute: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 17, 12, 0, 0)
            .single()
            .unwrap_or_default()
            + TimeDelta::minutes(minute)
    }

    const LIMITS: QuestionLimits = QuestionLimits {
        per_analysis: 3,
        per_user: 2,
        user_window_start: DateTime::UNIX_EPOCH,
    };

    fn new_question(analysis: AnalysisId, step: &str, text: &str) -> NewQuestion {
        NewQuestion {
            id: QuestionId::new_random(),
            analysis_id: analysis,
            step_id: StepId(step.to_owned()),
            user_id: "u1".to_owned(),
            text: text.to_owned(),
        }
    }

    async fn ask(
        db: &Db,
        analysis: AnalysisId,
        step: &str,
        text: &str,
        now: DateTime<Utc>,
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
    async fn question_is_stored_pending_per_tenant_and_text_kept_as_given() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let text = "<b>Is</b> this\nsent twice?";
        let id = ask(&db, analysis, "f1.2", text, at(0)).await;
        let record = db.questions().get(id, at(0)).await.expect("get");
        assert_eq!(record.question.state, AskState::Pending);
        assert_eq!(record.question.text, text);
        assert_eq!(record.question.step_id, StepId("f1.2".to_owned()));
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
