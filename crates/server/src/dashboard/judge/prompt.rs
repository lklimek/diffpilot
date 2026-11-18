//! The `judge_work` prompt: `prompts/judge_work.md` around the input's data block (plan §B.2). Untrusted titles and
//! texts appear only inside that block.

use contracts::agent::judge_work::{InputError, JudgeWorkInput};
use contracts::common::LlmLanguage;

/// The template; `{{language}}` is replaced by the user's language, `{{data}}` by the data block.
pub(super) const TEMPLATE: &str = include_str!("../../../../../prompts/judge_work.md");
const DATA: &str = "{{data}}";
const LANGUAGE: &str = "{{language}}";

/// The prompt of one judge job whose texts are written in `language`.
pub(super) fn render(input: &JudgeWorkInput, language: LlmLanguage) -> Result<String, InputError> {
    let name = match language {
        LlmLanguage::En => "English",
        LlmLanguage::Pl => "Polish (polski)",
        LlmLanguage::Ru => "Russian (русский)",
    };
    Ok(TEMPLATE
        .replacen(LANGUAGE, name, 1)
        .replacen(DATA, &input.data_block()?, 1))
}
