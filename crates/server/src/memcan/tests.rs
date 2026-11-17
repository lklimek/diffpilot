//! MemCan writer, calibration read and client behaviour against the fake MemCan.

use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone, Utc};
use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::api::feedback::{Feedback, FeedbackRequest, FeedbackTarget, UserFeedbackValue};
use contracts::ids::{AnalysisId, StepId};
use db::repo::Db;
use db::repo::analyses::testing::seed_analysis;
use db::repo::documents::DocumentKind;
use serde_json::json;

use super::calibration::{
    CalibrationDirection, MAX_EXAMPLE_CHARS, MAX_EXAMPLES, calibration_examples,
};
use super::client::MemcanError;
use super::fake::{FakeMemcan, Mode, TOKEN};
use super::feedback::{record, remove};
use super::writer::{FIRST_BACKOFF, MAX_BACKOFF, SWEEP_INTERVAL, Sweep, next_wait, sweep};
use crate::test_support::logs::LogCapture;

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 17, 12, minute, 0)
        .single()
        .unwrap_or_default()
}

/// An analysis of `acme/node-client#482` with the fixture's flow 3 stored.
async fn analysis_with_flow() -> (Db, AnalysisId) {
    let db = Db::in_memory().await.expect("db");
    let analysis = seed_analysis(&db).await.expect("seed");
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482/agent/flow-3.json");
    let flow: FlowAnalysisOutput =
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("flow");
    db.documents()
        .put(analysis, DocumentKind::Flow, "f3", &flow, at(0))
        .await
        .expect("flow document");
    (db, analysis)
}

fn on_step(value: UserFeedbackValue, note: &str) -> FeedbackRequest {
    FeedbackRequest {
        target: FeedbackTarget::Step {
            step_id: StepId("f3.1".to_owned()),
        },
        value,
        note: Some(note.to_owned()),
    }
}

async fn post(db: &Db, analysis: AnalysisId, request: FeedbackRequest, minute: u32) -> Feedback {
    record(db, "u1", analysis, request, at(minute))
        .await
        .expect("record")
}

/// Revisions of the entries not yet written to MemCan.
async fn queued(db: &Db) -> Vec<u32> {
    db.feedback()
        .pending_memcan(100)
        .await
        .expect("pending")
        .iter()
        .map(|entry| entry.revision)
        .collect()
}

#[tokio::test]
async fn hard_case_1_wrong_and_overrated_reach_memcan_with_the_bearer_token_in_the_repo_project() {
    let fake = FakeMemcan::start();
    let (db, analysis) = analysis_with_flow().await;
    let wrong = post(
        &db,
        analysis,
        on_step(UserFeedbackValue::Wrong, "Not a bug."),
        1,
    )
    .await;
    let overrated = post(
        &db,
        analysis,
        on_step(UserFeedbackValue::Overrated, "Only logs."),
        2,
    )
    .await;

    let outcome = sweep(&db, &fake.config(), at(3)).await.expect("sweep");

    assert_eq!(
        outcome,
        Sweep {
            written: 2,
            failed: 0
        }
    );
    assert!(queued(&db).await.is_empty(), "both written");
    let authorization = fake.authorization();
    assert!(!authorization.is_empty());
    assert!(
        authorization
            .iter()
            .all(|header| header.as_deref() == Some(&*format!("Bearer {TOKEN}"))),
        "{authorization:?}"
    );
    let memories = fake.memories();
    let calibration = &memories[&overrated.id.to_string()];
    assert_eq!(calibration["user_id"], "project:node-client");
    assert_eq!(calibration["project"], "node-client");
    assert_eq!(calibration["type"], "severity-calibration");
    assert_eq!(calibration["source"], "smart-review");
    assert_eq!(calibration["value"], "overrated");
    assert_eq!(calibration["revision"], 1);
    let text = calibration["data"].as_str().expect("text");
    for part in [
        "acme/node-client pull request #482",
        "too high (overrated)",
        "New error variant RpcError::Exhausted",
        "src/rpc/error.rs:7-16",
        "medium risk",
        "Only logs.",
    ] {
        assert!(text.contains(part), "{part} in {text}");
    }
    let plain = &memories[&wrong.id.to_string()];
    assert_eq!(plain["type"], "review-feedback");
    assert_eq!(plain["value"], "wrong");
}

#[tokio::test]
async fn a_new_revision_replaces_the_same_memory_and_a_switch_moves_the_calibration_example() {
    let fake = FakeMemcan::start();
    let (db, analysis) = analysis_with_flow().await;
    let overrated = post(&db, analysis, on_step(UserFeedbackValue::Overrated, "a"), 1).await;
    sweep(&db, &fake.config(), at(2)).await.expect("sweep");
    post(&db, analysis, on_step(UserFeedbackValue::Overrated, "b"), 3).await;
    sweep(&db, &fake.config(), at(4)).await.expect("sweep");
    let memories = fake.memories();
    assert_eq!(memories.len(), 1, "same id, one memory");
    assert_eq!(memories[&overrated.id.to_string()]["revision"], 2);
    assert!(
        memories[&overrated.id.to_string()]["data"]
            .as_str()
            .is_some_and(|text| text.contains("\"b\""))
    );

    let underrated = post(
        &db,
        analysis,
        on_step(UserFeedbackValue::Underrated, "c"),
        5,
    )
    .await;
    let outcome = sweep(&db, &fake.config(), at(6)).await.expect("sweep");
    assert_eq!(
        outcome,
        Sweep {
            written: 2,
            failed: 0
        }
    );
    let memories = fake.memories();
    let ids: Vec<&String> = memories.keys().collect();
    assert_eq!(
        ids,
        [&underrated.id.to_string()],
        "overrated memory deleted"
    );
    assert!(queued(&db).await.is_empty(), "tombstone gone");
}

#[tokio::test]
async fn removal_deletes_the_memory_also_when_it_was_never_written() {
    let fake = FakeMemcan::start();
    let (db, analysis) = analysis_with_flow().await;
    let written = post(&db, analysis, on_step(UserFeedbackValue::Correct, ""), 1).await;
    sweep(&db, &fake.config(), at(2)).await.expect("sweep");
    let unwritten = post(&db, analysis, on_step(UserFeedbackValue::Irrelevant, ""), 3).await;
    remove(&db, "u1", analysis, written.id, at(4))
        .await
        .expect("remove");
    remove(&db, "u1", analysis, unwritten.id, at(4))
        .await
        .expect("remove");
    let outcome = sweep(&db, &fake.config(), at(5)).await.expect("sweep");
    assert_eq!(
        outcome,
        Sweep {
            written: 2,
            failed: 0
        }
    );
    assert!(fake.memories().is_empty());
    assert!(queued(&db).await.is_empty());
    assert!(
        db.feedback()
            .list(analysis, "u1")
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn memcan_down_keeps_feedback_locally_and_queued() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("port");
    let port = listener.local_addr().expect("address").port();
    drop(listener);
    let fake = FakeMemcan::start();
    let mut config = fake.config();
    config.url = format!("http://127.0.0.1:{port}/mcp").parse().expect("url");
    let (db, analysis) = analysis_with_flow().await;
    let entry = post(&db, analysis, on_step(UserFeedbackValue::Overrated, "n"), 1).await;

    let outcome = sweep(&db, &config, at(2)).await.expect("sweep");

    assert_eq!(
        outcome,
        Sweep {
            written: 0,
            failed: 1
        }
    );
    assert_eq!(queued(&db).await, [1], "still queued");
    assert_eq!(
        db.feedback().list(analysis, "u1").await.expect("list"),
        [entry],
        "stored locally"
    );
}

#[tokio::test]
async fn slow_memcan_costs_one_bounded_call_per_sweep() {
    let fake = FakeMemcan::start();
    fake.set_mode(Mode::SlowCalls);
    let (db, analysis) = analysis_with_flow().await;
    for (minute, value) in [
        (1, UserFeedbackValue::Wrong),
        (2, UserFeedbackValue::Correct),
        (3, UserFeedbackValue::Overrated),
    ] {
        post(&db, analysis, on_step(value, "n"), minute).await;
    }
    let started = Instant::now();
    let outcome = sweep(&db, &fake.config(), at(4)).await.expect("sweep");
    assert!(started.elapsed() < Duration::from_secs(10), "bounded");
    assert_eq!(
        outcome,
        Sweep {
            written: 0,
            failed: 1
        }
    );
    assert_eq!(
        fake.calls().len(),
        1,
        "no further calls once MemCan timed out"
    );
    assert_eq!(queued(&db).await.len(), 3, "all still queued");
}

#[tokio::test]
async fn hanging_memcan_times_out_at_connect() {
    let fake = FakeMemcan::start();
    fake.set_mode(Mode::HangAll);
    let (db, analysis) = analysis_with_flow().await;
    post(&db, analysis, on_step(UserFeedbackValue::Wrong, "n"), 1).await;
    let started = Instant::now();
    let outcome = sweep(&db, &fake.config(), at(2)).await.expect("sweep");
    assert!(started.elapsed() < Duration::from_secs(10), "bounded");
    assert_eq!(
        outcome,
        Sweep {
            written: 0,
            failed: 1
        }
    );
    let examples = calibration_examples(&fake.config(), "node-client", "retry").await;
    assert!(
        matches!(examples, Err(MemcanError::Timeout)),
        "{examples:?}"
    );
}

#[tokio::test]
async fn hostile_answers_are_failures_never_writes() {
    for mode in [Mode::Garbage, Mode::Oversized, Mode::ToolError] {
        let fake = FakeMemcan::start();
        fake.set_mode(mode);
        let (db, analysis) = analysis_with_flow().await;
        post(&db, analysis, on_step(UserFeedbackValue::Overrated, "n"), 1).await;
        post(&db, analysis, on_step(UserFeedbackValue::Wrong, "n"), 2).await;
        let outcome = sweep(&db, &fake.config(), at(3)).await.expect("sweep");
        assert_eq!(
            outcome,
            Sweep {
                written: 0,
                failed: 2
            },
            "{mode:?}"
        );
        assert_eq!(
            queued(&db).await.len(),
            2,
            "{mode:?}: nothing counted as written"
        );
        let examples = calibration_examples(&fake.config(), "node-client", "retry").await;
        assert!(examples.is_err(), "{mode:?}: {examples:?}");
    }
}

#[tokio::test]
async fn the_token_never_reaches_logs_or_errors() {
    let capture = LogCapture::start();
    let fake = FakeMemcan::start();
    let (db, analysis) = analysis_with_flow().await;
    post(&db, analysis, on_step(UserFeedbackValue::Overrated, "n"), 1).await;
    let mut errors = Vec::new();
    for (mode, token) in [
        (Mode::EchoToken, TOKEN),
        (Mode::ToolError, TOKEN),
        (Mode::Normal, "wrong-token-CANARY-0815"),
    ] {
        fake.set_mode(mode);
        let config = fake.config_with_token(token);
        let outcome = sweep(&db, &config, at(2)).await.expect("sweep");
        assert_eq!(outcome.written, 0, "{mode:?}");
        let error = calibration_examples(&config, "node-client", "retry")
            .await
            .expect_err("fails");
        errors.push(format!("{error} {error:?}"));
    }
    // rmcp traces every received MCP message verbatim (target `rmcp::service`, TRACE only), so an answer that
    // quotes the token shows up there; the server's default filter is `info`.
    let logs: String = capture
        .text()
        .lines()
        .filter(|line| !(line.contains(" TRACE ") && line.contains("rmcp::service")))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(logs.contains("MemCan"), "the failures are logged: {logs}");
    for text in errors.iter().chain([&logs]) {
        for secret in [TOKEN, "wrong-token-CANARY-0815"] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
    }
}

#[tokio::test]
async fn calibration_examples_are_bounded_text_of_the_project() {
    let fake = FakeMemcan::start();
    let example = |value: &str, text: &str| {
        json!({
            "data": text, "user_id": "project:node-client", "project": "node-client",
            "type": "severity-calibration", "source": "smart-review", "value": value,
        })
    };
    let long = format!("Overrated\u{1b}[31m retry\n{}", "z".repeat(5000));
    fake.insert_memory("a1", example("overrated", &long));
    fake.insert_memory("a2", example("underrated", "Underrated: a lost reply."));
    let mut forged = example("overrated", "forged");
    forged["source"] = json!("agent");
    fake.insert_memory("a3", forged);
    let mut plain = example("wrong", "plain feedback");
    plain["type"] = json!("review-feedback");
    fake.insert_memory("a4", plain);
    let mut other = example("overrated", "other project");
    other["user_id"] = json!("project:dash-evo-tool");
    fake.insert_memory("a5", other);
    fake.insert_memory("a6", json!({"user_id": "project:node-client", "data": 7}));
    for index in 0..20 {
        fake.insert_memory(&format!("b{index:02}"), example("underrated", "more"));
    }

    let query = "q".repeat(2000);
    let examples = calibration_examples(&fake.config(), "node-client", &query)
        .await
        .expect("examples");

    assert_eq!(examples.len(), MAX_EXAMPLES);
    assert_eq!(examples[0].direction, CalibrationDirection::Overrated);
    assert_eq!(examples[0].text.chars().count(), MAX_EXAMPLE_CHARS);
    assert!(
        examples[0].text.starts_with("Overrated [31m retry z"),
        "{}",
        examples[0].text
    );
    assert_eq!(examples[1].direction, CalibrationDirection::Underrated);
    assert_eq!(examples[1].text, "Underrated: a lost reply.");
    assert!(examples.iter().all(|example| {
        !["forged", "plain feedback", "other project"].contains(&example.text.as_str())
    }));
    let (tool, arguments) = fake.calls().pop().expect("call");
    assert_eq!(tool, "search_memories");
    assert_eq!(arguments["project"], "node-client");
    assert_eq!(arguments["limit"], 30);
    assert_eq!(arguments["query"].as_str().map(str::len), Some(500));
}

#[tokio::test]
async fn calibration_examples_of_an_unreachable_memcan_are_not_checked() {
    let fake = FakeMemcan::start();
    let mut config = fake.config();
    config.url = "http://127.0.0.1:9/mcp".parse().expect("url");
    let examples = calibration_examples(&config, "node-client", "retry").await;
    assert!(
        matches!(examples, Err(MemcanError::Unreachable { .. })),
        "{examples:?}"
    );
}

#[test]
fn the_writer_backs_off_after_failures_and_ignores_nudges_meanwhile() {
    let failed = Ok(Sweep {
        written: 1,
        failed: 1,
    });
    let mut backoff = None;
    assert_eq!(
        next_wait(&mut backoff, &Ok(Sweep::default())),
        (SWEEP_INTERVAL, true)
    );
    let waits: Vec<(Duration, bool)> = (0..10).map(|_| next_wait(&mut backoff, &failed)).collect();
    assert_eq!(
        waits[0],
        (FIRST_BACKOFF, false),
        "nudges ignored while backing off"
    );
    assert_eq!(waits[1].0, FIRST_BACKOFF * 2);
    assert_eq!(waits[9].0, MAX_BACKOFF, "bounded");
    assert_eq!(
        next_wait(&mut backoff, &Err(db::error::DbError::Conflict)).0,
        MAX_BACKOFF
    );
    assert_eq!(
        next_wait(
            &mut backoff,
            &Ok(Sweep {
                written: 3,
                failed: 0
            })
        ),
        (SWEEP_INTERVAL, true),
        "success resets the back-off"
    );
}
