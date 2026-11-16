//! Tour ordering of flows (brief §4.3): behaviour flows by importance, then LLM-classified refactors, then
//! LLM-classified tests. Quiet rows always follow, Unexplained first among them (fixed by the API shape).

use contracts::api::tour::FlowGroup;
use contracts::ids::FlowId;

/// What ordering needs from a flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderInput {
    /// The flow.
    pub id: FlowId,
    /// Its group (from the classification).
    pub group: FlowGroup,
    /// Plan importance rank, 1 = read first.
    pub importance_rank: u32,
}

/// Flow ids in tour order: by group, then importance rank, then id.
pub fn tour_order(flows: &[OrderInput]) -> Vec<FlowId> {
    let mut sorted: Vec<&OrderInput> = flows.iter().collect();
    sorted.sort_by(|a, b| {
        (a.group, a.importance_rank, &a.id).cmp(&(b.group, b.importance_rank, &b.id))
    });
    sorted.into_iter().map(|flow| flow.id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str, group: FlowGroup, rank: u32) -> OrderInput {
        OrderInput {
            id: FlowId(id.to_owned()),
            group,
            importance_rank: rank,
        }
    }

    #[test]
    fn behaviour_by_importance_then_refactors_then_tests() {
        let flows = [
            input("f1", FlowGroup::LlmClassifiedTests, 1),
            input("f2", FlowGroup::Behaviour, 3),
            input("f3", FlowGroup::LlmClassifiedRefactor, 2),
            input("f4", FlowGroup::Behaviour, 1),
            input("f5", FlowGroup::LlmClassifiedRefactor, 1),
        ];
        let order: Vec<String> = tour_order(&flows).into_iter().map(|id| id.0).collect();
        assert_eq!(order, ["f4", "f2", "f5", "f3", "f1"]);
    }

    #[test]
    fn empty_input_gives_empty_order() {
        assert!(tour_order(&[]).is_empty());
    }
}
