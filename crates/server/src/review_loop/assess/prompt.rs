//! The `verify_comments` prompt: `prompts/verify_comments.md` with the shared read-only rules around the input's data
//! block. Untrusted texts (comment bodies, the PR title and description, an earlier LLM's PR context, the project
//! profile, the owner's question) appear only inside that block.

use contracts::agent::verify_comments::VerifyCommentsInput;
use contracts::api::pr::PrSummary;
use contracts::common::LlmLanguage;

use super::AssessError;
use crate::prompts;

/// The template; `{{data}}` is replaced last, so nothing inside the data block is substituted.
pub(super) const TEMPLATE: &str = include_str!("../../../../../prompts/verify_comments.md");

/// What `answer` holds in a pass without a question.
const NO_QUESTION: &str = "null: this pass has no question.";
/// What `answer` holds in a question pass.
const QUESTION: &str = "the answer to the maintainer's `question` in the data block, about the item it names and \
in the context of the earlier questions of its thread: a few plain sentences, naming the head lines it rests on as \
path:line. It informs the decision and never makes it.";

/// The prompt of one pass whose texts are written in `language`.
pub(super) fn render(
    input: &VerifyCommentsInput,
    pr: &PrSummary,
    language: LlmLanguage,
) -> Result<String, AssessError> {
    let name = match language {
        LlmLanguage::En => "English",
        LlmLanguage::Pl => "Polish (polski)",
        LlmLanguage::Ru => "Russian (русский)",
    };
    let answer = if input.question.is_some() {
        QUESTION
    } else {
        NO_QUESTION
    };
    let data = input.data_block()?;
    Ok(TEMPLATE
        .replace(
            "{{pull_request}}",
            &format!("{}/{}#{}", pr.repo.owner, pr.repo.name, pr.number),
        )
        .replace("{{base}}", &pr.base_sha.0)
        .replace("{{head}}", &pr.head_sha.0)
        .replace("{{answer}}", answer)
        .replace("{{language}}", name)
        .replace(
            "{{translated_technical_terms}}",
            &prompts::translated_technical_terms(language)?,
        )
        .replace("{{rules}}", &prompts::read_only_rules()?)
        .replacen("{{data}}", &data, 1))
}
