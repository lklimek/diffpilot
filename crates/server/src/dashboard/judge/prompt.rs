//! The `judge_work` prompt: `prompts/judge_work.md` around the input's data block (plan §B.2). Untrusted titles and
//! texts appear only inside that block.

use contracts::agent::judge_work::{InputError, JudgeWorkInput};

/// The template; `{{data}}` is replaced by the data block.
pub(super) const TEMPLATE: &str = include_str!("../../../../../prompts/judge_work.md");
const DATA: &str = "{{data}}";

/// The prompt of one judge job.
pub(super) fn render(input: &JudgeWorkInput) -> Result<String, InputError> {
    Ok(TEMPLATE.replacen(DATA, &input.data_block()?, 1))
}
