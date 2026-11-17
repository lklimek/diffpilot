//! Read-only waiting progress from jobs and accepted documents; no new stored state or schema.

use std::collections::BTreeMap;

use contracts::agent::JobKind;
use contracts::api::analyses::{
    AnalysisEstimate, AnalysisProgress, AnalysisState, AnalysisSummary, EstimateBasis, PlanPart,
    RunStage, RunStageState, StageKind,
};
use sqlx::Row;

use super::analyses::Analyses;
use super::jobs::{parse_ts, parse_wire};
use crate::error::DbError;

/// Largest estimate shown; sparse or implausible history yields no estimate instead of an absurd number.
const MAX_ESTIMATE: f64 = 7.0 * 24.0 * 3600.0;
const BATCH_PREFIX: &str = "This pull request is too large for one plan job: this is batch ";

/// Reads the server-written batch header, ignoring every quoted data block (never LLM text).
fn plan_part(prompt: &str) -> Option<PlanPart> {
    let mut end = None;
    for line in prompt.lines() {
        if let Some(marker) = &end {
            if line == marker {
                end = None;
            }
            continue;
        }
        if let Some(data) = line.strip_prefix("<<<DATA ") {
            if let Some(id) = data.split_whitespace().next() {
                end = Some(format!("<<<END {id}>>>"));
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix(BATCH_PREFIX) {
            let (index, rest) = rest.split_once(" of ")?;
            let (total, _) = rest.split_once(',')?;
            let (index, total) = (index.parse::<u32>().ok()?, total.parse::<u32>().ok()?);
            return (index > 0 && index <= total && total > 1).then_some(PlanPart { index, total });
        }
    }
    None
}

#[derive(Clone, Debug)]
struct Timing {
    kind: JobKind,
    seconds: f64,
    count: u32,
    lines: u64,
}

fn mean(samples: &[Timing], kind: JobKind) -> Option<f64> {
    let matching: Vec<_> = samples.iter().filter(|s| s.kind == kind).collect();
    let count: u32 = matching.iter().map(|s| s.count).sum();
    (count > 0).then(|| matching.iter().map(|s| s.seconds).sum::<f64>() / f64::from(count))
}

fn per_line(samples: &[Timing], kind: JobKind) -> Option<f64> {
    let matching: Vec<_> = samples
        .iter()
        .filter(|s| s.kind == kind && s.lines > 0)
        .collect();
    let lines: u64 = matching.iter().map(|s| s.lines).sum();
    (lines > 0).then(|| matching.iter().map(|s| s.seconds).sum::<f64>() / lines as f64)
}

fn estimate(
    progress: &AnalysisProgress,
    current: &[Timing],
    history: &[Timing],
    lines: u64,
) -> Option<AnalysisEstimate> {
    let parts_left = progress
        .planning
        .total?
        .saturating_sub(progress.planning.done);
    let mut seconds = 0.0;
    let mut local = false;
    if parts_left > 0 {
        if let Some(part) = mean(current, JobKind::Plan) {
            seconds += f64::from(parts_left) * part;
            local = true;
        } else {
            // A completed historical run contributes total plan time / its changed agent lines.
            let rate = per_line(history, JobKind::Plan)?;
            if lines == 0 {
                return None;
            }
            seconds +=
                rate * lines as f64 * f64::from(parts_left) / f64::from(progress.planning.total?);
        }
    }
    if progress.pr_context.done == 0 {
        seconds += mean(history, JobKind::PrContext)?;
    }
    let rough = progress.flows.total.is_none();
    match progress.flows.total {
        Some(total) => {
            let left = total.saturating_sub(progress.flows.done);
            if left > 0 {
                let each = if let Some(each) = mean(current, JobKind::AnalyseFlow) {
                    local = true;
                    each
                } else {
                    mean(history, JobKind::AnalyseFlow)?
                };
                seconds += f64::from(left) * each;
            }
        }
        None => {
            if lines == 0 {
                return None;
            }
            seconds += per_line(history, JobKind::AnalyseFlow)? * lines as f64;
        }
    }
    if !seconds.is_finite() || !(0.0..=MAX_ESTIMATE).contains(&seconds) {
        return None;
    }
    Some(AnalysisEstimate {
        remaining_seconds: seconds.ceil() as u32,
        basis: if local {
            EstimateBasis::ThisAnalysis
        } else if rough {
            EstimateBasis::LinesOnly
        } else {
            EstimateBasis::RecentRuns
        },
        rough,
    })
}

fn stage(done: u32, total: Option<u32>, current: bool) -> RunStage {
    let state = if total == Some(done) {
        RunStageState::Done
    } else if current {
        RunStageState::Current
    } else {
        RunStageState::Pending
    };
    RunStage { done, total, state }
}

impl Analyses<'_> {
    /// A rough pre-start estimate from the same history and formula as waiting progress.
    pub async fn estimate_for_size(
        &self,
        tenant: &str,
        lines: u64,
    ) -> Result<Option<AnalysisEstimate>, DbError> {
        let history = self.recent_timings(tenant, None).await?;
        let progress = AnalysisProgress {
            started_at: chrono::Utc::now(),
            planning: stage(0, Some(1), true),
            pr_context: stage(0, Some(1), false),
            flows: stage(0, None, false),
            finishing: stage(0, Some(1), false),
            estimate: None,
            percent: None,
        };
        Ok(estimate(&progress, &[], &history, lines))
    }

    async fn recent_timings(
        &self,
        tenant: &str,
        excluding: Option<&str>,
    ) -> Result<Vec<Timing>, DbError> {
        let history = sqlx::query(
            "WITH recent AS (SELECT id FROM analyses WHERE state = 'finished'
               AND tenant_id = ?1 AND (?2 IS NULL OR id <> ?2)
               ORDER BY finished_at DESC LIMIT 30)
             SELECT j.kind, SUM((julianday(j.finished_at) - julianday(j.claimed_at)) * 86400) AS seconds,
                    COUNT(DISTINCT j.stage) AS count,
                    (SELECT body FROM documents WHERE analysis_id = a.id AND kind = 'prepass' AND key = '') AS prepass
             FROM recent r JOIN analyses a ON a.id = r.id JOIN jobs j ON j.analysis_id = a.id AND j.round = a.round
             WHERE j.state = 'succeeded' AND j.kind <> 'ask'
               AND j.finished_at > j.claimed_at
               AND (julianday(j.finished_at) - julianday(j.claimed_at)) * 86400 BETWEEN 1 AND 10800
             GROUP BY a.id, j.kind")
            .bind(tenant).bind(excluding).fetch_all(self.pool).await?;
        history
            .iter()
            .map(|row| {
                Ok(Timing {
                    kind: parse_wire(&row.try_get::<String, _>("kind")?, "jobs.kind")?,
                    seconds: row.try_get("seconds")?,
                    count: row.try_get("count")?,
                    lines: changed_lines(row.try_get::<Option<String>, _>("prepass")?.as_deref())?,
                })
            })
            .collect::<Result<_, DbError>>()
    }

    /// Adds waiting facts from the current round and recent successful runs in the same tenant.
    pub(crate) async fn add_progress(&self, summary: &mut AnalysisSummary) -> Result<(), DbError> {
        let id = summary.id.to_string();
        let mut tx = super::read_tx(self.pool).await?;
        let meta = sqlx::query(
            "SELECT a.round, a.tenant_id,
             (SELECT json_extract(spec, '$.prompt') FROM jobs WHERE analysis_id = a.id AND kind = 'plan'
               AND generation = 0 ORDER BY round, created_at, rowid LIMIT 1) AS plan_prompt,
             (SELECT body FROM documents WHERE analysis_id = a.id AND kind = 'prepass' AND key = '') AS prepass,
             (SELECT json_array_length(body, '$.flows') FROM documents WHERE analysis_id = a.id AND kind = 'plan' AND key = '') AS flows,
             (SELECT COUNT(*) FROM documents WHERE analysis_id = a.id AND kind = 'plan' AND key <> '') AS plan_done,
             EXISTS(SELECT 1 FROM documents WHERE analysis_id = a.id AND kind = 'pr_context') AS context_done,
             (SELECT COUNT(*) FROM documents WHERE analysis_id = a.id AND kind = 'flow') AS stored_flow_done
             FROM analyses a WHERE a.id = ?")
            .bind(&id).fetch_one(&mut *tx).await?;
        let rows = sqlx::query(
            "SELECT j.kind, j.stage, j.state, j.created_at, j.claimed_at, j.finished_at,
                    EXISTS(SELECT 1 FROM documents d WHERE d.analysis_id = j.analysis_id AND
                       ((j.kind = 'plan' AND d.kind = 'plan' AND (d.key = substr(j.stage, 6) OR d.key = ''))
                        OR (j.kind = 'pr_context' AND d.kind = 'pr_context')
                        OR (j.kind = 'analyse_flow' AND d.kind = 'flow' AND d.key = substr(j.stage, 6)))) AS accepted
             FROM jobs j JOIN analyses a ON a.id = j.analysis_id AND a.round = j.round
             WHERE a.id = ? AND j.kind <> 'ask' ORDER BY j.created_at, j.rowid")
            .bind(&id).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        let lines = changed_lines(meta.try_get::<Option<String>, _>("prepass")?.as_deref())?;
        let history = self
            .recent_timings(&meta.try_get::<String, _>("tenant_id")?, Some(&id))
            .await?;
        let mut completed: BTreeMap<String, Timing> = BTreeMap::new();
        let prompt: Option<String> = meta.try_get("plan_prompt")?;
        let mut total = prompt
            .as_deref()
            .map(|text| plan_part(text).map_or(1, |part| part.total));
        let mut active_part = None;
        let mut started_at = summary.created_at;
        for (index, row) in rows.iter().enumerate() {
            if index == 0
                && meta.try_get::<u32, _>("round")? > 0
                && summary.review_ready != Some(true)
            {
                started_at = parse_ts(&row.try_get::<String, _>("created_at")?, "jobs.created_at")?;
            }
            let kind: JobKind = parse_wire(&row.try_get::<String, _>("kind")?, "jobs.kind")?;
            let key: String = row.try_get("stage")?;
            let state: String = row.try_get("state")?;
            if kind == JobKind::Plan
                && matches!(state.as_str(), "queued" | "claimed" | "running" | "paused")
            {
                active_part = key
                    .strip_prefix("plan:")
                    .and_then(|value| value.parse::<u32>().ok());
            }
            if state == "succeeded"
                && row.try_get::<bool, _>("accepted")?
                && let (Some(start), Some(end)) = (
                    row.try_get::<Option<String>, _>("claimed_at")?,
                    row.try_get::<Option<String>, _>("finished_at")?,
                )
            {
                let seconds = (parse_ts(&end, "jobs.finished_at")?
                    - parse_ts(&start, "jobs.claimed_at")?)
                .num_seconds();
                if (1..=10800).contains(&seconds) {
                    let timing = completed.entry(key).or_insert(Timing {
                        kind,
                        seconds: 0.0,
                        count: 1,
                        lines,
                    });
                    timing.seconds += seconds as f64;
                }
            }
        }
        let current: Vec<Timing> = completed.into_values().collect();
        let flow_total: Option<u32> = meta.try_get("flows")?;
        let plan_done = if flow_total.is_some() {
            total.unwrap_or(
                meta.try_get::<u32, _>("plan_done")?
                    .max(u32::from(flow_total.is_some_and(|n| n > 0))),
            )
        } else {
            meta.try_get("plan_done")?
        };
        if flow_total.is_some() && total.is_none() {
            total = Some(plan_done);
        }
        let context_done = u32::from(meta.try_get::<bool, _>("context_done")?);
        let (flow_done, flow_total) = match &summary.state {
            AnalysisState::Running { done, total, .. } => (*done, Some(*total)),
            AnalysisState::Paused { done, total, .. } => (*done, *total),
            AnalysisState::Finished { total } => (*total, Some(*total)),
            _ => (meta.try_get("stored_flow_done")?, flow_total),
        };
        if let AnalysisState::Planning { stage }
        | AnalysisState::Running { stage, .. }
        | AnalysisState::Paused { stage, .. } = &mut summary.state
            && let Some(stage) = stage
            && stage.kind == StageKind::Plan
        {
            stage.part = active_part
                .zip(total)
                .filter(|(i, n)| *n > 1 && i <= n)
                .map(|(index, total)| PlanPart { index, total });
        }
        let finished = matches!(summary.state, AnalysisState::Finished { .. });
        let planning_done = flow_total.is_some();
        let finishing = planning_done && context_done == 1 && flow_total == Some(flow_done);
        let mut progress = AnalysisProgress {
            started_at,
            planning: stage(plan_done, total, !planning_done),
            pr_context: stage(context_done, Some(1), planning_done && context_done == 0),
            flows: stage(
                flow_done,
                flow_total,
                planning_done && context_done == 1 && !finishing,
            ),
            finishing: stage(u32::from(finished), Some(1), finishing && !finished),
            estimate: None,
            percent: None,
        };
        if matches!(
            summary.state,
            AnalysisState::Planning { .. } | AnalysisState::Running { .. }
        ) {
            progress.estimate = estimate(&progress, &current, &history, lines);
        }
        progress.percent = if finished {
            Some(100)
        } else if let (Some(parts), Some(flows)) = (total, flow_total) {
            let all = u64::from(parts) + u64::from(flows) + 2;
            Some(
                ((u64::from(plan_done) + u64::from(context_done) + u64::from(flow_done)) * 100
                    / all)
                    .min(99) as u8,
            )
        } else if let Some(estimate) = progress.estimate {
            let done: f64 = current.iter().map(|sample| sample.seconds).sum();
            let all = done + f64::from(estimate.remaining_seconds);
            (all > 0.0).then(|| (100.0 * done / all).clamp(0.0, 99.0) as u8)
        } else {
            None
        };
        summary.progress = Some(Box::new(progress));
        Ok(())
    }
}

fn changed_lines(prepass: Option<&str>) -> Result<u64, DbError> {
    let Some(prepass) = prepass else {
        return Ok(0);
    };
    let value: serde_json::Value = serde_json::from_str(prepass)?;
    Ok(value
        .get("agent_lines")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .flat_map(|file| {
            file.get("ranges")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
        })
        .filter_map(|range| {
            Some(
                range
                    .get("end_line")?
                    .as_u64()?
                    .saturating_sub(range.get("start_line")?.as_u64()?)
                    + 1,
            )
        })
        .sum())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{
        Db,
        analyses::testing,
        documents::DocumentKind,
        jobs::{JobStage, NewJob},
    };
    use chrono::{TimeZone, Utc};
    use contracts::job::spec::JobPriority;

    fn progress() -> AnalysisProgress {
        AnalysisProgress {
            started_at: Utc.with_ymd_and_hms(2026, 11, 17, 10, 0, 0).unwrap(),
            planning: stage(5, Some(23), true),
            pr_context: stage(0, Some(1), false),
            flows: stage(0, None, false),
            finishing: stage(0, Some(1), false),
            estimate: None,
            percent: None,
        }
    }
    fn history() -> Vec<Timing> {
        vec![
            Timing {
                kind: JobKind::Plan,
                seconds: 600.0,
                count: 2,
                lines: 1000,
            },
            Timing {
                kind: JobKind::PrContext,
                seconds: 120.0,
                count: 1,
                lines: 1000,
            },
            Timing {
                kind: JobKind::AnalyseFlow,
                seconds: 1800.0,
                count: 6,
                lines: 1000,
            },
        ]
    }

    #[test]
    fn estimates_need_each_remaining_kind_and_use_measured_parts() {
        let current = vec![Timing {
            kind: JobKind::Plan,
            seconds: 300.0,
            count: 1,
            lines: 1000,
        }];
        assert_eq!(estimate(&progress(), &current, &[], 1000), None);
        let e = estimate(&progress(), &current, &history(), 1000).unwrap();
        assert_eq!(e.remaining_seconds, 18 * 300 + 120 + 1800);
        assert_eq!(e.basis, EstimateBasis::ThisAnalysis);
        assert!(e.rough);
        let e = estimate(&progress(), &[], &history(), 1000).unwrap();
        assert_eq!(e.basis, EstimateBasis::LinesOnly);
        assert!(e.rough);
        let mut p = progress();
        p.planning = stage(23, Some(23), false);
        p.flows = stage(2, Some(6), true);
        let e = estimate(&p, &[], &history(), 1000).unwrap();
        assert_eq!(e.remaining_seconds, 120 + 4 * 300);
        assert_eq!(e.basis, EstimateBasis::RecentRuns);
        assert!(!e.rough);
        p.flows.total = Some(u32::MAX);
        assert_eq!(estimate(&p, &[], &history(), 1000), None);
    }

    #[test]
    fn batch_header_ignores_quoted_data_and_invalid_counts() {
        let header = include_str!("../../../../prompts/parts.md")
            .replace("{{number}}", "6")
            .replace("{{total}}", "23");
        assert_eq!(
            plan_part(&header),
            Some(PlanPart {
                index: 6,
                total: 23
            })
        );
        assert_eq!(
            plan_part(&format!("<<<DATA abc profile>>>\n{header}\n<<<END abc>>>")),
            None
        );
        for pair in ["0 of 23", "24 of 23", "1 of 0", "1 of 1", "-1 of 23"] {
            assert_eq!(plan_part(&format!("{BATCH_PREFIX}{pair}, listing")), None);
        }
    }

    #[tokio::test]
    async fn parts_survive_reads_and_repairs_never_inflate_completed_parts() {
        let db = Db::in_memory().await.unwrap();
        let id = testing::seed_analysis(&db).await.unwrap();
        for (batch, generation) in [(1, 0), (1, 1), (2, 0)] {
            let mut spec = testing::job_spec(id, JobKind::Plan);
            spec.prompt = format!("{BATCH_PREFIX}{batch} of 23, listing only some directories.");
            let job = db
                .jobs()
                .enqueue(
                    NewJob {
                        spec,
                        stage: JobStage::Plan { batch },
                        generation,
                        priority: JobPriority::Batch,
                    },
                    Utc::now(),
                )
                .await
                .unwrap();
            if batch == 1 {
                sqlx::query("UPDATE jobs SET state = 'succeeded', claimed_at = '2026-11-17T10:00:00Z', finished_at = '2026-11-17T10:05:00Z' WHERE id = ?").bind(job.id.to_string()).execute(&db.pool).await.unwrap();
            }
        }
        db.documents()
            .put(
                id,
                DocumentKind::Plan,
                "1",
                &serde_json::json!({"flows": []}),
                Utc::now(),
            )
            .await
            .unwrap();
        let a = db.analyses().summary(id).await.unwrap();
        let AnalysisState::Planning { stage: Some(stage) } = &a.state else {
            panic!("stage");
        };
        assert_eq!(
            stage.part,
            Some(PlanPart {
                index: 2,
                total: 23
            })
        );
        let p = a.progress.as_ref().unwrap();
        assert_eq!(p.planning.done, 1);
        assert_eq!(p.planning.total, Some(23));
        assert_eq!(p.flows.total, None);
        assert_eq!(p.estimate, None);
        assert_eq!(p.started_at, a.created_at);
        assert_eq!(
            db.analyses().summary(id).await.unwrap(),
            a,
            "reconnect snapshot is stable"
        );
        db.analyses()
            .set_state(
                id,
                &AnalysisState::Failed {
                    reason: None,
                    flow: None,
                    message: "interrupted".into(),
                },
                Utc::now(),
            )
            .await
            .unwrap();
        assert!(db.analyses().retry(id, Utc::now()).await.unwrap());
        let retried = db.analyses().summary(id).await.unwrap().progress.unwrap();
        assert_eq!(
            retried.planning.total,
            Some(23),
            "retry reuses the original fixed count"
        );
        assert_eq!(retried.planning.done, 1);
    }

    #[tokio::test]
    async fn historical_jobs_supply_a_rough_line_estimate_and_pause_hides_it() {
        let db = Db::in_memory().await.unwrap();
        let old = testing::seed_analysis(&db).await.unwrap();
        let prepass =
            serde_json::json!({"agent_lines": [{"ranges": [{"start_line": 1, "end_line": 1000}]}]});
        db.documents()
            .put(old, DocumentKind::Prepass, "", &prepass, Utc::now())
            .await
            .unwrap();
        for (stage, end) in [
            (JobStage::Plan { batch: 1 }, "2026-11-17T10:10:00Z"),
            (JobStage::PrContext, "2026-11-17T10:02:00Z"),
            (
                JobStage::Flow(contracts::ids::FlowId("f1".into())),
                "2026-11-17T10:30:00Z",
            ),
        ] {
            let job = db
                .jobs()
                .enqueue(
                    NewJob {
                        spec: testing::job_spec(old, stage.kind()),
                        stage,
                        generation: 0,
                        priority: JobPriority::Batch,
                    },
                    Utc::now(),
                )
                .await
                .unwrap();
            sqlx::query("UPDATE jobs SET state = 'succeeded', claimed_at = '2026-11-17T10:00:00Z', finished_at = ? WHERE id = ?")
                .bind(end).bind(job.id.to_string()).execute(&db.pool).await.unwrap();
        }
        db.analyses()
            .set_state(old, &AnalysisState::Finished { total: 1 }, Utc::now())
            .await
            .unwrap();
        let id = testing::seed_analysis(&db).await.unwrap();
        db.documents()
            .put(id, DocumentKind::Prepass, "", &prepass, Utc::now())
            .await
            .unwrap();
        let mut spec = testing::job_spec(id, JobKind::Plan);
        spec.prompt = format!("{BATCH_PREFIX}1 of 23, listing only some directories.");
        db.jobs()
            .enqueue(
                NewJob {
                    spec,
                    stage: JobStage::Plan { batch: 1 },
                    generation: 0,
                    priority: JobPriority::Batch,
                },
                Utc::now(),
            )
            .await
            .unwrap();
        let progress = db.analyses().summary(id).await.unwrap().progress.unwrap();
        let eta = progress.estimate.unwrap();
        assert_eq!(eta.basis, EstimateBasis::LinesOnly);
        assert!(eta.rough);
        assert!(
            (2520..=2521).contains(&eta.remaining_seconds),
            "SQLite floating-point time rounding: {eta:?}"
        );
        assert_eq!(progress.percent, Some(0));
        assert_eq!(
            db.analyses()
                .estimate_for_size("default", 1000)
                .await
                .unwrap(),
            Some(eta)
        );
        assert_eq!(
            db.analyses()
                .estimate_for_size("other-tenant", 1000)
                .await
                .unwrap(),
            None
        );
        // A successful agent job is not a finished part until its output is accepted.
        sqlx::query("UPDATE jobs SET state = 'succeeded', claimed_at = '2026-11-17T10:00:00Z', finished_at = '2026-11-17T10:05:00Z' WHERE analysis_id = ?")
            .bind(id.to_string()).execute(&db.pool).await.unwrap();
        assert_eq!(
            db.analyses()
                .summary(id)
                .await
                .unwrap()
                .progress
                .unwrap()
                .estimate
                .unwrap()
                .basis,
            EstimateBasis::LinesOnly
        );
        let repair = db
            .jobs()
            .enqueue(
                NewJob {
                    spec: testing::job_spec(id, JobKind::Plan),
                    stage: JobStage::Plan { batch: 1 },
                    generation: 1,
                    priority: JobPriority::Batch,
                },
                Utc::now(),
            )
            .await
            .unwrap();
        sqlx::query("UPDATE jobs SET state = 'succeeded', claimed_at = '2026-11-17T10:05:00Z', finished_at = '2026-11-17T10:06:40Z' WHERE id = ?")
            .bind(repair.id.to_string()).execute(&db.pool).await.unwrap();
        db.documents()
            .put(
                id,
                DocumentKind::Plan,
                "1",
                &serde_json::json!({"flows": []}),
                Utc::now(),
            )
            .await
            .unwrap();
        let repaired = db
            .analyses()
            .summary(id)
            .await
            .unwrap()
            .progress
            .unwrap()
            .estimate
            .unwrap();
        assert_eq!(repaired.basis, EstimateBasis::ThisAnalysis);
        assert!(
            (10720..=10721).contains(&repaired.remaining_seconds),
            "accepted part includes 300s initial + 100s repair: {repaired:?}"
        );
        db.analyses()
            .set_state(
                id,
                &AnalysisState::Paused {
                    reason: Some(contracts::api::analyses::PauseReason::UsageLimit),
                    paused_by: None,
                    paused_at: None,
                    done: 0,
                    total: None,
                    resumes_at: Some(Utc::now()),
                    stage: None,
                },
                Utc::now(),
            )
            .await
            .unwrap();
        assert_eq!(
            db.analyses()
                .summary(id)
                .await
                .unwrap()
                .progress
                .unwrap()
                .estimate,
            None
        );
    }
    #[tokio::test]
    async fn failed_flow_counts_accepted_documents_not_plan_position() {
        let db = Db::in_memory().await.unwrap();
        let id = testing::seed_analysis(&db).await.unwrap();
        db.documents()
            .put(
                id,
                DocumentKind::Plan,
                "",
                &serde_json::json!({"flows": [{"id": "f1"}, {"id": "f2"}]}),
                Utc::now(),
            )
            .await
            .unwrap();
        db.documents()
            .put(
                id,
                DocumentKind::PrContext,
                "",
                &serde_json::json!({}),
                Utc::now(),
            )
            .await
            .unwrap();
        // f2 was requested first: a failure there cannot imply that f1 was analysed.
        db.analyses()
            .set_state(
                id,
                &AnalysisState::Failed {
                    reason: None,
                    flow: Some(contracts::api::analyses::FlowRef {
                        id: contracts::ids::FlowId("f2".into()),
                        title: "second".into(),
                    }),
                    message: "interrupted".into(),
                },
                Utc::now(),
            )
            .await
            .unwrap();
        let progress = db.analyses().summary(id).await.unwrap().progress.unwrap();
        assert_eq!(progress.flows, stage(0, Some(2), true));
        assert_eq!(progress.estimate, None);
    }
    #[tokio::test]
    async fn on_demand_and_failed_flows_have_no_whole_run_eta() {
        let db = Db::in_memory().await.unwrap();
        let id = testing::seed_analysis(&db).await.unwrap();
        for (kind, key, body) in [
            (
                DocumentKind::Plan,
                "",
                serde_json::json!({"flows":[{"id":"f1"},{"id":"f2"}]}),
            ),
            (DocumentKind::PrContext, "", serde_json::json!({})),
            (DocumentKind::Flow, "f1", serde_json::json!({})),
        ] {
            db.documents()
                .put(id, kind, key, &body, Utc::now())
                .await
                .unwrap();
        }
        let job = db
            .jobs()
            .enqueue(
                NewJob {
                    spec: testing::job_spec(id, JobKind::AnalyseFlow),
                    stage: JobStage::Flow(contracts::ids::FlowId("f1".into())),
                    generation: 0,
                    priority: JobPriority::Batch,
                },
                Utc::now(),
            )
            .await
            .unwrap();
        sqlx::query("UPDATE jobs SET state='succeeded', claimed_at='2026-11-17T10:00:00Z', finished_at='2026-11-17T10:01:00Z' WHERE id=?")
            .bind(job.id.to_string()).execute(&db.pool).await.unwrap();
        db.analyses()
            .set_state(
                id,
                &AnalysisState::Running {
                    done: 1,
                    total: 2,
                    stage: None,
                },
                Utc::now(),
            )
            .await
            .unwrap();
        assert!(
            db.analyses()
                .summary(id)
                .await
                .unwrap()
                .progress
                .unwrap()
                .estimate
                .is_some()
        );
        db.flow_work()
            .set_background(id, false, Utc::now())
            .await
            .unwrap();
        assert!(
            db.analyses()
                .summary(id)
                .await
                .unwrap()
                .progress
                .unwrap()
                .estimate
                .is_none()
        );
        db.flow_work()
            .set_background(id, true, Utc::now())
            .await
            .unwrap();
        db.flow_work()
            .fail(id, &contracts::ids::FlowId("f2".into()), "Retry needed")
            .await
            .unwrap();
        assert!(
            db.analyses()
                .summary(id)
                .await
                .unwrap()
                .progress
                .unwrap()
                .estimate
                .is_none()
        );
        let created = db.analyses().summary(id).await.unwrap().created_at;
        sqlx::query("UPDATE analyses SET round=round+1 WHERE id=?")
            .bind(id.to_string())
            .execute(&db.pool)
            .await
            .unwrap();
        db.jobs()
            .enqueue(
                NewJob {
                    spec: testing::job_spec(id, JobKind::AnalyseFlow),
                    stage: JobStage::Flow(contracts::ids::FlowId("f2".into())),
                    generation: 0,
                    priority: JobPriority::Batch,
                },
                created + chrono::TimeDelta::hours(1),
            )
            .await
            .unwrap();
        assert_eq!(
            db.analyses()
                .summary(id)
                .await
                .unwrap()
                .progress
                .unwrap()
                .started_at,
            created,
            "a flow retry must not reset the elapsed analysis time"
        );
    }
}
