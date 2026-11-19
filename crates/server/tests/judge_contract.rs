//! The `judge_work` contract and the scorer agree on what a judgement may answer (plan §B.2, ADR-017).

use contracts::agent::judge_work::part_values;
use contracts::api::dashboard::ScoreComponent;
use server::dashboard::score::allowed_points;

#[test]
fn judge_part_values_match_the_scorers_closed_sets() {
    for component in ScoreComponent::ALL {
        match component {
            ScoreComponent::DefectSeverity => assert_eq!(
                allowed_points(component),
                Some(&(0..=20).collect::<Vec<i32>>()[..]),
                "defect severity = likelihood + impact, each 0–10"
            ),
            _ => assert_eq!(
                part_values(component),
                allowed_points(component),
                "{component:?}"
            ),
        }
    }
}
