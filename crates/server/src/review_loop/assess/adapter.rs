//! Native method adapters into common comparison rows and primary-only projections.

use super::{frozen::Frozen, host, ingest};
use crate::review_loop::handin;
use contracts::agent::comment_report::*;
use contracts::api::review_loop::*;
use contracts::assessment::*;
use contracts::ids::CommitSha;
use contracts::job::result::{JobFailureKind, JobResult};
use contracts::time::Timestamp;
use db::repo::assessments::{PrimaryCarry, PrimaryPoint};
use db::repo::review_loop::{ItemRecord, ObservedItem};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

type Normalized = (Vec<AssessmentComparisonItem>, Vec<PrimaryPoint>);

pub(super) fn normalize(
    frozen: &Frozen,
    pass: &AssessmentPass,
    run: &AssessmentMethodRun,
    result: &JobResult,
    output: Value,
) -> Result<Normalized, JobFailureKind> {
    match run.reference.method {
        AssessmentMethod::ClaudiusHost => host_rows(frozen, pass, run, result, output),
        AssessmentMethod::PromptV4 => native_rows(frozen, pass, run, result, output),
    }
}

fn host_rows(
    frozen: &Frozen,
    pass: &AssessmentPass,
    run: &AssessmentMethodRun,
    result: &JobResult,
    output: Value,
) -> Result<Normalized, JobFailureKind> {
    let raw_report = result
        .report
        .as_ref()
        .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
        .unwrap_or(output);
    let report = host::decode(&frozen.data, raw_report.clone())?;
    if report.dropped > 0 {
        tracing::warn!(pass=%pass.id, dropped=report.dropped,"ungrounded or malformed assessment rows dropped");
    }
    let mut rows = Vec::new();
    let mut writes = Vec::new();
    let mut identities = HashMap::new();
    let mut semantic_dropped = 0;
    let mut point_ordinals = HashMap::<String, u32>::new();
    for row in report.rows.iter().filter(|r| {
        matches!(
            r.source_kind,
            SourceKind::Inline | SourceKind::ReviewBody | SourceKind::PrComment
        )
    }) {
        let Some(source) = source(frozen, row) else {
            continue;
        };
        let existing = existing(frozen, row);
        let key = comparison_key(&source, existing);
        identities.insert(row.id.clone(), key.clone());
        let duplicate = row
            .merged_into
            .as_ref()
            .and_then(|id| report.rows.iter().find(|r| &r.id == id));
        let assessed = duplicate.unwrap_or(row);
        let mut grouped = assessed.clone();
        let mut members = assessed.merged_thread_ids.clone();
        if let Some(thread) = &assessed.thread_id {
            members.push(thread.clone());
        }
        grouped.gates.extend(
            report
                .rows
                .iter()
                .filter(|r| r.thread_id.as_ref().is_some_and(|id| members.contains(id)))
                .flat_map(|r| r.gates.clone()),
        );
        let card = host_card(&grouped, result, &report.head, &frozen.data.manifest.repo);
        if card.is_none() {
            semantic_dropped += 1;
            continue;
        }
        let mut common = common(run, row.id.clone(), key.clone(), source.clone(), existing);
        common.recorded_decision = row.recorded_decision.clone();
        common.proposal = row.proposed_decision.clone();
        common.native_verdict = row
            .assessment
            .and_then(|v| serde_json::to_value(v).ok())
            .and_then(|v| v.as_str().map(str::to_owned));
        common.verdict = row.assessment.map(verdict);
        if row.merged_into.is_some() {
            common.verdict = None;
        }
        if report.head != pass.input.head.0 {
            common.status = ComparisonItemStatus::HeadMismatch;
        }
        let source_binding = match &source {
            AssessmentSource::Body {
                url,
                version,
                quote,
                ..
            } => Some((url.clone(), version.clone(), quote.clone())),
            _ => None,
        };
        let observed = if existing.is_none() && source_binding.is_some() {
            let value = find_source(frozen, row)?;
            let prefix = format!(
                "{}:{}:",
                if row.source_kind == SourceKind::ReviewBody {
                    "review"
                } else {
                    "comment"
                },
                source_id(value)
            );
            let ordinal = point_ordinals.entry(prefix.clone()).or_insert_with(|| {
                frozen
                    .items
                    .iter()
                    .filter_map(|i| i.stable_key.strip_prefix(&prefix)?.parse::<u32>().ok())
                    .max()
                    .unwrap_or(0)
            });
            *ordinal += 1;
            Some(ObservedItem {
                kind: if row.source_kind == SourceKind::ReviewBody {
                    CommentItemKind::ReviewBody
                } else {
                    CommentItemKind::PrComment
                },
                stable_key: format!("{prefix}{ordinal}"),
                author: row.reviewer.clone(),
                author_kind: if frozen
                    .data
                    .manifest
                    .bots
                    .iter()
                    .any(|b| b.eq_ignore_ascii_case(&row.reviewer))
                    || row.reviewer.ends_with("[bot]")
                {
                    AuthorKind::Bot
                } else {
                    AuthorKind::Human
                },
                url: Some(row.comment_url.clone()),
                location: row.location.as_deref().and_then(parse_location),
                title: row.description.chars().take(300).collect(),
                body: row.source_quote.clone().unwrap_or_default(),
                body_hash: crate::review_loop::markers::body_hash(
                    row.source_quote.as_deref().unwrap_or_default(),
                ),
                head: pass.input.head.clone(),
                round: None,
                bot_severity: None,
                github: ItemGithubFacts {
                    resolved: None,
                    resolved_by: None,
                    outdated: None,
                    bot_status: None,
                    edited_after_decision: false,
                    replies: 0,
                    marker: None,
                },
                outside: None,
            })
        } else {
            None
        };
        if report.head == pass.input.head.0 {
            writes.push(PrimaryPoint {
                carry: existing.and_then(|item| carry(frozen, item, row)),
                key: key.clone(),
                existing: existing.map(|i| (i.id, i.body_hash.clone())),
                observed,
                preferred_no: row
                    .id
                    .strip_prefix("CMT-")
                    .and_then(|n| n.parse().ok())
                    .filter(|n| *n >= frozen.data.ids.next_id)
                    .unwrap_or(frozen.data.ids.next_id),
                source: source_binding,
                duplicate: None,
                assessment: if row.merged_into.is_none() {
                    card
                } else {
                    None
                },
                fingerprint: existing
                    .and_then(|i| frozen.fingerprints.get(&i.id.to_string()))
                    .cloned()
                    .unwrap_or_default(),
            });
        }
        rows.push(common);
    }
    if semantic_dropped > 0 {
        tracing::warn!(pass=%pass.id,dropped=semantic_dropped,"assessment rows failed card validation and stay not assessed");
    }
    // Only direct, successfully validated canonical rows can own a duplicate.
    let canonical: HashSet<_> = rows
        .iter()
        .filter(|r| {
            report
                .rows
                .iter()
                .find(|raw| raw.id == r.local_id)
                .is_some_and(|r| r.merged_into.is_none())
        })
        .map(|r| r.comparison_key.clone())
        .collect();
    rows.retain_mut(|row| {
        let raw = report.rows.iter().find(|r| r.id == row.local_id);
        if let Some(target) = raw.and_then(|r| r.merged_into.as_ref()) {
            let Some(key) = identities
                .get(target)
                .filter(|key| canonical.contains(*key) && **key != row.comparison_key)
            else {
                return false;
            };
            row.status = ComparisonItemStatus::Duplicate;
            row.duplicate_of = Some(key.clone());
            row.verdict = None;
            if let Some(write) = writes.iter_mut().find(|w| w.key == row.comparison_key) {
                write.duplicate = Some(key.clone());
            }
        }
        true
    });
    writes.retain(|w| rows.iter().any(|r| r.comparison_key == w.key));
    for raw in report.rows.iter().filter(|r| r.merged_into.is_none()) {
        let Some(canonical) = identities
            .get(&raw.id)
            .filter(|key| rows.iter().any(|r| &r.comparison_key == *key))
        else {
            continue;
        };
        for thread in &raw.merged_thread_ids {
            if rows.iter().any(|r|matches!(&r.source,AssessmentSource::Inline {thread_id,..} if thread_id==thread)) {continue;}
            let Some(offered) = frozen.data.ids.items.iter().find(|i| &i.key == thread) else {
                continue;
            };
            let Some(item) = frozen
                .items
                .iter()
                .find(|i| offered.id == format!("CMT-{:03}", i.no))
            else {
                continue;
            };
            let Some(source) = item_source(frozen, item) else {
                continue;
            };
            let key = comparison_key(&source, Some(item));
            let mut linked = common(run, offered.id.clone(), key.clone(), source, Some(item));
            linked.status = ComparisonItemStatus::Duplicate;
            linked.duplicate_of = Some(canonical.clone());
            if report.head == pass.input.head.0 {
                writes.push(PrimaryPoint {
                    carry: None,
                    key,
                    existing: Some((item.id, item.body_hash.clone())),
                    observed: None,
                    preferred_no: item.no,
                    source: None,
                    duplicate: Some(canonical.clone()),
                    assessment: None,
                    fingerprint: frozen
                        .sent
                        .iter()
                        .find(|s| s.item == item.id)
                        .map_or_else(|| item.body_hash.clone(), |s| s.fingerprint.clone()),
                });
            }
            rows.push(linked);
        }
    }
    for (index, raw) in raw_report["comment_assessments"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if !matches!(
            raw["source_kind"].as_str(),
            Some("review_body" | "pr_comment")
        ) {
            continue;
        }
        let Some(url) = raw["comment_url"]
            .as_str()
            .filter(|url| frozen.data.scope.sources.iter().any(|s| s == url))
        else {
            continue;
        };
        let id = raw["id"].as_str().unwrap_or_default();
        if rows.iter().any(|r| r.local_id == id) {
            continue;
        }
        let Some(source) = frozen
            .data
            .reviews
            .iter()
            .chain(&frozen.data.comments)
            .find(|s| host::url(s) == url)
        else {
            continue;
        };
        let source = AssessmentSource::Body {
            url: url.into(),
            version: handin::hash(host::body(source)),
            quote: String::new(),
            source_kind: if raw["source_kind"] == "review_body" {
                CommentSourceKind::ReviewBody
            } else {
                CommentSourceKind::PrComment
            },
        };
        let item = frozen
            .items
            .iter()
            .find(|i| id == format!("CMT-{:03}", i.no));
        let key = item.map_or_else(
            || format!("ungrounded:{index}"),
            |i| comparison_key(&source, Some(i)),
        );
        let mut missing = common(run, id.into(), key, source, item);
        missing.status = ComparisonItemStatus::MissingRow;
        rows.push(missing);
    }
    for url in &frozen.data.scope.sources {
        if rows
            .iter()
            .any(|r| matches!(&r.source,AssessmentSource::Body {url:source,..} if source==url))
        {
            continue;
        }
        let Some(raw) = frozen
            .data
            .reviews
            .iter()
            .chain(&frozen.data.comments)
            .find(|s| host::url(s) == url)
        else {
            continue;
        };
        let source = AssessmentSource::Body {
            url: url.clone(),
            version: handin::hash(host::body(raw)),
            quote: String::new(),
            source_kind: if frozen.data.reviews.iter().any(|s| host::url(s) == url) {
                CommentSourceKind::ReviewBody
            } else {
                CommentSourceKind::PrComment
            },
        };
        let key = format!("source:{}", handin::hash(url));
        let mut absent = common(run, key.clone(), key, source, None);
        absent.status = if report.head != pass.input.head.0 {
            ComparisonItemStatus::HeadMismatch
        } else if report.rows.iter().any(|r| {
            r.comment_url == *url && matches!(r.source_kind, SourceKind::Status | SourceKind::Reply)
        }) {
            ComparisonItemStatus::Unmatched
        } else {
            ComparisonItemStatus::MissingRow
        };
        rows.push(absent);
    }
    for row in report
        .rows
        .iter()
        .filter(|r| matches!(r.source_kind, SourceKind::Reply | SourceKind::Status))
    {
        let Some(recorded) = &row.recorded_decision else {
            continue;
        };
        let node = host::threads(&frozen.data).find(|thread| {
            thread
                .pointer("/comments/nodes")
                .or_else(|| thread.get("comments"))
                .and_then(Value::as_array)
                .is_some_and(|comments| {
                    comments
                        .iter()
                        .any(|c| host::url(c) == recorded.comment_url)
                })
        });
        let source = if let Some(node) = node {
            AssessmentSource::Inline {
                thread_id: node["id"].as_str().unwrap_or_default().into(),
                version: handin::hash(
                    serde_json::to_vec(node).map_err(|_| JobFailureKind::OutputNotJson)?,
                ),
            }
        } else {
            let Some(raw) = frozen
                .data
                .reviews
                .iter()
                .chain(&frozen.data.comments)
                .find(|c| host::url(c) == recorded.comment_url)
            else {
                continue;
            };
            AssessmentSource::Body {
                source_kind: if frozen
                    .data
                    .reviews
                    .iter()
                    .any(|r| host::url(r) == recorded.comment_url)
                {
                    CommentSourceKind::ReviewBody
                } else {
                    CommentSourceKind::PrComment
                },
                url: recorded.comment_url.clone(),
                version: handin::hash(host::body(raw)),
                quote: recorded.quote.clone(),
            }
        };
        let key = format!("context:{}", row.id);
        let mut context = common(run, row.id.clone(), key, source, None);
        context.status = ComparisonItemStatus::Context;
        context.recorded_decision = Some(recorded.clone());
        rows.push(context);
    }
    dedup_keys(&mut rows, &mut writes);
    let read_sources: HashSet<_> = rows
        .iter()
        .filter_map(|r| {
            if matches!(
                r.status,
                ComparisonItemStatus::MissingRow
                    | ComparisonItemStatus::HeadMismatch
                    | ComparisonItemStatus::Context
            ) {
                return None;
            }
            match &r.source {
                AssessmentSource::Body { url, .. } => Some(url.clone()),
                _ => None,
            }
        })
        .collect();
    missing(frozen, run, &mut rows);
    rows.retain(|row| {
        row.status != ComparisonItemStatus::MissingRow
            || !frozen.items.iter().any(|item| {
                Some(item.id) == row.primary_item_id
                    && item.source_id.is_none()
                    && item
                        .url
                        .as_ref()
                        .is_some_and(|url| read_sources.contains(url))
            })
    });
    Ok((rows, writes))
}

fn source_id(value: &Value) -> String {
    value["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value["id"].to_string())
}

fn find_source<'a>(frozen: &'a Frozen, row: &ReportRow) -> Result<&'a Value, JobFailureKind> {
    frozen
        .data
        .reviews
        .iter()
        .chain(&frozen.data.comments)
        .find(|v| host::url(v) == row.comment_url)
        .ok_or(JobFailureKind::OutputNotJson)
}

fn source(frozen: &Frozen, row: &ReportRow) -> Option<AssessmentSource> {
    match row.source_kind {
        SourceKind::Inline => {
            let id = row.thread_id.as_ref()?;
            let t = host::threads(&frozen.data).find(|t| t["id"] == *id)?;
            Some(AssessmentSource::Inline {
                thread_id: id.clone(),
                version: handin::hash(serde_json::to_vec(t).ok()?),
            })
        }
        SourceKind::ReviewBody | SourceKind::PrComment => Some(AssessmentSource::Body {
            source_kind: if row.source_kind == SourceKind::ReviewBody {
                CommentSourceKind::ReviewBody
            } else {
                CommentSourceKind::PrComment
            },
            url: row.comment_url.clone(),
            version: handin::hash(host::body(find_source(frozen, row).ok()?)),
            quote: row.source_quote.clone()?,
        }),
        _ => None,
    }
}

fn existing<'a>(frozen: &'a Frozen, row: &ReportRow) -> Option<&'a ItemRecord> {
    let no = row.id.strip_prefix("CMT-")?.parse::<u32>().ok()?;
    let item = frozen
        .items
        .iter()
        .find(|i| i.no == no && i.gone_at.is_none())?;
    if !item.author.eq_ignore_ascii_case(&row.reviewer) {
        return None;
    }
    let offered = frozen.data.ids.items.iter().find(|i| i.id == row.id)?;
    match row.source_kind {
        SourceKind::Inline => (offered.key == *row.thread_id.as_ref()?).then_some(item),
        SourceKind::ReviewBody | SourceKind::PrComment => {
            let quote = row.source_quote.as_deref()?;
            let old = offered.quote.as_deref()?;
            let same_source = item.url.as_deref() == Some(row.comment_url.as_str())
                && (quote.contains(old) || old.contains(quote));
            let same_quote = crate::review_loop::markers::body_hash(quote)
                == crate::review_loop::markers::body_hash(old);
            let location = row.location.as_deref().and_then(parse_location);
            let same_lines = location
                .as_ref()
                .zip(item.location.as_ref())
                .is_some_and(|(a, b)| {
                    a.path == b.path
                        && a.lines.zip(b.lines).is_some_and(|(a, b)| {
                            a.start_line <= b.end_line && b.start_line <= a.end_line
                        })
                });
            (same_source || same_quote || same_lines).then_some(item)
        }
        _ => None,
    }
}

fn comparison_key(source: &AssessmentSource, existing: Option<&ItemRecord>) -> String {
    match source {
        AssessmentSource::Inline { thread_id, .. } => format!("thread:{thread_id}"),
        AssessmentSource::Body {
            url,
            version,
            quote,
            ..
        } => existing.map_or_else(
            || {
                format!(
                    "body:{}",
                    handin::hash(format!("{url}\n{version}\n{quote}"))
                )
            },
            |i| format!("item:{}", i.id),
        ),
    }
}

fn common(
    run: &AssessmentMethodRun,
    local_id: String,
    key: String,
    source: AssessmentSource,
    item: Option<&ItemRecord>,
) -> AssessmentComparisonItem {
    AssessmentComparisonItem {
        recorded_decision: None,
        comparison_key: key,
        local_id,
        run: run.reference.clone(),
        method_version: run.version.clone(),
        source,
        primary_item_id: item.map(|i| i.id),
        primary_item_hash: item.map(|i| i.body_hash.clone()),
        status: ComparisonItemStatus::Matched,
        duplicate_of: None,
        verdict: None,
        native_verdict: None,
        proposal: None,
        owner_decision: None,
    }
}

fn verdict(value: ReportAssessment) -> AssessmentVerdict {
    match value {
        ReportAssessment::Valid => AssessmentVerdict::Valid,
        ReportAssessment::Wrong => AssessmentVerdict::Invalid,
        ReportAssessment::AlreadyFixed => AssessmentVerdict::AlreadyFixed,
        ReportAssessment::OutOfScope => AssessmentVerdict::OutOfScope,
    }
}

fn host_card(
    row: &ReportRow,
    result: &JobResult,
    head: &str,
    repo: &str,
) -> Option<CommentAssessment> {
    let assessed = row.assessment?;
    let proposal = row.proposed_decision.as_ref()?;
    if ingest::fit_text(&row.reasoning, 1500).is_err()
        || ingest::fit_text(&row.description, 1500).is_err()
    {
        return None;
    }
    let kind: DecisionKind =
        serde_json::from_value(serde_json::to_value(proposal.kind).ok()?).ok()?;
    let title = proposal
        .title
        .as_deref()
        .map(|t| ingest::public(t, &row.id, "title"))
        .filter(|t| !t.is_empty() && ingest::fit_text(t, 600).is_ok());
    let target = match kind {
        DecisionKind::HandledInPr => {
            local_pr(proposal, repo).map(|number| DecisionTarget::ExistingPr { number })
        }
        DecisionKind::FollowUpPr => {
            if proposal.target_pr.is_some() || proposal.target_url.is_some() {
                None
            } else {
                Some(DecisionTarget::FollowUpPr {
                    title: title.clone(),
                })
            }
        }
        DecisionKind::UpstreamIssue => Some(DecisionTarget::UpstreamIssue {
            url: proposal.target_url.clone(),
        }),
        DecisionKind::Todo => title.clone().map(|title| DecisionTarget::Todo { title }),
        _ => None,
    };
    let mut guarded_proposal = proposal.clone();
    guarded_proposal.title = title;
    let mut card = CommentAssessment {
        proposed_decision: Some(guarded_proposal),
        job: result.job_id,
        head: CommitSha(head.into()),
        assessed_at: Timestamp::now(),
        reused_since: None,
        validity: match assessed {
            ReportAssessment::Valid | ReportAssessment::OutOfScope => Validity::Valid,
            ReportAssessment::Wrong => Validity::Invalid,
            ReportAssessment::AlreadyFixed => Validity::AlreadyFixed,
        },
        evidence: vec![],
        evidence_text: row
            .evidence
            .clone()
            .filter(|t| ingest::fit_text(t, 1500).is_ok()),
        recorded_decision: row.recorded_decision.clone(),
        pre_existing: row.pre_existing,
        category: row
            .category
            .clone()
            .filter(|c| ingest::CATEGORIES.contains(&c.as_str()) || c.starts_with("other: "))
            .unwrap_or_else(|| "other".into()),
        necessity: Necessity {
            verdict: if assessed == ReportAssessment::OutOfScope {
                NecessityVerdict::Out
            } else {
                NecessityVerdict::In
            },
            reason: row.reasoning.clone(),
            better_pr: None,
        },
        gates: row
            .gates
            .iter()
            .filter_map(|g| serde_json::from_value(Value::String(g.gate.clone())).ok())
            .collect(),
        elsewhere: vec![],
        fix_sketch: None,
        fix_size: row.fix_size,
        reply_drafts: row
            .proposed_reply
            .iter()
            .map(|r| ingest::public(r, &row.id, "proposed_reply"))
            .filter(|r| !r.is_empty() && ingest::fit_text(r, 1500).is_ok())
            .collect(),
        recommendation: Recommendation {
            kind,
            target,
            reason: ingest::public(&row.reasoning, &row.id, "reason"),
            reason_class: "source assessment".into(),
        },
        code: None,
        prompt_version: host::PROMPT_VERSION,
        skills_commit: result.skills_commit.clone(),
    };
    let mut seen = HashSet::new();
    card.gates.retain(|gate| seen.insert(*gate));
    ingest::guard_posted(&mut card);
    Some(card)
}

fn native_rows(
    frozen: &Frozen,
    pass: &AssessmentPass,
    run: &AssessmentMethodRun,
    result: &JobResult,
    output: Value,
) -> Result<Normalized, JobFailureKind> {
    let native = ingest::parse(output)
        .map_err(|_| JobFailureKind::OutputNotJson)?
        .ok_or(JobFailureKind::OutputNotJson)?;
    let mut rows = vec![];
    let mut writes = vec![];
    for entry in native.items {
        let Some(item) = frozen.items.iter().find(|i| {
            entry.item == format!("C{}", i.no) && frozen.sent.iter().any(|s| s.item == i.id)
        }) else {
            continue;
        };
        let Ok(card) = ingest::assessment(
            &entry,
            &ingest::Made {
                job: result.job_id,
                skills_commit: result.skills_commit.clone(),
            },
            &pass.input.head,
            Timestamp::now(),
        ) else {
            continue;
        };
        let Some(source) = item_source(frozen, item) else {
            continue;
        };
        let key = comparison_key(&source, Some(item));
        let mut common = common(run, entry.item.clone(), key.clone(), source, Some(item));
        common.native_verdict = serde_json::to_value(entry.validity)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned));
        common.verdict = match card.validity {
            Validity::Valid => Some(if card.necessity.verdict == NecessityVerdict::Out {
                AssessmentVerdict::OutOfScope
            } else {
                AssessmentVerdict::Valid
            }),
            Validity::Invalid => Some(AssessmentVerdict::Invalid),
            Validity::AlreadyFixed => Some(AssessmentVerdict::AlreadyFixed),
            Validity::PartlyValid => None,
        };
        common.proposal = Some(ProposedDecision {
            kind: serde_json::from_value(
                serde_json::to_value(entry.recommendation.kind)
                    .map_err(|_| JobFailureKind::OutputNotJson)?,
            )
            .map_err(|_| JobFailureKind::OutputNotJson)?,
            fixed_in: None,
            target_pr: entry.recommendation.target_pr,
            target_url: entry.recommendation.target_url.clone(),
            title: entry.recommendation.title.clone(),
        });
        let binding = match &common.source {
            AssessmentSource::Body {
                url,
                version,
                quote,
                ..
            } if host::distinct_quote(
                frozen
                    .data
                    .reviews
                    .iter()
                    .chain(&frozen.data.comments)
                    .find(|s| host::url(s) == url)
                    .map_or("", host::body),
                quote,
            ) =>
            {
                Some((url.clone(), version.clone(), quote.clone()))
            }
            AssessmentSource::Body { .. } => {
                common.status = ComparisonItemStatus::Unmatched;
                None
            }
            _ => None,
        };
        writes.push(PrimaryPoint {
            carry: None,
            key,
            existing: Some((item.id, item.body_hash.clone())),
            observed: None,
            preferred_no: item.no,
            source: binding,
            duplicate: None,
            assessment: Some(card),
            fingerprint: frozen
                .sent
                .iter()
                .find(|s| s.item == item.id)
                .map_or_else(|| item.body_hash.clone(), |s| s.fingerprint.clone()),
        });
        rows.push(common);
    }
    dedup_keys(&mut rows, &mut writes);
    missing(frozen, run, &mut rows);
    Ok((rows, writes))
}

fn item_source(frozen: &Frozen, item: &ItemRecord) -> Option<AssessmentSource> {
    if item.kind == CommentItemKind::Thread {
        let offered = frozen
            .data
            .ids
            .items
            .iter()
            .find(|i| i.id == format!("CMT-{:03}", item.no))?;
        let node = host::threads(&frozen.data).find(|t| t["id"] == offered.key)?;
        Some(AssessmentSource::Inline {
            thread_id: offered.key.clone(),
            version: handin::hash(serde_json::to_vec(node).ok()?),
        })
    } else {
        let url = item.url.as_ref()?;
        let raw = frozen
            .data
            .reviews
            .iter()
            .chain(&frozen.data.comments)
            .find(|s| host::url(s) == url)?;
        Some(AssessmentSource::Body {
            source_kind: if item.kind == CommentItemKind::ReviewBody {
                CommentSourceKind::ReviewBody
            } else {
                CommentSourceKind::PrComment
            },
            url: url.clone(),
            version: handin::hash(host::body(raw)),
            quote: item.quote.clone().unwrap_or_else(|| item.body.clone()),
        })
    }
}

fn missing(frozen: &Frozen, run: &AssessmentMethodRun, rows: &mut Vec<AssessmentComparisonItem>) {
    for sent in &frozen.sent {
        let Some(item) = frozen.items.iter().find(|i| i.id == sent.item) else {
            continue;
        };
        let Some(source) = item_source(frozen, item) else {
            continue;
        };
        let key = comparison_key(&source, Some(item));
        if rows.iter().any(|r| r.comparison_key == key) {
            continue;
        }
        let mut missing = common(run, format!("C{}", item.no), key, source, Some(item));
        missing.status = ComparisonItemStatus::MissingRow;
        rows.push(missing);
    }
}

fn dedup_keys(rows: &mut Vec<AssessmentComparisonItem>, writes: &mut Vec<PrimaryPoint>) {
    let mut seen = HashSet::new();
    rows.retain(|r| seen.insert(r.comparison_key.clone()));
    seen.clear();
    writes.retain(|w| seen.insert(w.key.clone()));
}

fn carry(frozen: &Frozen, item: &ItemRecord, row: &ReportRow) -> Option<PrimaryCarry> {
    let standing = frozen.standing.get(&item.id.to_string())?;
    let contradicted = row.recorded_decision.as_ref().is_some_and(|d| {
        let authority = binding_decision(
            &frozen.data.manifest.maintainers,
            &frozen.data.manifest.delegates,
            &item.author,
            d,
        );
        let kind = match d.kind {
            RecordedDecisionKind::Decision(k) => k,
            RecordedDecisionKind::Deferred => ProposedDecisionKind::Todo,
            RecordedDecisionKind::Withdrawn => ProposedDecisionKind::Reject,
        };
        authority
            && (serde_json::to_value(kind).ok() != serde_json::to_value(standing.kind).ok()
                || match &standing.target {
                    Some(DecisionTarget::ExistingPr { number }) => d.target_pr != Some(*number),
                    Some(DecisionTarget::UpstreamIssue { url }) => &d.target_url != url,
                    _ => false,
                })
    });
    Some(PrimaryCarry {
        decision: standing.id,
        reason: if standing.changed {
            Some(ReaskReason::CodeChanged)
        } else if contradicted {
            Some(ReaskReason::BotEdited)
        } else {
            None
        },
        evidence: CarryEvidence {
            quote: row.source_quote.clone().or_else(|| {
                let id = row.thread_id.as_ref()?;
                let thread = host::threads(&frozen.data).find(|t| t["id"] == *id)?;
                thread
                    .pointer("/comments/nodes/0/body")
                    .or_else(|| thread.pointer("/comments/0/body"))?
                    .as_str()
                    .map(|body| body.chars().take(600).collect())
            }),
            url: Some(row.comment_url.clone()),
            body_hash_before: Some(item.body_hash.clone()),
            body_hash_after: Some(item.body_hash.clone()),
            commit: None,
            lines: item.location.as_ref().and_then(|l| l.lines),
        },
    })
}

fn binding_decision(
    maintainers: &[String],
    delegates: &[String],
    author: &str,
    d: &RecordedDecision,
) -> bool {
    maintainers
        .iter()
        .any(|who| who.eq_ignore_ascii_case(&d.by))
        || (delegates.iter().any(|who| who.eq_ignore_ascii_case(&d.by))
            && d.on_behalf_of.as_ref().is_some_and(|named| {
                maintainers
                    .iter()
                    .any(|who| who.eq_ignore_ascii_case(named))
            }))
        || (d.kind == RecordedDecisionKind::Withdrawn && d.by.eq_ignore_ascii_case(author))
}

fn parse_location(value: &str) -> Option<ItemLocation> {
    let (path, lines) = value.rsplit_once(':')?;
    if !analysis::blob::is_tree_path(path) {
        return None;
    }
    let (start, end) = lines.split_once('-').unwrap_or((lines, lines));
    let start_line = start.parse::<u32>().ok()?;
    let end_line = end.parse::<u32>().ok()?;
    (start_line > 0 && end_line >= start_line).then(|| ItemLocation {
        path: path.into(),
        lines: Some(contracts::common::LineRange {
            start_line,
            end_line,
        }),
    })
}

fn local_pr(proposal: &ProposedDecision, repo: &str) -> Option<u32> {
    match &proposal.target_url {
        None => proposal.target_pr,
        Some(raw) => {
            let url = url::Url::parse(raw).ok()?;
            let parts: Vec<_> = url.path_segments()?.collect();
            if parts.len() != 4
                || parts[2] != "pull"
                || !format!("{}/{}", parts[0], parts[1]).eq_ignore_ascii_case(repo)
            {
                return None;
            }
            let number = parts[3].parse::<u32>().ok().filter(|n| *n > 0)?;
            proposal
                .target_pr
                .is_none_or(|n| n == number)
                .then_some(number)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_external_targets_are_preserved_without_retargeting_the_local_pr() {
        let proposal = ProposedDecision {
            kind: ProposedDecisionKind::HandledInPr,
            target_pr: None,
            target_url: Some("https://github.com/other/repo/pull/17".into()),
            fixed_in: None,
            title: None,
        };
        assert_eq!(local_pr(&proposal, "acme/r"), None);
        assert_eq!(local_pr(&proposal, "other/repo"), Some(17));
    }
    #[test]
    fn delegate_claim_requires_a_named_maintainer_to_bind_disposition() {
        let mut d = RecordedDecision {
            kind: RecordedDecisionKind::Decision(ProposedDecisionKind::Reject),
            by: "relay".into(),
            on_behalf_of: None,
            comment_url: "https://github.com/o/r/pull/1#issuecomment-2".into(),
            quote: "Reject this request".into(),
            target_pr: None,
            target_url: None,
        };
        let maintainers = vec!["owner".into()];
        let delegates = vec!["relay".into()];
        assert!(!binding_decision(&maintainers, &delegates, "reviewer", &d));
        d.on_behalf_of = Some("owner".into());
        assert!(binding_decision(&maintainers, &delegates, "reviewer", &d));
        d.on_behalf_of = Some("stranger".into());
        assert!(!binding_decision(&maintainers, &delegates, "reviewer", &d));
        d.by = "owner".into();
        d.on_behalf_of = None;
        assert!(binding_decision(&maintainers, &delegates, "reviewer", &d));
    }
}
