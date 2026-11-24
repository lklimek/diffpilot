//! Deterministic host-report fixture; no skill execution or provider requests.
use contracts::agent::pr_data::PrData;
use serde_json::{Value, json};

pub(super) fn report(data: &PrData) -> Value {
    let mut next_id = data.ids.next_id;
    let mut allocate_id = |key: &str| {
        data.ids
            .items
            .iter()
            .find(|item| item.key == key)
            .map_or_else(
                || {
                    let id = format!("CMT-{next_id:03}");
                    next_id += 1;
                    id
                },
                |item| item.id.clone(),
            )
    };
    let threads = data
        .threads
        .pointer("/data/repository/pullRequest/reviewThreads/nodes")
        .or_else(|| data.threads.get("threads"))
        .unwrap_or(&data.threads);
    let mut rows: Vec<_> = data.scope.threads.iter().map(|thread| {
        let id = allocate_id(thread);
        let comment = threads.as_array().and_then(|threads| threads.iter().find(|t| t["id"] == *thread))
            .and_then(|t| t.pointer("/comments/nodes/0").or_else(|| t.pointer("/comments/0")));
        json!({"id":id,"source_kind":"inline","thread_id":thread,
            "reviewer":comment.map(author).unwrap_or("fake"),
            "comment_url":comment.map(url).unwrap_or_default(),
            "description":"Fake review point","reasoning":"Fake assessment for tests.",
            "assessment":"valid","proposed_action":"Fix here","proposed_decision":{"kind":"fix_here"}})
    }).collect();
    let mut canonical = None;
    for (kind, sources, prefix) in [
        ("review_body", &data.reviews, "review"),
        ("pr_comment", &data.comments, "comment"),
    ] {
        if let Some(source) = sources.iter().find(|s| {
            data.scope.sources.iter().any(|u| u == url(s))
                && s["body"].as_str().is_some_and(|b| !b.is_empty())
        }) {
            let source_id = source["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| source["id"].to_string());
            let key = format!("{prefix}:{source_id}:1");
            let id = allocate_id(&key);
            let quote: String = source["body"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(600)
                .collect();
            let mut row = json!({"id":id,"source_kind":kind,"reviewer":author(source),
                "comment_url":url(source),"source_quote":quote,
                "description":"Fake source point","reasoning":"Fake assessment for tests.",
                "proposed_action":"Fix here","proposed_decision":{"kind":"fix_here"}});
            if let Some(target) = &canonical {
                row["merged_into"] = json!(target);
            } else {
                row["assessment"] = json!("valid");
                canonical = Some(id);
            }
            rows.push(row);
        }
    }
    json!({"schema_version":"4.2.0","metadata":{"report_type":"comment_check",
        "input_mode":"host_supplied","pr_number":data.manifest.pr,"commit":data.manifest.head},
        "comment_assessments":rows})
}

fn url(value: &Value) -> &str {
    value
        .get("html_url")
        .or_else(|| value.get("url"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn author(value: &Value) -> &str {
    value
        .get("user")
        .or_else(|| value.get("author"))
        .and_then(|a| {
            a.get("login")
                .and_then(Value::as_str)
                .or_else(|| a.as_str())
        })
        .unwrap_or("ghost")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_fixture_retains_frozen_head_scope_and_existing_ids() {
        let data: PrData = serde_json::from_value(json!({
            "manifest":{"repo":"o/r","pr":12,"head":"abc","base_sha":"def","base_ref":"main",
                "title":"Fixture","body":"","pr_author":"alice","maintainers":[],"bots":[]},
            "threads":[],"review_comments":[],"reviews":[],"comments":[],"conventions":{},
            "ids":{"next_id":10,"items":[{"id":"CMT-002","key":"T2"}]},
            "scope":{"threads":["T2"],"sources":[]}
        }))
        .unwrap();
        let output = report(&data);
        let report: contracts::agent::comment_report::CommentReport =
            serde_json::from_value(output.clone()).unwrap();
        assert!(report.supported());
        assert_eq!(report.metadata.commit, "abc");
        assert_eq!(output["comment_assessments"][0]["id"], "CMT-002");
        assert_eq!(output["comment_assessments"][0]["thread_id"], "T2");
        assert_eq!(report.comment_assessments.len(), 1);
    }
    #[test]
    fn host_fixture_covers_grounded_body_sources_and_direct_duplicates() {
        let data: PrData = serde_json::from_value(json!({
            "manifest":{"repo":"o/r","pr":12,"head":"abc","base_sha":"def","base_ref":"main",
                "title":"Fixture","body":"","pr_author":"alice","maintainers":[],"bots":[]},
            "threads":[],"review_comments":[],
            "reviews":[{"id":41,"url":"review-url","body":"Bound the retry loop.","author":{"login":"bob"}}],
            "comments":[{"url":"comment-url","body":"Please add a regression test.","author":{"login":"bob"}},
                {"url":"out-of-scope","body":"Not in this pass.","author":{"login":"bob"}}],
            "conventions":{}, "ids":{"next_id":10,"items":[{"id":"CMT-002","key":"review:41:1"}]},
            "scope":{"threads":["T1"],"sources":["review-url","comment-url"]}
        })).unwrap();
        let output = report(&data);
        let rows = output["comment_assessments"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        let review = rows
            .iter()
            .find(|r| r["source_kind"] == "review_body")
            .unwrap();
        let comment = rows
            .iter()
            .find(|r| r["source_kind"] == "pr_comment")
            .unwrap();
        assert_eq!(review["id"], "CMT-002");
        assert_eq!(review["source_quote"], "Bound the retry loop.");
        assert_eq!(comment["source_quote"], "Please add a regression test.");
        assert_eq!(comment["merged_into"], review["id"]);
        assert!(comment.get("assessment").is_none());
        assert_eq!(comment["comment_url"], "comment-url");
        let ids: std::collections::HashSet<_> =
            rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), rows.len());
    }
}
