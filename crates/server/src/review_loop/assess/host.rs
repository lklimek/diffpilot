//! Grounded, scope-bound read-side adapter for claudius host reports.

use contracts::agent::comment_report::{
    CommentReport, ProposedDecision, ProposedDecisionKind, ReportRow, SourceKind,
};
use contracts::agent::pr_data::PrData;
use contracts::job::result::JobFailureKind;
use serde_json::Value;

/// Version of prompts/check_pr_comments.md, independent of the upstream report schema.
pub(super) const PROMPT_VERSION: u32 = 3;
use std::collections::HashSet;

pub(super) struct Decoded {
    pub head: String,
    pub rows: Vec<ReportRow>,
    pub dropped: usize,
}

/// Validate the envelope separately, then drop only unfit rows (M1–M5, M13).
pub(super) fn decode(data: &PrData, value: Value) -> Result<Decoded, JobFailureKind> {
    let report: CommentReport =
        serde_json::from_value(value).map_err(|_| JobFailureKind::ReportVersionUnsupported)?;
    if !report.supported()
        || report.metadata.pr_number != data.manifest.pr
        || report.metadata.commit.trim().is_empty()
    {
        return Err(JobFailureKind::ReportVersionUnsupported);
    }
    let total = report.comment_assessments.len();
    let mut seen = HashSet::new();
    let mut rows: Vec<ReportRow> = report
        .comment_assessments
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .filter(|row: &ReportRow| valid_row(data, row))
        .filter(|row| seen.insert(row.id.clone()))
        .collect();
    let canonical: HashSet<_> = rows
        .iter()
        .filter(|r| r.merged_into.is_none())
        .map(|r| r.id.clone())
        .collect();
    rows.retain(|r| {
        r.merged_into
            .as_ref()
            .is_none_or(|id| canonical.contains(id))
    });
    for row in &mut rows {
        row.merged_thread_ids
            .retain(|id| data.scope.threads.contains(id));
        row.merged_urls
            .retain(|url| data.scope.sources.contains(url));
        if row
            .recorded_decision
            .as_ref()
            .is_some_and(|recorded| !all_comments(data).any(|comment| grounded(comment, recorded)))
        {
            row.recorded_decision = None;
        }
    }
    Ok(Decoded {
        head: report.metadata.commit,
        dropped: total - rows.len(),
        rows,
    })
}

fn valid_row(data: &PrData, row: &ReportRow) -> bool {
    let Some(digits) = row.id.strip_prefix("CMT-") else {
        return false;
    };
    if digits.len() < 3
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || digits.parse::<u32>().ok().filter(|n| *n > 0).is_none()
        || row.description.trim().is_empty()
        || row.description.chars().count() > 1500
        || row.reasoning.trim().is_empty()
        || row.reasoning.chars().count() > 1500
    {
        return false;
    }
    let assessed = matches!(
        row.source_kind,
        SourceKind::Inline | SourceKind::ReviewBody | SourceKind::PrComment
    );
    if assessed
        && (row
            .proposed_decision
            .as_ref()
            .is_none_or(|d| !valid_proposal(d))
            || (row.merged_into.is_none() && row.assessment.is_none()))
    {
        return false;
    }
    if let Some(offered) = data.ids.items.iter().find(|item| item.id == row.id) {
        let kind = offered
            .source_kind
            .unwrap_or_else(|| kind_of_key(&offered.key));
        if kind != row.source_kind {
            return false;
        }
    }
    match row.source_kind {
        SourceKind::Inline => row.thread_id.as_ref().is_some_and(|id| {
            data.scope.threads.contains(id)
                && threads(data).any(|t| {
                    t["id"] == *id
                        && t["isResolved"] == false
                        && comments(t).next().is_some_and(|c| {
                            author(c).eq_ignore_ascii_case(&row.reviewer)
                                && url(c) == row.comment_url
                        })
                })
        }),
        SourceKind::ReviewBody | SourceKind::PrComment => {
            let files = if row.source_kind == SourceKind::ReviewBody {
                &data.reviews
            } else {
                &data.comments
            };
            data.scope.sources.contains(&row.comment_url)
                && files.iter().any(|source| {
                    url(source) == row.comment_url
                        && author(source).eq_ignore_ascii_case(&row.reviewer)
                        && row
                            .source_quote
                            .as_ref()
                            .is_some_and(|q| distinct_quote(body(source), q))
                })
        }
        SourceKind::Reply | SourceKind::Status => true,
    }
}

pub(super) fn kind_of_key(key: &str) -> SourceKind {
    if key.starts_with("review:") {
        SourceKind::ReviewBody
    } else if key.starts_with("comment:") {
        SourceKind::PrComment
    } else {
        SourceKind::Inline
    }
}

pub(super) fn valid_proposal(proposal: &ProposedDecision) -> bool {
    let target = proposal.target_pr.is_some_and(|n| n > 0);
    let linked = proposal
        .target_url
        .as_ref()
        .is_some_and(|u| crate::github::threads::github_web_url(u).is_some());
    let titled = proposal
        .title
        .as_ref()
        .is_some_and(|t| !t.trim().is_empty() && t.chars().count() <= 600);
    if proposal.target_url.is_some() && !linked {
        return false;
    }
    match proposal.kind {
        ProposedDecisionKind::HandledInPr => target || linked,
        ProposedDecisionKind::FollowUpPr => target || titled,
        ProposedDecisionKind::UpstreamIssue => linked,
        _ => true,
    }
}

pub(super) fn grounded(
    comment: &Value,
    d: &contracts::agent::comment_report::RecordedDecision,
) -> bool {
    url(comment) == d.comment_url
        && author(comment).eq_ignore_ascii_case(&d.by)
        && (distinct_quote(body(comment), &d.quote)
            || comment["relayMarker"].as_str() == Some(d.quote.as_str()))
}

pub(super) fn distinct_quote(text: &str, quote: &str) -> bool {
    let n = quote.chars().count();
    n > 0 && n <= 600 && (n >= 12 || quote == text) && text.match_indices(quote).count() == 1
}

pub(super) fn url(value: &Value) -> &str {
    value
        .get("html_url")
        .or_else(|| value.get("url"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

pub(super) fn author(value: &Value) -> &str {
    let author = value.get("user").or_else(|| value.get("author"));
    author
        .and_then(|a| {
            a.get("login")
                .and_then(Value::as_str)
                .or_else(|| a.as_str())
        })
        .unwrap_or("ghost")
}

pub(super) fn body(value: &Value) -> &str {
    value["body"].as_str().unwrap_or_default()
}

pub(super) fn threads(data: &PrData) -> impl Iterator<Item = &Value> {
    data.threads
        .pointer("/data/repository/pullRequest/reviewThreads/nodes")
        .or_else(|| data.threads.get("threads"))
        .unwrap_or(&data.threads)
        .as_array()
        .into_iter()
        .flatten()
}

pub(super) fn comments(thread: &Value) -> impl Iterator<Item = &Value> {
    thread["comments"]
        .get("nodes")
        .unwrap_or(&thread["comments"])
        .as_array()
        .into_iter()
        .flatten()
}

pub(super) fn all_comments(data: &PrData) -> impl Iterator<Item = &Value> {
    threads(data)
        .flat_map(comments)
        .chain(&data.comments)
        .chain(&data.reviews)
        .chain(&data.review_comments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use contracts::agent::pr_data::PrData;
    use serde_json::{Value, json};

    fn data() -> PrData {
        serde_json::from_value(json!({
            "manifest":{"repo":"acme/r", "pr":12, "head":"abc", "base_ref":"main", "base_sha":"def", "title":"Retry", "body":"Bound retries", "pr_author":"alice", "maintainers":["owner"], "bots":["bot"], "delegates":["relay"]},
            "threads":{"threads":[{"id":"T1", "isResolved":false, "comments":[{"author":{"login":"bot"},"body":"Bound the retry loop.","url":"https://github.com/acme/r/pull/12#discussion_r1"}]}]},
            "review_comments":[], "reviews":[], "comments":[{"id":42,"url":"https://github.com/acme/r/pull/12#issuecomment-42", "author":{"login":"owner"}, "body":"Bound the retry loop. Release the lock before waiting."}],
            "conventions":{}, "ids":{"next_id":2,"items":[]},
            "scope":{"threads":["T1"], "sources":["https://github.com/acme/r/pull/12#issuecomment-42"]}
        })).unwrap()
    }

    fn row(id: &str) -> Value {
        json!({"id":id,"reviewer":"owner", "source_kind":"pr_comment", "comment_url":"https://github.com/acme/r/pull/12#issuecomment-42", "source_quote":"Bound the retry loop.", "description":"Bound retries", "reasoning":"The retry loop is unbounded.", "assessment":"valid", "proposed_action":"Fix here", "proposed_decision":{"kind":"fix_here"}})
    }

    fn report(rows: Vec<Value>) -> Value {
        json!({"schema_version":"4.2.0", "metadata":{"report_type":"comment_check","input_mode":"host_supplied","pr_number":12,"commit":"abc"}, "comment_assessments":rows})
    }

    #[test]
    fn host_drops_bad_rows_but_rejects_bad_envelopes() {
        let mut bad = row("CMT-003");
        bad["source_quote"] = json!("invented quotation");
        let accepted = decode(&data(), report(vec![row("CMT-002"), bad, json!({})])).unwrap();
        assert_eq!(accepted.rows.len(), 1);
        assert_eq!(accepted.dropped, 2);
        let mut old = report(vec![]);
        old["schema_version"] = json!("4.1.0");
        assert!(decode(&data(), old).is_err());
        let mut wrong_pr = report(vec![]);
        wrong_pr["metadata"]["pr_number"] = json!(99);
        assert!(decode(&data(), wrong_pr).is_err());
    }

    #[test]
    fn host_keeps_explicit_cross_source_duplicates_and_rejects_cycles() {
        let mut inline = row("CMT-002");
        inline["source_kind"] = json!("inline");
        inline["thread_id"] = json!("T1");
        inline["reviewer"] = json!("bot");
        inline["comment_url"] = json!("https://github.com/acme/r/pull/12#discussion_r1");
        let mut duplicate = row("CMT-003");
        duplicate["merged_into"] = json!("CMT-002");
        duplicate.as_object_mut().unwrap().remove("assessment");
        let accepted = decode(&data(), report(vec![duplicate.clone(), inline.clone()])).unwrap();
        assert_eq!(accepted.rows.len(), 2);
        inline["merged_into"] = json!("CMT-003");
        assert!(
            decode(&data(), report(vec![duplicate, inline]))
                .unwrap()
                .rows
                .is_empty()
        );
    }

    #[test]
    fn host_checks_merged_threads_and_urls_independently() {
        let mut point = row("CMT-002");
        point["merged_thread_ids"] = json!(["outside"]);
        point["merged_urls"] = json!(["https://github.com/acme/r/pull/12#issuecomment-999"]);
        let accepted = decode(&data(), report(vec![point])).unwrap();
        assert_eq!(accepted.rows.len(), 1);
        assert!(accepted.rows[0].merged_thread_ids.is_empty());
        assert!(accepted.rows[0].merged_urls.is_empty());
    }

    #[test]
    fn host_preserves_stale_head_and_rejects_ungrounded_author_and_targets() {
        let mut stale = report(vec![row("CMT-002")]);
        stale["metadata"]["commit"] = json!("older");
        assert_eq!(decode(&data(), stale).unwrap().head, "older");
        let mut forged = row("CMT-002");
        forged["reviewer"] = json!("other");
        let mut target = row("CMT-003");
        target["proposed_decision"] = json!({"kind":"handled_in_pr"});
        assert!(
            decode(&data(), report(vec![forged, target]))
                .unwrap()
                .rows
                .is_empty()
        );
    }

    #[test]
    fn host_recorded_decisions_require_grounded_author_and_quote() {
        let mut input = data();
        input.comments.push(json!({"author":{"login":"relay"},"url":"https://github.com/acme/r/pull/12#issuecomment-50","body":"Per owner decision, defer this request."}));
        let mut point = row("CMT-002");
        point["recorded_decision"] = json!({"kind":"deferred","by":"relay","on_behalf_of":"owner","comment_url":"https://github.com/acme/r/pull/12#issuecomment-50","quote":"Per owner decision, defer this request."});
        let accepted = decode(&input, report(vec![point.clone()])).unwrap();
        assert_eq!(
            accepted.rows[0].recorded_decision.as_ref().unwrap().by,
            "relay"
        );
        assert_eq!(
            accepted.rows[0]
                .recorded_decision
                .as_ref()
                .unwrap()
                .on_behalf_of
                .as_deref(),
            Some("owner")
        );
        point["recorded_decision"]["by"] = json!("forged");
        assert!(
            decode(&input, report(vec![point])).unwrap().rows[0]
                .recorded_decision
                .is_none()
        );
    }
}
