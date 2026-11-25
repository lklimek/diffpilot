//! The `judge_work` prompt: `prompts/judge_work.md` around the input's data block (plan §B.2). Untrusted titles and
//! texts appear only inside that block.

use contracts::agent::judge_work::JudgeWorkInput;
use contracts::common::LlmLanguage;

use super::RankError;
use crate::prompts::translated_technical_terms;

/// The template; language instructions are filled before the data block is inserted.
pub(super) const TEMPLATE: &str = include_str!("../../../../../prompts/judge_work.md");
const DATA: &str = "{{data}}";
const LANGUAGE: &str = "{{language}}";

/// The prompt of one judge job whose texts are written in `language`.
pub(super) fn render(input: &JudgeWorkInput, language: LlmLanguage) -> Result<String, RankError> {
    let name = match language {
        LlmLanguage::En => "English",
        LlmLanguage::Pl => "Polish (polski)",
        LlmLanguage::Ru => "Russian (русский)",
    };
    Ok(TEMPLATE
        .replace(LANGUAGE, name)
        .replace(
            "{{translated_technical_terms}}",
            &translated_technical_terms(language)?,
        )
        .replacen(DATA, &input.data_block()?, 1))
}
