use super::*;
use contracts::api::review_loop::{
    CommentSource, CommentSourceKind, DecisionKind, DecisionVia, SourceRole,
};
use contracts::assessment::*;
use db::repo::review_loop::{NewDecision, NewEffect};

async fn host_fixture(count: usize) -> (Fixture, Vec<ItemRecord>) {
    host_source_fixture(count, false).await
}

async fn host_source_fixture(count: usize, review_body: bool) -> (Fixture, Vec<ItemRecord>) {
    let f = fixture(HEAD).await;
    f.state
        .db
        .assessments()
        .set_config(
            DEFAULT_TENANT,
            &contracts::ids::UserId("u1".into()),
            &AssessmentConfig::default(),
        )
        .await
        .unwrap();
    let mut source_records = vec![];
    let mut sources = vec![];
    let mut items = vec![];
    let (kind, source_kind, key, fragment) = if review_body {
        (
            CommentItemKind::ReviewBody,
            CommentSourceKind::ReviewBody,
            "review",
            "pullrequestreview",
        )
    } else {
        (
            CommentItemKind::PrComment,
            CommentSourceKind::PrComment,
            "comment",
            "issuecomment",
        )
    };
    for n in 1..=count {
        let url = format!("https://github.com/acme/node-client/pull/482#{fragment}-{n}");
        let mut body =
            format!("Bound the retry loop for request {n}. Release the lock before waiting.");
        if review_body {
            body.push_str(" Add a timeout while waiting.");
        }
        let seen = ObservedItem {
            kind,
            author: "owner".into(),
            author_kind: AuthorKind::Human,
            url: Some(url.clone()),
            location: None,
            body: body.clone(),
            body_hash: crate::review_loop::markers::body_hash(&body),
            ..observed(&format!("{key}:{n}:1"))
        };
        let UpsertOutcome::New(item) = f
            .state
            .db
            .review_loop()
            .upsert_item(
                DEFAULT_TENANT,
                PR,
                &seen,
                ItemState::Undecided,
                Timestamp::now(),
            )
            .await
            .unwrap()
        else {
            panic!("new")
        };
        f.state
            .db
            .comment_sources()
            .put(
                DEFAULT_TENANT,
                PR,
                &CommentSource {
                    url: url.clone(),
                    kind: source_kind,
                    author: "owner".into(),
                    body: body.clone(),
                    body_hash: crate::review_loop::handin::hash(&body),
                    role: SourceRole::Source,
                    read_at: None,
                },
            )
            .await
            .unwrap();
        source_records.push(json!({"id":n,"url":url,"author":{"login":"owner"},"body":body}));
        sources.push(url);
        items.push(item);
    }
    let (reviews, comments) = if review_body {
        (source_records, vec![])
    } else {
        (vec![], source_records)
    };
    let data = json!({"manifest":{"repo":"acme/node-client","pr":482,"head":HEAD,"base_sha":BASE,"base_ref":"main","title":"Retry calls","body":"Bound retries","pr_author":"alice","maintainers":["owner"],"bots":[]},"threads":[],"review_comments":[],"reviews":reviews,"comments":comments,"conventions":{},"ids":{"next_id":1,"items":[]},"scope":{"threads":[],"sources":sources}});
    store_read(&f.state, "Bound retries", json!({"pr_data":data})).await;
    (f, items)
}

fn host_row(item: &ItemRecord, id: u32, quote: &str) -> Value {
    let source_kind = if item.kind == CommentItemKind::ReviewBody {
        "review_body"
    } else {
        "pr_comment"
    };
    json!({"id":format!("CMT-{id:03}"),"source_kind":source_kind,"reviewer":"owner","comment_url":item.url,"source_quote":quote,"description":"Bound retries","reasoning":"The loop has no upper bound.","assessment":"valid","proposed_action":"Fix here","proposed_decision":{"kind":"fix_here"}})
}

async fn complete_host(state: &AppState, rows: Vec<Value>) -> PassRecord {
    let pass = live_pass(state).await;
    let job = claim_method(state).await;
    let mut result = succeeded(job.id, Value::Null);
    result.assessment = job.spec.assessment.map(Box::new);
    result.report = Some(Box::new(serde_json::from_value(json!({"schema_version":"4.2.0","metadata":{"report_type":"comment_check","input_mode":"host_supplied","pr_number":482,"commit":pass.head},"comment_assessments":rows})).unwrap()));
    state
        .db
        .jobs()
        .complete(&result, Timestamp::now())
        .await
        .unwrap();
    wait_pass(state, pass.id).await;
    pass
}

async fn choose(
    state: &AppState,
    item: &ItemRecord,
    kind: DecisionKind,
    target: Option<DecisionTarget>,
    effects: Vec<NewEffect>,
) {
    state
        .db
        .review_loop()
        .decide(
            DEFAULT_TENANT,
            &[NewDecision {
                origin: None,
                item: item.id,
                item_revision: item.revision,
                user_id: "u1".into(),
                kind,
                target,
                reason: None,
                r#override: None,
                resolve_ticked: false,
                reply_text: None,
                comment: None,
                recommendation: None,
                via: DecisionVia::Question,
                carried_from: None,
                head: state.db.pull_requests().get(PR).await.unwrap().pr.head_sha,
                effects,
            }],
            Timestamp::now(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn host_incomplete_standing_targets_do_not_block_admission() {
    #[derive(Clone)]
    struct Log(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let log = Log(Default::default());
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    for (kind, target) in [
        (
            DecisionKind::UpstreamIssue,
            DecisionTarget::UpstreamIssue { url: None },
        ),
        (
            DecisionKind::FollowUpPr,
            DecisionTarget::FollowUpPr { title: None },
        ),
    ] {
        let (f, items) = host_fixture(2).await;
        choose(&f.state, &items[0], kind, Some(target), vec![]).await;
        assert_eq!(
            request_pass(&f.state, PR, PassReason::Opened)
                .await
                .unwrap(),
            PassQueued::Queued
        );
        let captured = String::from_utf8(std::mem::take(&mut *log.0.lock().unwrap())).unwrap();
        let omitted: Vec<_> = captured
            .lines()
            .filter(|line| {
                line.contains(
                    "standing decision omitted from assessment ids: required target missing",
                )
            })
            .collect();
        assert_eq!(omitted.len(), 1, "{captured}");
        assert!(
            omitted[0].ends_with(&format!("item_no=1 kind={kind:?}")),
            "{}",
            omitted[0]
        );
        let job = claim_method(&f.state).await;
        let data = job.spec.pr_data.unwrap();
        assert!(
            data.ids
                .items
                .iter()
                .find(|i| i.id == "CMT-001")
                .unwrap()
                .decision
                .is_none()
        );
        assert_eq!(
            f.state
                .db
                .assessments()
                .quota(&contracts::ids::UserId("u1".into()), Timestamp::now())
                .await
                .unwrap()
                .remaining,
            79
        );
    }
}

#[tokio::test]
async fn host_ask_outside_source_scope_is_terminal_or_answered() {
    let (f, items) = host_fixture(1).await;
    let data = f
        .state
        .db
        .review_loop()
        .pr_state(PR)
        .await
        .unwrap()
        .unwrap()
        .github_snapshot
        .unwrap();
    let mut data = data["pr_data"].clone();
    data["scope"]["sources"] =
        json!(["https://github.com/acme/node-client/pull/482#issuecomment-2"]);
    store_read(&f.state, "Bound retries", json!({"pr_data":data})).await;
    let id = question(&f.state, items[0].id, "Why?").await;
    let outcome = request_pass(&f.state, PR, PassReason::Ask).await;
    let question = f
        .state
        .db
        .questions()
        .get(id, Timestamp::now())
        .await
        .unwrap();
    assert!(
        question.job_id.is_some() || question.question.state == AskState::Failed,
        "{outcome:?}: {question:?}"
    );
    if question.job_id.is_some() {
        let pass = live_pass(&f.state).await;
        ingest_all(&f.state, &pass, Some("The caller retries without a bound.")).await;
        assert_eq!(
            f.state
                .db
                .questions()
                .get(id, Timestamp::now())
                .await
                .unwrap()
                .question
                .state,
            AskState::Answered
        );
    }
    assert!(
        f.state
            .db
            .questions()
            .next_unsent_for_pr(PR, Timestamp::now())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn marked_points_keep_their_ids_when_a_sibling_is_reassessed() {
    let (f, items) = host_fixture(1).await;
    let first = "Bound the retry loop for request 1.";
    let second = "Release the lock before waiting.";
    request_pass(&f.state, PR, PassReason::Opened)
        .await
        .unwrap();
    complete_host(
        &f.state,
        vec![
            host_row(&items[0], 100, first),
            host_row(&items[0], 101, second),
        ],
    )
    .await;
    let rl = f.state.db.review_loop();
    let marked = rl.item_by_no(PR, 100).await.unwrap().unwrap();
    let sibling = rl.item_by_no(PR, 101).await.unwrap().unwrap();
    let head_before = rl
        .latest_assessment(marked.id)
        .await
        .unwrap()
        .unwrap()
        .assessment
        .head;
    choose(
        &f.state,
        &marked,
        DecisionKind::NotAReviewPoint,
        None,
        vec![],
    )
    .await;

    move_head(&f.state, BASE).await;
    assert_eq!(
        request_pass(&f.state, PR, PassReason::AssessAgain(sibling.id))
            .await
            .unwrap(),
        PassQueued::Queued
    );
    let pass = live_pass(&f.state).await;
    assert_eq!(
        pass.sent.iter().map(|item| item.item).collect::<Vec<_>>(),
        [sibling.id]
    );
    let frozen_pass = f
        .state
        .db
        .assessments()
        .pass(
            DEFAULT_TENANT,
            &contracts::ids::UserId("u1".into()),
            pass.id,
        )
        .await
        .unwrap();
    let frozen = frozen::Frozen::read(&f.state, &frozen_pass).await.unwrap();
    let native_ids: Vec<_> = frozen
        .native
        .items
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    assert_eq!(native_ids, ["C101"]);
    assert_eq!(frozen.data.scope.sources, [sibling.url.clone().unwrap()]);
    let repeated = frozen
        .data
        .ids
        .items
        .iter()
        .find(|item| item.quote.as_deref() == Some(first));
    let reported_no = repeated
        .and_then(|item| item.id.strip_prefix("CMT-")?.parse::<u32>().ok())
        .unwrap_or(frozen.data.ids.next_id);
    let count_before = rl.items(PR).await.unwrap().len();
    complete_host(
        &f.state,
        vec![
            host_row(&marked, reported_no, first),
            host_row(&sibling, sibling.no, second),
        ],
    )
    .await;

    assert_eq!(
        rl.items(PR).await.unwrap().len(),
        count_before,
        "a shared source must not re-create the marked point"
    );
    assert_eq!(repeated.map(|item| item.id.as_str()), Some("CMT-100"));
    assert!(repeated.unwrap().decision.is_none());
    let current = rl.item(marked.id).await.unwrap();
    assert!(current.active_mark.is_some());
    assert!(current.gone_at.is_none());
    assert_eq!(
        rl.latest_assessment(marked.id)
            .await
            .unwrap()
            .unwrap()
            .assessment
            .head,
        head_before
    );
    assert_eq!(
        rl.latest_assessment(sibling.id)
            .await
            .unwrap()
            .unwrap()
            .assessment
            .head
            .0,
        BASE
    );

    // A report can use a fresh ID even though it was given the existing one.
    assert_eq!(
        request_pass(&f.state, PR, PassReason::AssessAgain(sibling.id))
            .await
            .unwrap(),
        PassQueued::Queued
    );
    let fresh_no = rl
        .items(PR)
        .await
        .unwrap()
        .iter()
        .map(|item| item.no)
        .max()
        .unwrap()
        + 1;
    complete_host(
        &f.state,
        vec![
            host_row(&marked, fresh_no, first),
            host_row(&sibling, sibling.no, second),
        ],
    )
    .await;
    assert_eq!(rl.items(PR).await.unwrap().len(), count_before);
    assert!(rl.item(marked.id).await.unwrap().active_mark.is_some());
}

#[tokio::test]
async fn fresh_id_with_a_retrimmed_quote_keeps_the_marked_point() {
    fresh_id_near_marked_point("Bound the retry loop for request 1. ", None, false, false).await;
}

#[tokio::test]
async fn fresh_id_with_a_partial_quote_keeps_the_marked_point() {
    fresh_id_near_marked_point("retry loop for request 1", None, false, false).await;
}

#[tokio::test]
async fn fresh_id_at_overlapping_lines_creates_a_new_nitpick() {
    fresh_id_near_marked_point(
        "Add a timeout while waiting.",
        Some("src/rpc/client.rs:41-44"),
        false,
        true,
    )
    .await;
}

#[tokio::test]
async fn fresh_id_in_an_updated_review_creates_a_new_point() {
    fresh_id_near_marked_point(
        "Bound the retry loop for request 1. Warn when the retry limit is reached.",
        None,
        true,
        true,
    )
    .await;
}

#[tokio::test]
async fn fresh_id_with_a_short_excerpt_creates_a_new_point() {
    fresh_id_near_marked_point("the retry loop", None, false, true).await;
}

#[tokio::test]
async fn fresh_id_with_normalised_whitespace_keeps_the_marked_point() {
    fresh_id_near_marked_point("Bound the   retry loop for request 1.", None, true, false).await;
}

async fn fresh_id_near_marked_point(
    quote: &str,
    location: Option<&str>,
    edited_review: bool,
    expect_created: bool,
) {
    let (f, items) = host_source_fixture(1, true).await;
    let source = &items[0];
    let mut first = host_row(source, 100, "Bound the retry loop for request 1.");
    first["location"] = json!("src/rpc/client.rs:40-42");
    let second = "Release the lock before waiting.";
    request_pass(&f.state, PR, PassReason::Opened)
        .await
        .unwrap();
    complete_host(&f.state, vec![first, host_row(source, 101, second)]).await;
    let rl = f.state.db.review_loop();
    let marked = rl.item_by_no(PR, 100).await.unwrap().unwrap();
    let sibling = rl.item_by_no(PR, 101).await.unwrap().unwrap();
    if edited_review {
        let mut data = rl
            .pr_state(PR)
            .await
            .unwrap()
            .unwrap()
            .github_snapshot
            .unwrap()["pr_data"]
            .clone();
        let mut source = f
            .state
            .db
            .comment_sources()
            .list(DEFAULT_TENANT, PR)
            .await
            .unwrap()
            .pop()
            .unwrap()
            .source;
        let body = format!("{quote} {second}");
        data["reviews"][0]["body"] = json!(body);
        source.body_hash = crate::review_loop::handin::hash(&body);
        source.body = body;
        f.state
            .db
            .comment_sources()
            .put(DEFAULT_TENANT, PR, &source)
            .await
            .unwrap();
        store_read(&f.state, "Bound retries", json!({"pr_data": data})).await;
    }
    assert_eq!(
        request_pass(&f.state, PR, PassReason::AssessAgain(sibling.id))
            .await
            .unwrap(),
        PassQueued::Queued,
    );
    // The mark may be set after the pass has frozen its input.
    choose(
        &f.state,
        &marked,
        DecisionKind::NotAReviewPoint,
        None,
        vec![],
    )
    .await;
    let mut repeated = host_row(source, 102, quote);
    if let Some(location) = location {
        repeated["location"] = json!(location);
    }
    let (logs, _subscriber) = crate::test_support::logs::LogCapture::start_scoped();
    let pass = complete_host(&f.state, vec![repeated, host_row(source, 101, second)]).await;
    let fresh = rl.item_by_no(PR, 102).await.unwrap();
    assert_eq!(
        fresh.is_some(),
        expect_created,
        "unexpected projection for {quote:?}"
    );
    if let Some(fresh) = fresh {
        assert_eq!(fresh.state, ItemState::Undecided);
        assert!(fresh.active_mark.is_none());
        assert_eq!(fresh.quote.as_deref(), Some(quote));
    }
    let logs = logs.text();
    let audit = logs
        .lines()
        .find(|line| line.contains("fresh assessment point suppressed by an internal mark"));
    assert_eq!(audit.is_none(), expect_created, "{logs}");
    if let Some(line) = audit {
        assert!(line.contains("INFO"), "{line}");
        assert!(line.contains(&format!("pr={PR}")), "{line}");
        assert!(line.contains(&format!("item={}", marked.no)), "{line}");
        assert!(line.contains(source.url.as_deref().unwrap()), "{line}");
    }
    assert!(rl.item(marked.id).await.unwrap().active_mark.is_some());
    assert_eq!(
        Some(
            rl.latest_assessment(sibling.id)
                .await
                .unwrap()
                .unwrap()
                .assessment
                .job
        ),
        pass.job_id,
        "the sibling still receives its assessment"
    );
}

#[tokio::test]
async fn host_new_point_fingerprint_carries_owner_effects_at_unchanged_code() {
    host_point_owner_effects(false).await;
}

#[tokio::test]
async fn host_new_point_fingerprint_reasks_and_cancels_owner_effects_at_changed_code() {
    host_point_owner_effects(true).await;
}

async fn host_point_owner_effects(changed: bool) {
    use contracts::api::review_loop::{EffectKind, EffectParams, EffectState, Executor};
    let (f, items) = host_fixture(2).await;
    if changed {
        move_head(&f.state, BASE).await;
    }
    request_pass(&f.state, PR, PassReason::Opened)
        .await
        .unwrap();
    let quote = "Release the lock before waiting.";
    let mut row = host_row(&items[0], 100, quote);
    row["location"] = json!("src/rpc/client.rs:40-43");
    complete_host(&f.state, vec![row, host_row(&items[1], 2, &items[1].body)]).await;
    let point = f
        .state
        .db
        .review_loop()
        .items(PR)
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.no == 100)
        .unwrap();
    choose(
        &f.state,
        &point,
        DecisionKind::FixHere,
        None,
        vec![NewEffect {
            seq: 1,
            after: None,
            kind: EffectKind::Reply,
            executor: Executor::Agent,
            params: EffectParams {
                text: Some("I will bound the retry loop.".into()),
                ..Default::default()
            },
        }],
    )
    .await;
    move_head(&f.state, HEAD).await;
    assert_eq!(
        request_pass(&f.state, PR, PassReason::AssessAgain(items[1].id))
            .await
            .unwrap(),
        PassQueued::Queued
    );
    let pass = live_pass(&f.state).await;
    assert!(!pass.sent.iter().any(|sent| sent.item == point.id));
    let frozen_pass = f
        .state
        .db
        .assessments()
        .pass(
            DEFAULT_TENANT,
            &contracts::ids::UserId("u1".into()),
            pass.id,
        )
        .await
        .unwrap();
    let frozen = frozen::Frozen::read(&f.state, &frozen_pass).await.unwrap();
    assert_eq!(frozen.standing[&point.id.to_string()].changed, changed);
    let head_summary = summary(HEAD);
    let mut code = super::super::CodeReader::new(&f.state, &head_summary);
    let current_code = code.lines(&point).await.unwrap();
    let base_summary = summary(BASE);
    let mut base_code = super::super::CodeReader::new(&f.state, &base_summary);
    assert_ne!(current_code, base_code.lines(&point).await.unwrap());
    let mut row = host_row(&items[1], 100, quote);
    row["location"] = json!("src/rpc/client.rs:40-43");
    complete_host(&f.state, vec![row]).await;
    let current = f.state.db.review_loop().item(point.id).await.unwrap();
    assert_eq!(
        current.state,
        if changed {
            ItemState::Reopened
        } else {
            ItemState::Executing
        }
    );
    let events = f
        .state
        .db
        .review_loop()
        .carry_events(DEFAULT_TENANT, Some(PR), None, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].kind,
        if changed {
            contracts::api::review_loop::CarryKind::AskedAgain
        } else {
            contracts::api::review_loop::CarryKind::Carried
        }
    );
    assert_eq!(
        events[0].reason,
        changed.then_some(contracts::api::review_loop::ReaskReason::CodeChanged)
    );
    let log = f
        .state
        .db
        .review_loop()
        .item_decisions(point.id)
        .await
        .unwrap();
    let effects: Vec<_> = log.iter().flat_map(|d| &d.decision.effects).collect();
    assert_eq!(effects.len(), 1);
    assert!(effects.iter().all(|e| e.state
        == if changed {
            EffectState::Cancelled
        } else {
            EffectState::Open
        }));
}

#[tokio::test]
async fn host_omitted_source_is_missing_but_explicit_status_is_consumed() {
    let (f, items) = host_fixture(2).await;
    request_pass(&f.state, PR, PassReason::Opened)
        .await
        .unwrap();
    let status = json!({"id":"CMT-100","source_kind":"status","reviewer":"owner","comment_url":items[1].url,"description":"No requests","reasoning":"This source reports progress.","proposed_action":"Keep context"});
    let pass = complete_host(&f.state, vec![status]).await;
    let report = f
        .state
        .db
        .assessments()
        .comparison(
            DEFAULT_TENANT,
            &contracts::ids::UserId("u1".into()),
            pass.id,
            Timestamp::now(),
        )
        .await
        .unwrap();
    let omitted = report.items.iter().find(|r| matches!(&r.source,AssessmentSource::Body {url,..} if Some(url)==items[0].url.as_ref())).unwrap();
    assert_eq!(omitted.status, ComparisonItemStatus::MissingRow);
    let sources = f
        .state
        .db
        .comment_sources()
        .list(DEFAULT_TENANT, PR)
        .await
        .unwrap();
    assert!(sources[0].source.read_at.is_none());
    assert!(sources[1].source.read_at.is_some());
    assert!(
        f.state
            .db
            .review_loop()
            .item(items[0].id)
            .await
            .unwrap()
            .gone_at
            .is_none()
    );
    assert!(
        f.state
            .db
            .review_loop()
            .item(items[1].id)
            .await
            .unwrap()
            .gone_at
            .is_some()
    );
}

#[tokio::test]
async fn host_cross_source_duplicates_are_not_resent_at_same_version() {
    for duplicate_no in [2, 100] {
        let (f, items) = host_fixture(2).await;
        request_pass(&f.state, PR, PassReason::Opened)
            .await
            .unwrap();
        let mut duplicate = host_row(&items[1], duplicate_no, &items[1].body);
        duplicate["merged_into"] = json!("CMT-001");
        duplicate.as_object_mut().unwrap().remove("assessment");
        complete_host(
            &f.state,
            vec![host_row(&items[0], 1, &items[0].body), duplicate],
        )
        .await;
        assert_eq!(
            request_pass(&f.state, PR, PassReason::Opened)
                .await
                .unwrap(),
            PassQueued::Skipped
        );
    }
}

#[tokio::test]
async fn host_merged_only_threads_are_not_resent_at_same_version() {
    let f = fixture(HEAD).await;
    let first = item(&f.state, "T1", "src/rpc/client.rs", (40, 43)).await;
    item(&f.state, "T2", "src/rpc/client.rs", (40, 43)).await;
    f.state
        .db
        .assessments()
        .set_config(
            DEFAULT_TENANT,
            &contracts::ids::UserId("u1".into()),
            &AssessmentConfig::default(),
        )
        .await
        .unwrap();
    request_pass(&f.state, PR, PassReason::Opened)
        .await
        .unwrap();
    let row = json!({"id":format!("CMT-{:03}",first.no),"source_kind":"inline","reviewer":"thepastaclaw","thread_id":"T1","merged_thread_ids":["T2"],"comment_url":"","description":"Bound retries","reasoning":"The loop has no upper bound.","assessment":"valid","proposed_action":"Fix here","proposed_decision":{"kind":"fix_here"}});
    complete_host(&f.state, vec![row]).await;
    assert_eq!(
        request_pass(&f.state, PR, PassReason::Opened)
            .await
            .unwrap(),
        PassQueued::Skipped
    );
}

#[tokio::test]
async fn host_source_cap_continues_to_the_sixth_source() {
    let (f, items) = host_fixture(6).await;
    request_pass(&f.state, PR, PassReason::Opened)
        .await
        .unwrap();
    let pass = live_pass(&f.state).await;
    assert_eq!(pass.sent.len(), 5);
    let rows = pass
        .sent
        .iter()
        .map(|s| {
            let item = items.iter().find(|i| i.id == s.item).unwrap();
            host_row(item, item.no, &item.body)
        })
        .collect();
    complete_host(&f.state, rows).await;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(next) = f
                .state
                .db
                .review_loop()
                .live_passes()
                .await
                .unwrap()
                .first()
            {
                assert_eq!(next.sent.len(), 1);
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("remaining source must get a continuation");
}

#[tokio::test]
async fn host_new_ids_retire_unreused_server_placeholders() {
    let (f, items) = host_fixture(1).await;
    request_pass(&f.state, PR, PassReason::Opened)
        .await
        .unwrap();
    complete_host(
        &f.state,
        vec![host_row(
            &items[0],
            100,
            "Bound the retry loop for request 1.",
        )],
    )
    .await;
    assert!(
        f.state
            .db
            .review_loop()
            .item(items[0].id)
            .await
            .unwrap()
            .gone_at
            .is_some()
    );
    assert_eq!(
        request_pass(&f.state, PR, PassReason::Opened)
            .await
            .unwrap(),
        PassQueued::Skipped
    );
}

#[tokio::test]
async fn host_gone_and_status_items_are_not_offered_for_reuse() {
    let (f, items) = host_fixture(2).await;
    let mut frozen = frozen::Frozen {
        authorities: Default::default(),
        own_reply_urls: Default::default(),
        own_reply_texts: Default::default(),
        written: Default::default(),
        data: serde_json::from_value(
            f.state
                .db
                .review_loop()
                .pr_state(PR)
                .await
                .unwrap()
                .unwrap()
                .github_snapshot
                .unwrap()["pr_data"]
                .clone(),
        )
        .unwrap(),
        items: vec![
            ItemRecord {
                gone_at: Some(Timestamp::now()),
                ..items[0].clone()
            },
            ItemRecord {
                status_comment: true,
                ..items[1].clone()
            },
        ],
        sent: vec![],
        native: golden_input(),
        standing: Default::default(),
        fingerprints: Default::default(),
        purpose_fingerprint: None,
    };
    frozen
        .ids(&f.state, PR, &summary(HEAD), None)
        .await
        .unwrap();
    assert!(frozen.data.ids.items.is_empty());
}

#[tokio::test]
async fn host_proposal_titles_use_the_public_text_guard() {
    for (title, expected) in [
        (
            "This assessment is partial. Bound retries.",
            Some("Bound retries."),
        ),
        ("This is verified.", None),
    ] {
        let (f, items) = host_fixture(1).await;
        request_pass(&f.state, PR, PassReason::Opened)
            .await
            .unwrap();
        let mut row = host_row(&items[0], 1, &items[0].body);
        row["proposed_decision"] = json!({"kind":"todo","title":title});
        complete_host(&f.state, vec![row]).await;
        let card = f
            .state
            .db
            .review_loop()
            .latest_assessment(items[0].id)
            .await
            .unwrap()
            .unwrap()
            .assessment;
        let title = card.proposed_decision.unwrap().title;
        assert_eq!(title.as_deref(), expected);
        assert_eq!(
            card.recommendation.target,
            title.map(|title| DecisionTarget::Todo { title })
        );
    }
}
