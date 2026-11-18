//! Reconstruct Inspector connections from the graph without asking the model to repeat them.

use std::collections::BTreeMap;

use contracts::agent::analyse_flow::{AgentEdge, AgentStep, FlowAnalysisOutput};

/// Adjacent transitions for a flow without an explicit graph.
pub(super) fn reading_order(steps: &[AgentStep]) -> Vec<AgentEdge> {
    steps
        .windows(2)
        .map(|pair| AgentEdge {
            from_step_id: pair[0].id.clone(),
            to_step_id: pair[1].id.clone(),
            label: None,
        })
        .collect()
}

/// Inspector prose using displayed step numbers and edge labels.
pub(super) fn descriptions(document: &FlowAnalysisOutput) -> BTreeMap<String, (String, String)> {
    let positions: BTreeMap<_, _> = document
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.id.as_str(), index + 1))
        .collect();
    let mut texts: BTreeMap<_, _> = document
        .steps
        .iter()
        .map(|step| (step.id.clone(), (String::new(), String::new())))
        .collect();
    for edge in &document.edges {
        let description = |id: &str| {
            let target = positions
                .get(id)
                .map_or_else(|| id.to_owned(), |position| format!("Step {position}"));
            match edge.label.as_deref().filter(|label| !label.is_empty()) {
                Some(label) => format!("{target} ({label})"),
                None => target,
            }
        };
        for (id, incoming, other) in [
            (&edge.to_step_id, true, &edge.from_step_id),
            (&edge.from_step_id, false, &edge.to_step_id),
        ] {
            if let Some((called_by, next)) = texts.get_mut(id) {
                let text = if incoming { called_by } else { next };
                if !text.is_empty() {
                    text.push_str("; ");
                }
                text.push_str(&description(other));
            }
        }
    }
    texts
}

/// Fill omitted edges and prefix connection prose with the derived step targets.
pub(super) fn fill(document: &mut FlowAnalysisOutput) {
    if document.edges.is_empty() {
        document.edges = reading_order(&document.steps);
    }
    let mut texts = descriptions(document);
    for step in &mut document.steps {
        if let Some((called_by, next)) = texts.remove(&step.id) {
            for (text, derived) in [(&mut step.called_by, called_by), (&mut step.next, next)] {
                if text.trim().is_empty() || text.trim() == derived {
                    *text = derived;
                } else if !derived.is_empty() {
                    *text = format!("{derived}; {text}");
                }
            }
        }
    }
}

/// The model-written part of stored connection prose: `stored` without its derived prefix.
pub(super) fn addition<'a>(stored: &'a str, derived: &str) -> &'a str {
    if derived.is_empty() {
        return stored;
    }
    match stored.strip_prefix(derived) {
        Some("") => "",
        Some(rest) => rest.strip_prefix("; ").unwrap_or(stored),
        None => stored,
    }
}
