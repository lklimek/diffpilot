//! Read-only sums of retained terminal job results; no separate counters.

use contracts::time::Timestamp;

use super::jobs::Jobs;
use crate::error::DbError;
use contracts::api::usage::{UsageTotals, UsageWindows};
use contracts::ids::AnalysisId;
use sqlx::{Row, sqlite::SqliteRow};

fn totals(row: &SqliteRow) -> Result<Option<UsageTotals>, DbError> {
    let count = |key| -> Result<u64, DbError> {
        u64::try_from(row.try_get::<i64, _>(key)?).map_err(|_| DbError::Corrupt {
            column: "jobs.result.usage",
        })
    };
    let jobs = count("jobs")?;
    if jobs == 0 {
        return Ok(None);
    }
    Ok(Some(UsageTotals {
        input_tokens: count("input")?,
        cached_input_tokens: count("cached")?,
        output_tokens: count("output")?,
        reasoning_output_tokens: None,
        jobs,
    }))
}

impl Jobs<'_> {
    /// Payer-scoped UTC calendar totals and server-known active containers from one read transaction.
    pub async fn payer_telemetry(
        &self,
        user: &contracts::ids::UserId,
        now: Timestamp,
    ) -> Result<contracts::gateway::CredentialTelemetry, DbError> {
        let mut tx = super::read_tx(self.pool).await?;
        let rows = sqlx::query("WITH windows(label,since) AS (
                VALUES ('today',datetime(?2,'start of day')),('month',datetime(?2,'start of month'))),
            reported AS (
                SELECT j.id,j.finished_at AS at,json_extract(j.result,'$.usage.input_tokens') AS input,
                    json_extract(j.result,'$.usage.cached_input_tokens') AS cached,json_extract(j.result,'$.usage.output_tokens') AS output
                FROM jobs j WHERE j.payer_id=?1 AND j.finished_at IS NOT NULL AND json_type(j.result,'$.usage')='object'
                    AND NOT EXISTS(SELECT 1 FROM assessment_attempt_usage u JOIN assessment_method_runs r ON r.pass_id=u.pass_id AND r.method=u.method WHERE r.job_id=j.id AND u.attempt=j.attempt AND u.known=1)
                UNION ALL
                SELECT r.job_id,u.recorded_at,u.input_tokens,u.cached_input_tokens,u.output_tokens
                FROM assessment_attempt_usage u JOIN assessment_method_runs r ON r.pass_id=u.pass_id AND r.method=u.method WHERE u.payer_id=?1 AND u.known=1)
            SELECT w.label,COUNT(DISTINCT r.id) AS jobs,COALESCE(SUM(r.input),0) AS input,COALESCE(SUM(r.cached),0) AS cached,COALESCE(SUM(r.output),0) AS output
            FROM windows w LEFT JOIN reported r ON julianday(r.at)>=julianday(w.since) AND julianday(r.at)<=julianday(?2) GROUP BY w.label")
            .bind(&user.0).bind(now.to_rfc3339_offset()).fetch_all(&mut *tx).await?;
        let mut telemetry = contracts::gateway::CredentialTelemetry::default();
        for row in rows {
            match row.try_get::<String, _>("label")?.as_str() {
                "today" => telemetry.tokens_today = totals(&row)?,
                "month" => telemetry.tokens_month = totals(&row)?,
                _ => {
                    return Err(DbError::Corrupt {
                        column: "payer usage window",
                    });
                }
            }
        }
        let active: Vec<Option<i64>> = sqlx::query_scalar("SELECT DISTINCT pull_request_id FROM jobs WHERE payer_id=? AND state IN ('claimed','running') ORDER BY pull_request_id")
            .bind(&user.0).fetch_all(&mut *tx).await?;
        telemetry.live_containers = Some(
            active
                .into_iter()
                .map(|pr| match pr {
                    Some(pull_request_id) => contracts::runtime::ContainerId::Pr {
                        pull_request_id,
                        payer: user.clone(),
                    },
                    None => contracts::runtime::ContainerId::Judge {
                        payer: user.clone(),
                    },
                })
                .collect(),
        );
        tx.commit().await?;
        Ok(telemetry)
    }

    /// Model choices from immutable specs of claimed jobs, in first-use order, across all rounds.
    pub async fn models_used(
        &self,
        analysis: AnalysisId,
    ) -> Result<Vec<contracts::job::spec::ModelChoice>, DbError> {
        let rows: Vec<String> = sqlx::query_scalar("SELECT json_extract(spec, '$.model') AS choice FROM jobs WHERE analysis_id=? AND claimed_at IS NOT NULL GROUP BY choice ORDER BY MIN(claimed_at), MIN(rowid)")
            .bind(analysis.to_string()).fetch_all(self.pool).await?;
        rows.iter()
            .map(|s| serde_json::from_str(s).map_err(DbError::from))
            .collect()
    }

    /// Sums provider reports in finished jobs, including Ask, repair generations and all rounds.
    pub async fn usage(&self, analysis: AnalysisId) -> Result<Option<UsageTotals>, DbError> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS jobs,
            COALESCE(SUM(json_extract(result, '$.usage.input_tokens')), 0) AS input,
            COALESCE(SUM(json_extract(result, '$.usage.cached_input_tokens')), 0) AS cached,
            COALESCE(SUM(json_extract(result, '$.usage.output_tokens')), 0) AS output
            FROM jobs WHERE analysis_id = ? AND finished_at IS NOT NULL
              AND json_type(result, '$.usage') = 'object'",
        )
        .bind(analysis.to_string())
        .fetch_one(self.pool)
        .await?;
        totals(&row)
    }

    /// One aggregate query for tenant usage today, over seven days and in the current local calendar month.
    pub async fn usage_windows(
        &self,
        tenant: &str,
        now: Timestamp,
    ) -> Result<UsageWindows, DbError> {
        let rows = sqlx::query("WITH windows(label, since) AS (
                VALUES ('today', datetime(?2, 'localtime', 'start of day', 'utc')),
                       ('last_7_days', datetime(?2, 'localtime', 'start of day', '-6 days', 'utc')),
                       ('this_month', datetime(?2, 'localtime', 'start of month', 'utc'))),
            reported AS (SELECT j.id, j.result, j.finished_at FROM jobs j JOIN analyses a ON a.id = j.analysis_id
                WHERE a.tenant_id = ?1 AND j.finished_at IS NOT NULL AND json_type(j.result, '$.usage') = 'object')
            SELECT w.label, COUNT(r.id) AS jobs,
                COALESCE(SUM(json_extract(r.result, '$.usage.input_tokens')), 0) AS input,
                COALESCE(SUM(json_extract(r.result, '$.usage.cached_input_tokens')), 0) AS cached,
                COALESCE(SUM(json_extract(r.result, '$.usage.output_tokens')), 0) AS output
            FROM windows w LEFT JOIN reported r ON julianday(r.finished_at) >= julianday(w.since)
                AND julianday(r.finished_at) <= julianday(?2) GROUP BY w.label")
            .bind(tenant).bind(now.to_rfc3339_offset()).fetch_all(self.pool).await?;
        let mut usage = UsageWindows::default();
        for row in rows {
            match row.try_get::<String, _>("label")?.as_str() {
                "today" => usage.today = totals(&row)?,
                "last_7_days" => usage.last_7_days = totals(&row)?,
                "this_month" => usage.this_month = totals(&row)?,
                _ => {
                    return Err(DbError::Corrupt {
                        column: "usage window",
                    });
                }
            }
        }
        Ok(usage)
    }
}

/// Execution measurements from existing job timestamps, across all rounds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobStatistics {
    /// Submitted jobs, including jobs without a provider report.
    pub jobs: u64,
    /// Submitted repair generations.
    pub repair_runs: u64,
    /// Seconds spent in finished plan attempts.
    pub plan_seconds: u64,
    /// Seconds spent in finished PR-context attempts.
    pub pr_context_seconds: u64,
    /// Seconds spent in finished flow attempts.
    pub flow_seconds: u64,
    /// Seconds spent in finished Ask attempts.
    pub ask_seconds: u64,
}

impl Jobs<'_> {
    /// Aggregate execution times and counts; unfinished jobs contribute zero elapsed time.
    pub async fn statistics(&self, analysis: AnalysisId) -> Result<JobStatistics, DbError> {
        let row = sqlx::query("WITH measured AS (SELECT kind, generation,
            MAX(0, COALESCE((julianday(finished_at) - julianday(claimed_at)) * 86400, 0)) AS seconds
            FROM jobs WHERE analysis_id = ?)
            SELECT COUNT(*) AS jobs, COALESCE(SUM(generation > 0), 0) AS repairs,
              CAST(ROUND(COALESCE(SUM(CASE WHEN kind = 'plan' THEN seconds ELSE 0 END), 0)) AS INTEGER) AS plan,
              CAST(ROUND(COALESCE(SUM(CASE WHEN kind = 'pr_context' THEN seconds ELSE 0 END), 0)) AS INTEGER) AS context,
              CAST(ROUND(COALESCE(SUM(CASE WHEN kind = 'analyse_flow' THEN seconds ELSE 0 END), 0)) AS INTEGER) AS flow,
              CAST(ROUND(COALESCE(SUM(CASE WHEN kind = 'ask' THEN seconds ELSE 0 END), 0)) AS INTEGER) AS ask FROM measured")
            .bind(analysis.to_string()).fetch_one(self.pool).await?;
        let count = |key| -> Result<u64, DbError> {
            u64::try_from(row.try_get::<i64, _>(key)?).map_err(|_| DbError::Corrupt {
                column: "job statistics",
            })
        };
        Ok(JobStatistics {
            jobs: count("jobs")?,
            repair_runs: count("repairs")?,
            plan_seconds: count("plan")?,
            pr_context_seconds: count("context")?,
            flow_seconds: count("flow")?,
            ask_seconds: count("ask")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use jiff::SignedDuration;

    use super::*;
    use crate::repo::{
        Db,
        analyses::testing,
        jobs::{JobStage, NewJob},
    };
    use contracts::agent::JobKind;
    use contracts::job::spec::JobPriority;

    #[tokio::test]
    async fn models_used_groups_claimed_specs_across_rounds_and_ask() {
        use contracts::job::spec::{ModelChoice, ReasoningEffort};
        let db = Db::in_memory().await.unwrap();
        let id = testing::seed_analysis(&db).await.unwrap();
        assert!(db.jobs().models_used(id).await.unwrap().is_empty());
        let sol = ModelChoice {
            model: "gpt-6.1-sol".into(),
            reasoning_effort: ReasoningEffort::High,
        };
        let astra = ModelChoice {
            model: "gpt-6-astra".into(),
            reasoning_effort: ReasoningEffort::Low,
        };
        let now = Timestamp::now();
        for (i, kind, choice, claimed) in [
            (0, JobKind::Plan, sol.clone(), true),
            (1, JobKind::Ask, astra.clone(), true),
            (2, JobKind::Plan, sol.clone(), true),
            (
                3,
                JobKind::Plan,
                ModelChoice {
                    model: "gpt-6-sol".into(),
                    reasoning_effort: ReasoningEffort::Medium,
                },
                false,
            ),
        ] {
            let mut spec = testing::job_spec(id, kind);
            spec.model = choice;
            let stage = if kind == JobKind::Ask {
                JobStage::Ask(contracts::ids::QuestionId::new_random())
            } else {
                JobStage::Plan { batch: i }
            };
            let job = db
                .jobs()
                .enqueue(
                    NewJob {
                        spec,
                        stage,
                        generation: 0,
                        priority: JobPriority::Batch,
                    },
                    now,
                )
                .await
                .unwrap();
            sqlx::query("UPDATE jobs SET round=?, claimed_at=? WHERE id=?")
                .bind(i)
                .bind(
                    claimed.then(|| {
                        (now + SignedDuration::from_secs(i64::from(i))).to_rfc3339_offset()
                    }),
                )
                .bind(job.id.to_string())
                .execute(&db.pool)
                .await
                .unwrap();
        }
        assert_eq!(db.jobs().models_used(id).await.unwrap(), vec![sol, astra]);
    }

    #[tokio::test]
    async fn this_month_usage_window() {
        let db = Db::in_memory().await.unwrap();
        let id = testing::seed_analysis(&db).await.unwrap();
        let zone = jiff::tz::TimeZone::system();
        let local = |month, day, hour| {
            Timestamp::from(
                jiff::civil::date(2026, month, day)
                    .at(hour, 0, 0, 0)
                    .to_zoned(zone.clone())
                    .unwrap()
                    .timestamp(),
            )
        };
        let now = local(11, 15, 12);
        let month_start = local(11, 1, 0);
        for (index, finished_at, reported) in [
            (1, now, true),
            (2, local(11, 12, 12), true),
            (3, month_start, true),
            (4, month_start - SignedDuration::from_secs(1), true),
            (5, local(11, 16, 12), true),
            (6, now, false),
        ] {
            let job = db
                .jobs()
                .enqueue(
                    NewJob {
                        spec: testing::job_spec(id, JobKind::Plan),
                        stage: JobStage::Plan { batch: index },
                        generation: 0,
                        priority: JobPriority::Batch,
                    },
                    now,
                )
                .await
                .unwrap();
            let result = if reported {
                serde_json::json!({"usage": {"input_tokens": 100, "cached_input_tokens": 40, "output_tokens": 12}})
            } else {
                serde_json::json!({"usage": null})
            };
            sqlx::query(
                "UPDATE jobs SET state = 'succeeded', finished_at = ?, result = ? WHERE id = ?",
            )
            .bind(finished_at.to_rfc3339_offset())
            .bind(result.to_string())
            .bind(job.id.to_string())
            .execute(&db.pool)
            .await
            .unwrap();
        }
        let windows = db.jobs().usage_windows("default", now).await.unwrap();
        assert_eq!(windows.today.unwrap().jobs, 1);
        assert_eq!(windows.today.unwrap().input_tokens, 100);
        assert_eq!(windows.last_7_days.unwrap().jobs, 2);
        assert_eq!(windows.last_7_days.unwrap().output_tokens, 24);
        assert_eq!(
            windows.this_month,
            Some(UsageTotals {
                input_tokens: 300,
                cached_input_tokens: 120,
                output_tokens: 36,
                reasoning_output_tokens: None,
                jobs: 3,
            })
        );
        assert_eq!(
            db.jobs()
                .usage_windows("another-tenant", now)
                .await
                .unwrap(),
            UsageWindows::default()
        );
    }
    #[tokio::test]
    async fn statistics_count_rounds_repairs_and_finished_time_by_kind() {
        let db = Db::in_memory().await.unwrap();
        let id = testing::seed_analysis(&db).await.unwrap();
        for (stage, generation, round, seconds) in [
            (JobStage::Plan { batch: 1 }, 0, 0, Some(60)),
            (JobStage::Plan { batch: 1 }, 1, 0, Some(30)),
            (JobStage::PrContext, 0, 1, Some(20)),
            (
                JobStage::Flow(contracts::ids::FlowId("f1".into())),
                0,
                1,
                Some(40),
            ),
            (
                JobStage::Ask(contracts::ids::QuestionId::new_random()),
                0,
                1,
                Some(10),
            ),
            (JobStage::Plan { batch: 2 }, 0, 1, None),
        ] {
            let now = Timestamp::now();
            let job = db
                .jobs()
                .enqueue(
                    NewJob {
                        spec: testing::job_spec(id, stage.kind()),
                        stage,
                        generation,
                        priority: JobPriority::Batch,
                    },
                    now,
                )
                .await
                .unwrap();
            sqlx::query("UPDATE jobs SET round = ?, claimed_at = ?, finished_at = ? WHERE id = ?")
                .bind(round)
                .bind(now.to_rfc3339_offset())
                .bind(seconds.map(|s| (now + SignedDuration::from_secs(s)).to_rfc3339_offset()))
                .bind(job.id.to_string())
                .execute(&db.pool)
                .await
                .unwrap();
        }
        assert_eq!(
            db.jobs().statistics(id).await.unwrap(),
            JobStatistics {
                jobs: 6,
                repair_runs: 1,
                plan_seconds: 90,
                pr_context_seconds: 20,
                flow_seconds: 40,
                ask_seconds: 10,
            }
        );
    }
}
