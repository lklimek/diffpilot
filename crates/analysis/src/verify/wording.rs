//! Wording rules of brief §1 over LLM text: banned phrases and percentages. Hits are warnings, never rejections
//! (plan §12.4). The patterns mirror `scripts/wording-rules.json` (UI text checks); a test keeps both in sync.

use std::sync::LazyLock;

use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::agent::ask::AskOutput;
use contracts::agent::plan::PlanOutput;
use contracts::agent::pr_context::PrContextOutput;
use regex::{Regex, RegexBuilder};

use super::WordingWarning;

/// Banned phrases, matched case-insensitively (`words` of `scripts/wording-rules.json`).
pub const BANNED_PHRASES: &str = r"\bno\s+risk|\bnothing\s+to\s+review\b|\bno\s+review\s+needed\b|\bnoise\b|\bverified\b|\bwas\s+wrong\b|\bconfidence\b";
/// Percentages (`percent` of `scripts/wording-rules.json`; `{}` stands for an interpolated value in sources).
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

/// Capability excuses are rejected, unlike advisory wording warnings.
pub(super) fn agent_limitation(text: &str) -> bool {
    static PATTERN: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(concat!(
            r"(?i)\bas an ai\b|",
            r"\b(?:severity skill|my (?:tools?|skills?|instructions?)|required (?:review )?(?:tools?|skills?|instructions?))\b[^.!?\n]{0,60}\b(?:unavailable|missing|not available|not accessible|not loaded|not provided|could not be loaded)\b|",
            r"\b(?:i|we) (?:cannot|could not|couldn't|am unable to|was unable to|were unable to)\s+(?:load|read|access|find|use)\b[^.!?\n]{0,60}\b(?:skills?|tools?|instructions?)\b|",
            r"\b(?:cannot|could not|unable to|failed to)\s+(?:load|read|access|find|use)\b[^.!?\n]{0,30}\b(?:severity skill|required (?:review )?(?:skills?|tools?|instructions?))\b|",
            r"\bjako (?:ai|sztuczna inteligencja|model językowy)\b|",
            r"\b(?:skill severity|narzędzia agenta|wymagane instrukcje)[^.!?\n]{0,60}\b(?:niedostępn\w*|brak\w*)|",
            r"\bnie mog(?:ę|łem|łam|liśmy|łyśmy) (?:wczytać|załadować|odczytać)[^.!?\n]{0,60}(?:skill|narzędzi|instrukcj)|",
            r"\bкак (?:ии|ai|искусственный интеллект|языковая модель)(?:[\s,.!?:;]|$)|",
            r"\b(?:навык severity|skill severity|инструменты агента|требуемые инструкции)[^.!?\n]{0,60}(?:недоступ\w*|отсутству\w*)|",
            r"(?:\bне могу|\b(?:я|мы) не (?:смог|смогла|смогли))\s+(?:загрузить|прочитать|открыть)[^.!?\n]{0,60}(?:навык|skill|инструменты агента|инструкции агента|требуемые (?:навыки|инструменты|инструкции))"
        )).ok()
    });
    PATTERN
        .as_ref()
        .is_none_or(|pattern| pattern.is_match(text))
}

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

/// Wording-rule hits in one text.
pub fn warnings_in(text: &str) -> Vec<WordingWarning> {
    let mut found = Vec::new();
    scan(&mut found, "", text);
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
            if let Some(flag) = &note.flag {
                for line in &flag.scenario {
                    scan(&mut found, &format!("{at}.scenario"), line);
                }
                scan(&mut found, &format!("{at}.not_checked"), &flag.not_checked);
            }
        }
        if let Some(assessment) = &step.assessment {
            let at = format!("{at}.assessment");
            for (field, text) in [
                (
                    "headline",
                    assessment.headline.as_deref().unwrap_or_default(),
                ),
                ("reason", assessment.reason.as_str()),
                (
                    "basis_note",
                    assessment.basis_note.as_deref().unwrap_or_default(),
                ),
                (
                    "not_checked",
                    assessment.not_checked.as_deref().unwrap_or_default(),
                ),
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

    /// Capability excuses are rejected in English, Polish and Russian; ordinary review text is not.
    #[test]
    fn capability_excuses_are_caught_in_every_language() {
        for excuse in [
            "As an AI I cannot judge this.",
            "Jako model językowy nie oceniam tego.",
            "Nie mogłem wczytać wymaganego skilla severity.",
            "Как ИИ я не могу это оценить.",
            "Как языковая модель, я не оцениваю это.",
            "Я не смог загрузить навык severity.",
            "Не могу прочитать инструкции агента.",
            "Навык severity недоступен, поэтому оценки нет.",
        ] {
            assert!(agent_limitation(excuse), "{excuse}");
        }
        for text in [
            "Не могу понять, зачем менять тайм-аут: вопрос к автору.",
            "Функция загружает конфигурацию до проверки токена.",
            "Nie mogę znaleźć testu dla tej ścieżki błędu.",
        ] {
            assert!(!agent_limitation(text), "{text}");
        }
    }

    /// Review repro: ordinary Russian review text (a component failing, an AI feature) is no capability excuse,
    /// as its English equivalents are not.
    #[test]
    fn ordinary_russian_technical_text_is_no_capability_excuse() {
        let rejected: Vec<&str> = [
            "Парсер не смог открыть файл, и инструмент завершился с ошибкой.",
            "Функция не смогла прочитать конфигурацию инструмента, если путь пустой.",
            "После обновления клиенты не смогли загрузить инструкции по установке.",
            "Сервис работает как AI-прокси между клиентом и моделью.",
            "Модуль кэширует ответы так же, как ИИ-ассистент в редакторе.",
            "Тест проверяет, что CLI не смог прочитать файл без прав.",
            "The parser could not open the file and the tool crashed.",
        ]
        .into_iter()
        .filter(|text| agent_limitation(text))
        .collect();
        assert!(rejected.is_empty(), "{rejected:#?}");
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
    fn flag_scenario_and_not_checked_wording_is_scanned() {
        let mut output: FlowAnalysisOutput = serde_json::from_str(include_str!(
            "../../../../fixtures/pr-482/agent/flow-2.json"
        ))
        .expect("fixture");
        assert!(flow(&output).is_empty());
        let flag = output.steps[0]
            .annotations
            .iter_mut()
            .find_map(|a| a.flag.as_mut())
            .expect("a flag");
        flag.scenario[1] = "The retry was verified.".to_owned();
        flag.not_checked = "Nothing; no risk.".to_owned();
        assert_eq!(
            phrases(&flow(&output)),
            [
                ("verified", "steps[0].annotations[2].scenario"),
                ("no risk", "steps[0].annotations[2].not_checked"),
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

    #[test]
    fn wording_rules_match_the_shared_rules_file() {
        let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts");
        let rules: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(scripts.join("wording-rules.json"))
                .expect("wording-rules.json"),
        )
        .expect("wording-rules.json is JSON");
        assert_eq!(rules["words"].as_str(), Some(BANNED_PHRASES), "`words`");
        assert_eq!(rules["percent"].as_str(), Some(PERCENTAGE), "`percent`");
        // Same flags as the source check: banned phrases case-insensitive, percentages as written.
        let script =
            std::fs::read_to_string(scripts.join("check-wording.mjs")).expect("check-wording.mjs");
        assert!(
            script.contains(r#"new RegExp(RULES.words, "giu")"#),
            "WORDS flags"
        );
        assert!(
            script.contains(r#"new RegExp(RULES.percent, "gu")"#),
            "PERCENT flags"
        );
        assert_eq!(RULES.len(), 2, "both patterns compile");
    }
}
