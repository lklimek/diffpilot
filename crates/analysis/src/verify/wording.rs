//! Wording rules of brief §1 over LLM text: banned phrases and percentages. Hits are warnings, never rejections
//! (plan §12.4). The patterns mirror `scripts/check-wording.mjs` (UI text); a test keeps both in sync.

use std::sync::LazyLock;

use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::agent::ask::AskOutput;
use contracts::agent::plan::PlanOutput;
use contracts::agent::pr_context::PrContextOutput;
use regex::{Regex, RegexBuilder};

use super::WordingWarning;

/// Banned phrases, matched case-insensitively (`WORDS` of `scripts/check-wording.mjs`).
pub const BANNED_PHRASES: &str = r"\bno\s+risk|\bnothing\s+to\s+review\b|\bno\s+review\s+needed\b|\bnoise\b|\bverified\b|\bwas\s+wrong\b|\bconfidence\b";
/// Percentages (`PERCENT` of `scripts/check-wording.mjs`; `{}` stands for an interpolated value there).
pub const PERCENTAGE: &str = r"(?:\d+|\{\})\s?%";

static RULES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [BANNED_PHRASES, PERCENTAGE]
        .into_iter()
        .filter_map(|pattern| {
            RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .ok()
        })
        .collect()
});

/// Wording warnings in the texts of a `plan` output.
pub fn plan(output: &PlanOutput) -> Vec<WordingWarning> {
    let mut found = Vec::new();
    for (index, flow) in output.flows.iter().enumerate() {
        let at = format!("flows[{index}]");
        scan(&mut found, &format!("{at}.title"), &flow.title);
        scan(&mut found, &format!("{at}.intent"), &flow.intent);
    }
    for (index, hunk) in output.unassigned.iter().enumerate() {
        scan(
            &mut found,
            &format!("unassigned[{index}].reason"),
            &hunk.reason,
        );
    }
    found
}

/// Wording warnings in the texts of a `pr_context` output.
pub fn pr_context(output: &PrContextOutput) -> Vec<WordingWarning> {
    let mut found = Vec::new();
    scan(&mut found, "problem", &output.problem);
    scan(&mut found, "why_needed", &output.why_needed);
    scan(&mut found, "what_it_gives", &output.what_it_gives);
    for (index, work) in output.related_work.iter().enumerate() {
        scan(
            &mut found,
            &format!("related_work[{index}].overlap"),
            &work.overlap,
        );
    }
    for (index, question) in output.fit_questions.iter().enumerate() {
        let at = format!("fit_questions[{index}].question");
        scan(&mut found, &at, &question.question);
    }
    found
}

/// Wording warnings in the texts of an `analyse_flow` output.
pub fn flow(output: &FlowAnalysisOutput) -> Vec<WordingWarning> {
    let mut found = Vec::new();
    let intro = &output.introduction;
    scan(
        &mut found,
        "introduction.problem_before",
        &intro.problem_before,
    );
    scan(&mut found, "introduction.change", &intro.change);
    let example = &intro.example_before_after;
    scan(&mut found, "introduction.example_before_after", example);
    for (index, step) in output.steps.iter().enumerate() {
        let at = format!("steps[{index}]");
        for (field, text) in [
            ("title", &step.title),
            ("called_by", &step.called_by),
            ("decides", &step.decides),
            ("next", &step.next),
            ("why", &step.why),
        ] {
            scan(&mut found, &format!("{at}.{field}"), text);
        }
        for (number, note) in step.annotations.iter().enumerate() {
            let at = format!("{at}.annotations[{number}]");
            scan(&mut found, &format!("{at}.was"), &note.was);
            scan(&mut found, &format!("{at}.is"), &note.is);
            if let Some(text) = &note.text {
                scan(&mut found, &format!("{at}.text"), text);
            }
        }
        if let Some(assessment) = &step.assessment {
            let at = format!("{at}.assessment");
            for (field, text) in [
                ("headline", &assessment.headline),
                ("reason", &assessment.reason),
                ("basis_note", &assessment.basis_note),
                ("not_checked", &assessment.not_checked),
            ] {
                scan(&mut found, &format!("{at}.{field}"), text);
            }
            if let Some(text) = &assessment.smallest_correction {
                scan(&mut found, &format!("{at}.smallest_correction"), text);
            }
        }
    }
    for (index, edge) in output.edges.iter().enumerate() {
        if let Some(label) = &edge.label {
            scan(&mut found, &format!("edges[{index}].label"), label);
        }
    }
    for (index, impact) in output.impact_refs.iter().enumerate() {
        scan(
            &mut found,
            &format!("impact_refs[{index}].reason"),
            &impact.reason,
        );
    }
    for (index, pattern) in output.pattern_repetitions.iter().enumerate() {
        let at = format!("pattern_repetitions[{index}].description");
        scan(&mut found, &at, &pattern.description);
    }
    for (index, thread) in output.thread_statuses.iter().enumerate() {
        let at = format!("thread_statuses[{index}].reason");
        scan(&mut found, &at, &thread.reason);
    }
    found
}

/// Wording warnings in an `ask` answer.
pub fn ask(output: &AskOutput) -> Vec<WordingWarning> {
    let mut found = Vec::new();
    scan(&mut found, "answer", &output.answer);
    found
}

/// Adds a warning per rule hit in `text`; the phrase is the match with its whitespace collapsed.
fn scan(found: &mut Vec<WordingWarning>, location: &str, text: &str) {
    for rule in RULES.iter() {
        for hit in rule.find_iter(text) {
            found.push(WordingWarning {
                phrase: hit
                    .as_str()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                location: location.to_owned(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use contracts::agent::pr_context::FitQuestion;
    use contracts::common::FitTopic;

    use super::*;

    fn pr_context_with(problem: &str, question: &str) -> PrContextOutput {
        PrContextOutput {
            problem: problem.to_owned(),
            why_needed: "Timeouts fail payments.".to_owned(),
            what_it_gives: "Retries.".to_owned(),
            related_work: Vec::new(),
            fit_questions: vec![FitQuestion {
                question: question.to_owned(),
                topic: FitTopic::Other,
                evidence: Vec::new(),
            }],
        }
    }

    fn phrases(warnings: &[WordingWarning]) -> Vec<(&str, &str)> {
        warnings
            .iter()
            .map(|w| (w.phrase.as_str(), w.location.as_str()))
            .collect()
    }

    #[test]
    fn banned_phrases_and_percentages_in_llm_text_are_warnings() {
        let output = pr_context_with(
            "No\n risk here; the change was Verified with 95 % confidence.",
            "Is this noise?",
        );
        assert_eq!(
            phrases(&pr_context(&output)),
            [
                ("No risk", "problem"),
                ("Verified", "problem"),
                ("confidence", "problem"),
                ("95 %", "problem"),
                ("noise", "fit_questions[0].question"),
            ]
        );
    }

    #[test]
    fn ask_answer_wording_is_scanned() {
        let output = AskOutput {
            answer: "This was verified; no risk at 90% here?".to_owned(),
            evidence: Vec::new(),
        };
        assert_eq!(
            phrases(&ask(&output)),
            [
                ("verified", "answer"),
                ("no risk", "answer"),
                ("90%", "answer")
            ]
        );
        let clean = AskOutput {
            answer: "Can the reply arrive twice?".to_owned(),
            evidence: Vec::new(),
        };
        assert!(ask(&clean).is_empty());
    }

    #[test]
    fn clean_text_and_word_parts_give_no_warning() {
        let output = pr_context_with(
            "Unverified noisy retries; risk is not assessed here. Version 1.2 keeps 3 retries.",
            "Can broadcast_tx be sent twice when the reply arrives late?",
        );
        assert!(pr_context(&output).is_empty());
    }

    /// Reads the regex literal `const NAME = /…/flags;` from `scripts/check-wording.mjs`.
    fn mjs_pattern(source: &str, name: &str, flags: &str) -> String {
        let start = format!("const {name} = /");
        let line = source
            .lines()
            .find(|line| line.starts_with(&start))
            .unwrap_or_else(|| panic!("{name} not found in check-wording.mjs"));
        line.strip_prefix(&start)
            .and_then(|rest| rest.strip_suffix(&format!("/{flags};")))
            .unwrap_or_else(|| panic!("{name} is not a /…/{flags} literal: {line}"))
            .to_owned()
    }

    #[test]
    fn wording_rules_match_check_wording_mjs() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/check-wording.mjs");
        let source = std::fs::read_to_string(script).expect("check-wording.mjs");
        assert_eq!(mjs_pattern(&source, "WORDS", "giu"), BANNED_PHRASES);
        assert_eq!(mjs_pattern(&source, "PERCENT", "gu"), PERCENTAGE);
        assert_eq!(RULES.len(), 2, "both patterns compile");
    }
}
