//! A refresh of the dashboard world ranked by the judge ranker end to end: scan, `judge_work` jobs answered by the
//! test in the runner's place (neutral answers, as the runner's fake provider), stored judgements, ranked items.

use std::sync::Mutex;

use contracts::agent::judge_work::{JudgeItem, JudgeWorkInput, part_values};
use contracts::api::dashboard::{ItemKey, ScoreComponent};
use contracts::job::result::{JobOutcome, JobResult};
use contracts::job::spec::{CredentialKind, LlmCredentialRef};
use db::repo::jobs::ClaimParams;

use super::*;
use crate::dashboard::judge::JudgeRanker;
use crate::dashboard::score::{EligibilityJudgement, Freshness, JudgedPart, Judgement};
use crate::dashboard::snapshot;
use crate::pipeline::CREDENTIAL_ID;

/// The runner's fake-provider answer: every asked part 0 (defect 0 and 0), eligibility as computed.
fn neutral(input: &JudgeWorkInput) -> Value {
    let items: Vec<Value> = input
        .items
        .iter()
        .map(|item| {
            let defect = item
                .asks
                .contains(&ScoreComponent::DefectSeverity)
                .then(|| json!({"likelihood": 0, "impact": 0, "basis": "fake judgement"}));
            let parts: Vec<Value> = item
                .asks
                .iter()
                .filter(|c| part_values(**c).is_some())
                .map(|c| json!({"component": c, "points": 0, "basis": "fake judgement"}))
                .collect();
            let eligibility = item.asks_eligibility.then(
                || json!({"eligible": item.eligibility.eligible, "reason": "fake judgement"}),
            );
            json!({"key": item.key, "defect": defect, "parts": parts, "eligibility": eligibility,
                "also_keys": [], "alarm": null, "why": "Fake judgement.", "next": "Fake next step."})
        })
        .collect();
    json!({ "items": items })
}

/// The judgement the scorer applies for a neutral answer to `item`.
fn expected(item: &JudgeItem) -> ItemJudgement {
    let judgement = Judgement {
        parts: item
            .asks
            .iter()
            .map(|component| JudgedPart {
                component: *component,
                points: 0,
                basis: "fake judgement".to_owned(),
            })
            .collect(),
        eligibility: item.asks_eligibility.then(|| EligibilityJudgement {
            eligible: item.eligibility.eligible,
            reason: "fake judgement".to_owned(),
        }),
        alarm: false,
        also: Vec::new(),
        judged_at: now(),
        freshness: Freshness::Current,
    };
    ItemJudgement {
        judgement,
        text: None,
        older_prompt: false,
    }
}

/// Plays the runner until aborted: claims judge jobs and answers each neutrally; records the items it judged.
fn serve(state: AppState, judged: Arc<Mutex<Vec<JudgeItem>>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let now = Utc::now();
            let job = state
                .db
                .jobs()
                .claim(ClaimParams {
                    credential_ids: &[CREDENTIAL_ID.to_owned()],
                    job_token_hash: b"hash",
                    lease_until: now + TimeDelta::hours(1),
                    now,
                })
                .await
                .expect("claim");
            let Some(job) = job else {
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            };
            let input = JudgeWorkInput::from_prompt(&job.spec.prompt).expect("judge input");
            judged
                .lock()
                .expect("judged")
                .extend(input.items.iter().cloned());
            let result = JobResult {
                job_id: job.id,
                attempt: job.attempt,
                outcome: JobOutcome::Succeeded {
                    output: neutral(&input),
                },
                session: None,
                usage: None,
                skills_commit: None,
                llm_call: None,
            };
            state
                .db
                .jobs()
                .complete(&result, Utc::now())
                .await
                .expect("complete");
        }
    })
}

#[tokio::test]
async fn judged_refresh_ranks_the_fixture_world_as_the_scorer_expects() {
    let w = world_with(Arc::new(JudgeRanker), Duration::from_secs(24 * 3600)).await;
    w.state
        .db
        .llm_credentials()
        .register(
            DEFAULT_TENANT,
            &LlmCredentialRef {
                id: CREDENTIAL_ID.to_owned(),
                kind: CredentialKind::CodexChatgpt,
            },
            true,
            Utc::now(),
        )
        .await
        .expect("credential");
    let judged = Arc::new(Mutex::new(Vec::new()));
    let runner = serve(w.state.clone(), Arc::clone(&judged));
    let done = w.refreshed().await;
    let ranking = done.ranking.clone().expect("ranking");
    assert_eq!(ranking.state, RankingState::Finished, "{done:?}");
    let items: Vec<JudgeItem> = judged.lock().expect("judged").clone();
    let count = u32::try_from(items.len()).expect("count");
    assert!(count > 0);
    assert_eq!(
        (done.judged, done.reused, done.left_provisional),
        (count, 0, 0)
    );
    assert_eq!((ranking.judged, ranking.total), (count, count));

    let summary = w.summary(ALICE).await;
    let meta = summary.snapshot.expect("snapshot");
    assert_eq!(meta.item_counts.not_judged, 0);
    assert_eq!(summary.older_prompt_judgements, 0);
    let page = w.items("?limit=500").await;
    let (_, stored) = snapshot::load(&w.state, DEFAULT_TENANT, "u1")
        .await
        .expect("load")
        .expect("snapshot");
    let applied: HashMap<ItemKey, ItemJudgement> =
        items.iter().map(|i| (i.key.clone(), expected(i))).collect();
    let view = snapshot::build(&stored, &applied, None);
    let want: Vec<(&str, Tier, i32)> = view
        .items
        .iter()
        .map(|i| {
            (
                i.item.id.as_str(),
                i.item.score.tier,
                i.item.score.total_tenths,
            )
        })
        .collect();
    let got: Vec<(&str, Tier, i32)> = page
        .items
        .iter()
        .map(|i| (i.id.as_str(), i.score.tier, i.score.total_tenths))
        .collect();
    assert_eq!(got, want);
    assert!(page.items.iter().all(|i| {
        i.score.state != ScoreState::Unjudged
            && i.llm
                .as_ref()
                .is_some_and(|t| t.why == "Fake judgement." && !t.inherited)
    }));

    // Nothing changed: the next refresh reuses every judgement and creates no job.
    let again = w.refreshed().await;
    runner.abort();
    assert_eq!(again.ranking.map(|r| r.state), Some(RankingState::Finished));
    assert_eq!((again.judged, again.reused), (0, count));
    let jobs = w
        .state
        .db
        .jobs()
        .list_for_refresh(again.id, &[])
        .await
        .expect("jobs");
    assert!(jobs.is_empty());
    assert_eq!(judged.lock().expect("judged").len(), items.len());
}
