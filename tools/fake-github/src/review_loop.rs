//! The review loop's GitHub reads (Q-L1–Q-L4 of `docs/review-workflow-plan.md` §4.1) from a recording directory
//! such as `fixtures/review-loop/github`: a document whose query and `o`/`n`/`pr` variables equal a recording's is
//! answered with that recording; anything else goes on to the other GraphQL handlers.

use std::path::Path;

use axum::body::Bytes;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::Value;

use crate::StartError;
use crate::fixtures::{RawRecording, load};

/// The recordings served, one per query (PR facts, review threads, reviews, PR comments).
const RECORDINGS: [&str; 4] = [
    "pr.json",
    "review-threads.json",
    "reviews.json",
    "comments.json",
];
/// Variables that pick the pull request; others (the page cursor `after`) are not compared.
const PICKS: [&str; 3] = ["o", "n", "pr"];

/// One recorded answer.
struct Answer {
    query: String,
    picks: Vec<Value>,
    status: StatusCode,
    body: Bytes,
}

/// The recorded review-loop reads.
pub(crate) struct ReviewLoop {
    answers: Vec<Answer>,
}

fn picks(variables: &Value) -> Vec<Value> {
    PICKS
        .iter()
        .map(|key| variables.get(*key).cloned().unwrap_or(Value::Null))
        .collect()
}

impl ReviewLoop {
    /// Loads the recordings of `dir`.
    pub(crate) fn load(dir: &Path) -> Result<Self, StartError> {
        let mut answers = Vec::new();
        for file in RECORDINGS {
            let recording: RawRecording = load(dir, file)?;
            let bad = |field| StartError::BadRecording {
                path: dir.join(file),
                field,
            };
            let query = recording
                .request
                .body
                .get("query")
                .and_then(Value::as_str)
                .ok_or_else(|| bad("request.body.query"))?
                .to_owned();
            let variables = recording
                .request
                .body
                .get("variables")
                .cloned()
                .unwrap_or(Value::Null);
            let status = StatusCode::from_u16(recording.status).map_err(|_| bad("status"))?;
            answers.push(Answer {
                query,
                picks: picks(&variables),
                status,
                body: Bytes::from(recording.response.get().to_owned()),
            });
        }
        Ok(Self { answers })
    }

    /// The recorded answer of the document, if it is one of the recorded reads.
    pub(crate) fn answer(&self, query: &str, variables: &Value) -> Option<Response> {
        let picked = picks(variables);
        self.answers
            .iter()
            .find(|answer| answer.query == query && answer.picks == picked)
            .map(|answer| crate::json_body(answer.status, answer.body.clone()))
    }
}
