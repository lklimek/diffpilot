//! Golden: ~160 synthetic items (`testdata/make_golden.py`, seeded; invented repositories, people and values chosen
//! to hit every rule branch and bucket edge) ranked as the daily-priorities skill's own Python reference ranks them.
//! Expected rows come from the skill's `scoring.py`, `todos.py` and `score.py` merge, so the golden checks the port
//! independently. Settings = the skill's defaults (no skipped bot authors); the generator applies the one deviation,
//! D19 (the ownership policy keeps pull requests directed at the user).

use std::collections::HashMap;

use contracts::api::dashboard::{DashboardSettings, ItemKey, ScoreComponent, Tier};
use contracts::time::Timestamp;
use serde::Deserialize;

use super::*;

/// The judged points per component (null = not judged), an eligibility flip, merges.
#[derive(Deserialize)]
struct FixtureJudgement {
    p: [Option<i32>; 12],
    e: Option<bool>,
    #[serde(default)]
    a: Vec<ItemKey>,
}

#[derive(Deserialize)]
struct Expected {
    id: String,
    tier: Tier,
    score: f64,
    tenths: [i32; 12],
}

#[derive(Deserialize)]
struct Fixture {
    now: Timestamp,
    own_login: String,
    settings: DashboardSettings,
    prs: Vec<PrFacts>,
    issues: Vec<IssueFacts>,
    todos: Vec<TodoFacts>,
    judgements: HashMap<ItemKey, FixtureJudgement>,
    ranked: Vec<Expected>,
    excluded: Vec<String>,
}

impl FixtureJudgement {
    fn judgement(&self, now: Timestamp) -> Judgement {
        Judgement {
            parts: ScoreComponent::ALL
                .iter()
                .zip(self.p)
                .filter_map(|(component, points)| {
                    points.map(|points| JudgedPart {
                        component: *component,
                        points,
                        basis: "synthetic".into(),
                    })
                })
                .collect(),
            eligibility: self.e.map(|eligible| EligibilityJudgement {
                eligible,
                reason: "synthetic".into(),
            }),
            alarm: false,
            also: self.a.clone(),
            judged_at: now,
            freshness: Freshness::Current,
        }
    }
}

#[test]
fn golden_synthetic_run_matches_skill_reference() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("testdata/golden-run.json")).expect("fixture parses");
    let now = fixture.now;
    let rules = ScoreRules::new(&fixture.own_login, &fixture.settings);
    let judgements: HashMap<ItemKey, Judgement> = fixture
        .judgements
        .iter()
        .map(|(key, j)| (key.clone(), j.judgement(now)))
        .collect();
    let find = |key: &ItemKey| judgements.get(key);
    let pr_key = |pr: &PrFacts| {
        ItemKey::new(&ItemTarget::PullRequest {
            repo: pr.repo.clone(),
            number: pr.number,
        })
    };
    let issue_key = |i: &IssueFacts| {
        ItemKey::new(&ItemTarget::Issue {
            repo: i.repo.clone(),
            number: i.number,
        })
    };
    let todo_ids = todo_display_ids(&fixture.todos.iter().map(|t| t.id).collect::<Vec<_>>());

    let mut items = Vec::new();
    for pr in &fixture.prs {
        let judgement = find(&pr_key(pr).expect("key"));
        items.push(score_pr(&rules, pr, judgement, now).expect("synthetic judgement fits"));
    }
    for issue in &fixture.issues {
        let judgement = find(&issue_key(issue).expect("key"));
        items.push(score_issue(&rules, issue, judgement, now).expect("synthetic judgement fits"));
    }
    for todo in &fixture.todos {
        let key = ItemKey::new(&ItemTarget::Todo(todo.id)).expect("key");
        let item = score_todo(todo, todo_ids[&todo.id].clone(), find(&key), now);
        items.push(item.expect("synthetic judgement fits"));
    }
    let ranking = rank(items);

    let excluded: Vec<&str> = ranking.excluded.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(excluded, fixture.excluded);
    let got: Vec<&str> = ranking.ranked.iter().map(|i| i.id.as_str()).collect();
    let want: Vec<&str> = fixture.ranked.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(got, want, "rank order");
    for (item, expected) in ranking.ranked.iter().zip(&fixture.ranked) {
        assert_eq!(item.score.tier, expected.tier, "{}", expected.id);
        assert!(
            (item.points - expected.score).abs() < 1e-6,
            "{}: {} vs {}",
            expected.id,
            item.points,
            expected.score
        );
        let tenths: Vec<i32> = item
            .score
            .parts
            .iter()
            .map(|p| p.points_tenths.unwrap_or(i32::MIN))
            .collect();
        for ((got, want), component) in tenths.iter().zip(expected.tenths).zip(ScoreComponent::ALL)
        {
            assert!(
                (got - want).abs() <= 1,
                "{} {component:?}: {got} vs {want}",
                expected.id
            );
        }
    }
}
