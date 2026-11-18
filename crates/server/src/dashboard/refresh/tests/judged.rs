//! A refresh of the dashboard world ranked by the judge ranker end to end: scan, `judge_work` jobs answered by the
//! test in the runner's place (neutral answers, as the runner's fake provider), stored judgements, ranked items.

use std::sync::Mutex;

use contracts::agent::judge_work::{JudgeItem, JudgeWorkInput, part_values};
use contracts::api::dashboard::{ItemKey, PartStatus, ScoreComponent};
use contracts::job::result::{JobOutcome, JobResult};
use contracts::job::spec::{CredentialKind, LlmCredentialRef};
use contracts::time::Timestamp;
use db::repo::jobs::ClaimParams;
use jiff::SignedDuration;

use super::*;
use crate::dashboard::judge::JudgeRanker;
use crate::dashboard::score::{EligibilityJudgement, Freshness, JudgedPart, Judgement};
use crate::dashboard::snapshot::{self, StoredLinkedIssue};
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

/// An answer the test runner gives to a judge job's input.
type Answer = Arc<dyn Fn(&JudgeWorkInput) -> Value + Send + Sync>;

/// Plays the runner until aborted: claims judge jobs and answers each neutrally; records the items it judged.
fn serve(state: AppState, judged: Arc<Mutex<Vec<JudgeItem>>>) -> tokio::task::JoinHandle<()> {
    serve_with(state, judged, Arc::new(neutral))
}

/// Plays the runner until aborted, answering each judge job with `answer`; records the items it judged.
fn serve_with(
    state: AppState,
    judged: Arc<Mutex<Vec<JudgeItem>>>,
    answer: Answer,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let now = Timestamp::now();
            let job = state
                .db
                .jobs()
                .claim(ClaimParams {
                    credential_ids: &[CREDENTIAL_ID.to_owned()],
                    job_token_hash: b"hash",
                    lease_until: now + SignedDuration::from_hours(1),
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
                    output: answer(&input),
                },
                session: None,
                usage: None,
                skills_commit: None,
                llm_call: None,
            };
            state
                .db
                .jobs()
                .complete(&result, Timestamp::now())
                .await
                .expect("complete");
        }
    })
}

/// The fixture world ranked by the judge ranker, with the runner's credential.
async fn judged_world() -> World {
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
            Timestamp::now(),
        )
        .await
        .expect("credential");
    w
}

/// One refresh of the fixture world judged with `answer`: its status and the ranked page.
async fn ranked_with(answer: Answer) -> (RefreshStatus, ItemPage) {
    let w = judged_world().await;
    let runner = serve_with(w.state.clone(), Arc::new(Mutex::new(Vec::new())), answer);
    let done = w.refreshed().await;
    runner.abort();
    (done, w.items("?limit=500").await)
}

/// Neutral answers, with `edit` applied to the answer of the item `key` when it is in the batch.
fn neutral_but(key: &'static str, edit: fn(&mut Value)) -> Answer {
    Arc::new(move |input: &JudgeWorkInput| {
        let mut answer = neutral(input);
        if let Some(item) = answer["items"]
            .as_array_mut()
            .and_then(|items| items.iter_mut().find(|i| i["key"] == key))
        {
            edit(item);
        }
        answer
    })
}

fn order(page: &ItemPage) -> Vec<(&str, Tier, i32)> {
    page.items
        .iter()
        .map(|i| (i.id.as_str(), i.score.tier, i.score.total_tenths))
        .collect()
}

/// A judged urgency bonus of 30 lifts acme/docs-site#77 from P2 (63.1) to P1 (93.1), to the top.
#[tokio::test]
async fn an_answer_raises_an_items_tier() {
    let (done, page) = ranked_with(neutral_but("acme~docs-site~pr~77", |item| {
        for part in item["parts"].as_array_mut().into_iter().flatten() {
            if part["component"] == "urgency_bonus" {
                part["points"] = json!(30);
                part["basis"] = json!("Production outage reported in the description.");
            }
        }
    }))
    .await;
    assert_eq!(done.ranking.map(|r| r.state), Some(RankingState::Finished));
    assert_eq!(
        order(&page)[..5],
        [
            ("DOC-77", Tier::P1, 931),
            ("PLAT-1203", Tier::P1, 746),
            ("WUI-320", Tier::P1, 674),
            ("NC-482", Tier::P1, 660),
            ("WUI-311", Tier::P1, 651),
        ]
    );
}

/// AR-9: a part the server computed from facts is never set by the judge. An answer replacing the computed
/// human-waiting part of acme/node-client#482 (10 → 15) is dropped; the rest of the answer counts, the order is the
/// neutral one.
#[tokio::test]
async fn an_override_of_a_fact_computed_part_changes_nothing() {
    let (done, page) = ranked_with(neutral_but("acme~node-client~pr~482", |item| {
        item["parts"].as_array_mut().expect("parts").push(json!({
            "component": "human_waiting",
            "points": 15,
            "basis": "The author asked for this review again yesterday."
        }));
    }))
    .await;
    assert_eq!(done.ranking.map(|r| r.state), Some(RankingState::Finished));
    assert_eq!(
        order(&page)[..3],
        [
            ("PLAT-1203", Tier::P1, 746),
            ("WUI-320", Tier::P1, 674),
            ("NC-482", Tier::P1, 660),
        ]
    );
    let item = &page.items[2];
    let waiting = item
        .score
        .parts
        .iter()
        .find(|p| p.component == ScoreComponent::HumanWaiting)
        .expect("part");
    assert_eq!(
        (waiting.points_tenths, waiting.status),
        (Some(100), PartStatus::Computed)
    );
    assert_eq!(
        item.score.state,
        ScoreState::Final,
        "the answer's asked parts apply"
    );
}

/// An estimated default sent for confirmation is the judge's to change: acme/wallet-ui#311's human-waiting
/// default 0 judged 15 lifts it from 65.1 to 80.1, to the top.
#[tokio::test]
async fn an_estimated_default_is_changed_by_the_judge() {
    let (_, page) = ranked_with(neutral_but("acme~wallet-ui~pr~311", |item| {
        for part in item["parts"].as_array_mut().into_iter().flatten() {
            if part["component"] == "human_waiting" {
                part["points"] = json!(15);
                part["basis"] = json!("The author asks us for a review in the latest comment.");
            }
        }
    }))
    .await;
    assert_eq!(
        order(&page)[..3],
        [
            ("WUI-311", Tier::P1, 801),
            ("PLAT-1203", Tier::P1, 746),
            ("WUI-320", Tier::P1, 674),
        ]
    );
    let waiting = page.items[0]
        .score
        .parts
        .iter()
        .find(|p| p.component == ScoreComponent::HumanWaiting)
        .expect("part");
    assert_eq!(
        (waiting.points_tenths, waiting.status),
        (Some(150), PartStatus::Judged)
    );
}

/// Answers that are never valid leave the ranking failed and every item in its provisional order: the scorer's
/// estimates stand for the LLM parts (e.g. acme/wallet-ui#320 at 90.4 instead of 67.4 with neutral answers).
#[tokio::test]
async fn invalid_output_leaves_the_provisional_order() {
    let (done, page) = ranked_with(Arc::new(
        |_: &JudgeWorkInput| json!({"items": "not a list"}),
    ))
    .await;
    assert_eq!(done.state, RefreshState::Finished);
    assert_eq!(done.ranking.map(|r| r.state), Some(RankingState::Failed));
    assert!(
        page.items
            .iter()
            .all(|i| i.score.state == ScoreState::Unjudged && i.llm.is_none())
    );
    assert_eq!(
        order(&page),
        [
            ("PLAT-1203", Tier::P1, 976),
            ("WUI-320", Tier::P1, 904),
            ("NC-482", Tier::P1, 760),
            ("WUI-311", Tier::P1, 751),
            ("PLAT-1176", Tier::P1, 722),
            ("DOC-77", Tier::P1, 681),
            ("NC-476", Tier::P1, 678),
            ("CON-1534", Tier::P1, 654),
            ("OPS-14", Tier::P2, 550),
            ("PLAT-1199", Tier::P2, 461),
            ("PLAT-1187", Tier::P2, 429),
            ("WUI-298", Tier::P2, 376),
            ("NC-490", Tier::P2, 356),
            ("PLAT-1210", Tier::P3, 333),
            ("CON-1541", Tier::P3, 302),
            ("TODO-3fa91c2e", Tier::P3, 263),
            ("CON-1520", Tier::P3, 243),
            ("CON-1525", Tier::P3, 191),
            ("PLAT-1215", Tier::P3, 177),
            ("WUI-305", Tier::P3, 111),
            ("TODO-91bd0a44", Tier::P3, -10),
        ]
    );
}

#[tokio::test]
async fn judged_refresh_ranks_the_fixture_world_as_the_scorer_expects() {
    let w = judged_world().await;
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
    // Plan §B.2: the scan keeps the issues a pull request closes (the judge input maps them, `judge::tests`).
    let fix = stored
        .pull_requests
        .iter()
        .find(|p| p.facts.repo.name == "platform" && p.facts.number == 1203)
        .expect("acme/platform#1203 scanned");
    assert_eq!(
        fix.linked_issues,
        vec![StoredLinkedIssue {
            number: 1187,
            title: "Block sync stalls after the 2.4 upgrade on three nodes".to_owned(),
        }]
    );
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
