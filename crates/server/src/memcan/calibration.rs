//! Severity calibration examples (`overrated` / `underrated` feedback written by this server) read back from
//! MemCan for the prompts. Memories are untrusted data (any MemCan writer, the agent included, can forge them):
//! only the text is kept, cut to [`MAX_EXAMPLE_CHARS`], at most [`MAX_EXAMPLES`] of them.

use serde::Deserialize;
use serde_json::json;

use super::client::{MemcanClient, MemcanError};
use super::memory::{CALIBRATION_TYPE, SOURCE, clip};
use crate::config::MemcanConfig;

/// Most examples returned.
pub const MAX_EXAMPLES: usize = 8;
/// Longest example text, in characters.
pub const MAX_EXAMPLE_CHARS: usize = 1000;
/// Longest search query sent, in characters.
const MAX_QUERY_CHARS: usize = 500;
/// Memories asked for; non-calibration hits are dropped before [`MAX_EXAMPLES`] applies.
const SEARCH_LIMIT: u32 = 30;

/// Direction of a calibration example.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalibrationDirection {
    /// The reviewer judged the LLM's risk rating too high.
    Overrated,
    /// The reviewer judged the LLM's risk rating too low.
    Underrated,
}

/// One calibration example: untrusted, single-line text to be given to the agent as data, never as instructions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalibrationExample {
    /// Direction.
    pub direction: CalibrationDirection,
    /// The memory text (at most [`MAX_EXAMPLE_CHARS`] characters, no control characters).
    pub text: String,
}

/// Severity calibration examples of `project` (the analysed repository's short name), most similar to `query`
/// first. An error means "not checked", never "no examples".
pub async fn calibration_examples(
    config: &MemcanConfig,
    project: &str,
    query: &str,
) -> Result<Vec<CalibrationExample>, MemcanError> {
    const TOOL: &str = "search_memories";
    let client = MemcanClient::connect(config).await?;
    let answer = client
        .call(
            TOOL,
            json!({
                "query": clip(query, MAX_QUERY_CHARS),
                "project": project,
                "limit": SEARCH_LIMIT,
            }),
        )
        .await?;
    parse_examples(&answer).ok_or(MemcanError::InvalidResponse { tool: TOOL })
}

#[derive(Deserialize)]
struct SearchAnswer {
    memories: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Memory {
    memory: String,
    #[serde(default)]
    private_text_excluded: bool,
    #[serde(rename = "type")]
    kind: String,
    source: String,
    value: String,
}

/// The calibration examples in a `search_memories` answer; `None` if the answer is not one. Other memories
/// (wrong shape, other source or type) are skipped.
fn parse_examples(answer: &str) -> Option<Vec<CalibrationExample>> {
    let answer: SearchAnswer = serde_json::from_str(answer).ok()?;
    Some(
        answer
            .memories
            .into_iter()
            .filter_map(|memory| serde_json::from_value::<Memory>(memory).ok())
            .filter(|memory| {
                memory.kind == CALIBRATION_TYPE
                    && memory.source == SOURCE
                    && memory.private_text_excluded
            })
            .filter_map(|memory| {
                let direction = match memory.value.as_str() {
                    "overrated" => CalibrationDirection::Overrated,
                    "underrated" => CalibrationDirection::Underrated,
                    _ => return None,
                };
                let text = clip(&memory.memory, MAX_EXAMPLE_CHARS);
                (!text.is_empty()).then_some(CalibrationExample { direction, text })
            })
            .take(MAX_EXAMPLES)
            .collect(),
    )
}
