//! Deterministic host-report fixture; no skill execution or provider requests.
use contracts::agent::pr_data::PrData;
use serde_json::{Value, json};

pub(super) fn report(data: &PrData) -> Value {
    let rows: Vec<_> = data.scope.threads.iter().enumerate().map(|(index, thread)| {
        let id = data.ids.items.iter().find(|item| item.key == *thread)
            .map_or_else(|| format!("CMT-{:03}", data.ids.next_id + index as u32), |item| item.id.clone());
        json!({"id":id,"source_kind":"inline","thread_id":thread,"reviewer":"fake",
            "comment_url":"","description":"Fake review point","reasoning":"Fake assessment for tests.",
            "assessment":"valid","proposed_action":"Fix here","proposed_decision":{"kind":"fix_here"}})
    }).collect();
    json!({"schema_version":"4.2.0","metadata":{"report_type":"comment_check",
        "input_mode":"host_supplied","pr_number":data.manifest.pr,"commit":data.manifest.head},
        "comment_assessments":rows})
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
}
