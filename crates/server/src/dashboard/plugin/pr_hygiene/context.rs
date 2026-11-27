//! Plain, readable PR Hygiene context; optional fields of an unfamiliar shape are left out.

use std::collections::{BTreeMap, HashSet};

use contracts::api::dashboard::Fact;
use contracts::time::Timestamp;
use serde_json::Value;

use super::fact;

pub(super) fn readable(value: &str) -> String {
    match value {
        "bots" => "Bot reviews",
        "self-review" => "Self review",
        "ci" => "Continuous integration",
        "review" => "Human review",
        "mergeable" => "Ready to merge",
        "not-governed" => "Outside governance",
        "waiting-bots" => "Waiting for bot reviews",
        "waiting-author" => "Waiting for author",
        "waiting-self-review" => "Waiting for self review",
        "waiting-build" => "Waiting for build",
        "ready-for-human" => "Ready for human review",
        "too-many-open-prs" => "Too many open pull requests",
        other => return other.replace(['-', '_'], " "),
    }
    .to_owned()
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

fn entries<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn names(value: &Value, key: &str) -> Option<String> {
    let names = value
        .get(key)?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    Some(if names.is_empty() {
        "None".to_owned()
    } else {
        names.join(", ")
    })
}

pub(super) fn timing(value: &Value, as_of: Timestamp) -> Vec<Fact> {
    let mut facts = Vec::new();
    if let Some(since) = text(value, "since") {
        let since = match since.parse::<Timestamp>() {
            Ok(at) if at <= as_of => {
                let minutes = as_of.duration_since(at).as_secs() / 60;
                format!("{since} ({}h {}m at data time)", minutes / 60, minutes % 60)
            }
            _ => since.to_owned(),
        };
        facts.push(fact("Since", &since));
    }
    if let Some(basis) = text(value, "since_basis") {
        facts.push(fact(
            "Since basis",
            match basis {
                "engine" => "Engine",
                "opened" => "PR opened",
                other => other,
            },
        ));
    }
    facts
}

pub(super) fn actions(next: Option<&str>, value: &Value) -> Vec<String> {
    let flags = [
        ("merge_conflict", "Resolve the merge conflict"),
        ("changes_requested", "Address requested review changes"),
    ];
    let mut seen = HashSet::new();
    next.into_iter()
        .chain(entries(value, "blockers").filter_map(Value::as_str))
        .chain(flags.into_iter().filter_map(|(key, action)| {
            (value.get(key).and_then(Value::as_bool) == Some(true)).then_some(action)
        }))
        .filter(|action| !action.trim().is_empty() && seen.insert(*action))
        .map(str::to_owned)
        .collect()
}

pub(super) fn details(value: &Value) -> Vec<String> {
    let mut points = Vec::new();
    for blocker in entries(value, "blockers").filter_map(Value::as_str) {
        points.push(format!("Blocker: {blocker}"));
    }
    for (key, label) in [
        ("changes_requested", "Changes requested"),
        ("ci_failing", "CI failing"),
        ("merge_conflict", "Merge conflict"),
        ("idle", "Idle"),
        ("draft", "Draft"),
        ("tracked", "Tracked"),
    ] {
        if let Some(flag) = value.get(key).and_then(Value::as_bool) {
            points.push(format!("{label}: {}", if flag { "Yes" } else { "No" }));
        }
    }
    if let Some(count) = value.get("unresolved_comments").and_then(Value::as_u64) {
        points.push(format!("Unresolved comments: {count}"));
    }
    if let Some(author) = text(value, "author") {
        let kind = text(value, "author_kind")
            .map(|s| format!(" ({})", readable(s)))
            .unwrap_or_default();
        points.push(format!("Author: {author}{kind}"));
    }
    for (key, label) in [
        ("base", "Base"),
        ("milestone", "Milestone"),
        ("updated_at", "Updated"),
    ] {
        if let Some(field) = value.get(key) {
            if let Some(text) = field.as_str() {
                points.push(format!("{label}: {text}"));
            } else if field.is_null() {
                points.push(format!("{label}: None"));
            }
        }
    }
    for (key, label) in [("areas", "Areas"), ("objectors", "Objectors")] {
        if let Some(names) = names(value, key) {
            points.push(format!("{label}: {names}"));
        }
    }
    for ask in entries(value, "asks") {
        if let Some(approvers) = names(ask, "approvers") {
            let area = text(ask, "area").unwrap_or("unspecified area");
            points.push(format!("Review requested for {area}: {approvers}"));
        }
    }
    if value
        .get("asks")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        points.push("Review requests: None".to_owned());
    }
    // Timestamps, not array order, decide which review represents a reviewer now.
    let mut latest = BTreeMap::new();
    for review in entries(value, "reviews") {
        let (Some(reviewer), Some(state), Some(at)) = (
            text(review, "reviewer"),
            text(review, "state"),
            text(review, "at"),
        ) else {
            continue;
        };
        let Ok(time) = at.parse::<Timestamp>() else {
            continue;
        };
        let entry = latest.entry(reviewer).or_insert((time, state, at));
        if time > entry.0 {
            *entry = (time, state, at);
        }
    }
    for (reviewer, (_, state, at)) in latest {
        let state = match state {
            "approved" => "Approved".to_owned(),
            "changes-requested" => "Changes requested".to_owned(),
            "commented" => "Commented".to_owned(),
            "dismissed" => "Dismissed".to_owned(),
            other => readable(other),
        };
        points.push(format!("Latest review by {reviewer}: {state} at {at}"));
    }
    points
}

pub(super) fn history(changes: &[Value]) -> Vec<String> {
    let mut changes: Vec<_> = changes
        .iter()
        .filter_map(|change| {
            Some((
                text(change, "observed_at")?.parse::<Timestamp>().ok()?,
                change,
            ))
        })
        .collect();
    changes.sort_by_key(|(at, _)| *at);
    let mut points = Vec::new();
    if changes.len() > 5 {
        points.push(format!(
            "Recent stage changes: latest 5 of {}",
            changes.len()
        ));
    }
    for (_, change) in changes.iter().rev().take(5).rev() {
        let stage = readable(text(change, "stage").unwrap_or("unknown"));
        let engine = readable(text(change, "engine_state").unwrap_or("unknown"));
        let since = text(change, "since").unwrap_or("not recorded");
        let observed = text(change, "observed_at").unwrap_or("not recorded");
        points.push(format!(
            "Stage change: {stage}; {engine}; since {since}; observed {observed}"
        ));
    }
    if changes.is_empty() {
        points.push("Stage history: None recorded".to_owned());
    }
    points
}

#[cfg(test)]
mod tests {
    use super::actions;
    use serde_json::json;

    #[test]
    fn actionable_points_keep_source_order_and_true_flags_without_duplicates() {
        let status = json!({
            "blockers": ["Fix the test", "", "  ", "Find a maintainer", "Find a maintainer"],
            "merge_conflict": false,
            "changes_requested": true,
            "ci_failing": true
        });
        assert_eq!(
            actions(Some("Fix the test"), &status),
            [
                "Fix the test",
                "Find a maintainer",
                "Address requested review changes"
            ]
        );
        assert_eq!(actions(None, &status)[0], "Fix the test");
        assert_eq!(
            actions(None, &json!({"merge_conflict": true})),
            ["Resolve the merge conflict"]
        );
        assert!(
            actions(
                Some(" "),
                &json!({"merge_conflict": "true", "changes_requested": false})
            )
            .is_empty()
        );
    }
}
