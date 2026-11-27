//! The review loop's GitHub reads (Q-L1–Q-L4 of `docs/review-workflow-plan.md` §4.1) from a recording directory
//! such as `fixtures/review-loop/github`: a document whose query and `o`/`n`/`pr` variables equal a recording's is
//! answered with that recording. Exact review-thread reads reuse the same recorded comments.

use std::path::Path;

use axum::body::Bytes;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

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
    pull: Option<Value>,
    threads: Vec<Value>,
}

fn picks(variables: &Value) -> Vec<Value> {
    PICKS
        .iter()
        .map(|key| variables.get(*key).cloned().unwrap_or(Value::Null))
        .collect()
}

fn recorded_pull(variables: &Value, repo: &Value) -> Option<Value> {
    let owner = variables["o"].as_str()?;
    let name = variables["n"].as_str()?;
    Some(json!({
        "number": variables["pr"].as_u64()?,
        "title": repo["pullRequest"]["title"].as_str()?,
        "repository": {
            "nameWithOwner": format!("{owner}/{name}"),
            "isPrivate": repo["isPrivate"].as_bool()?,
        },
    }))
}

impl ReviewLoop {
    /// Loads the recordings of `dir`.
    pub(crate) fn load(dir: &Path) -> Result<Self, StartError> {
        let mut answers = Vec::new();
        let mut pull = None;
        let mut threads = Vec::new();
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
            if matches!(file, "pr.json" | "review-threads.json") {
                let response: Value =
                    serde_json::from_str(recording.response.get()).map_err(|_| bad("response"))?;
                let repo = &response["data"]["repository"];
                if file == "pr.json" {
                    pull = recorded_pull(&variables, repo);
                } else {
                    threads = repo["pullRequest"]["reviewThreads"]["nodes"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                }
            }
            answers.push(Answer {
                query,
                picks: picks(&variables),
                status,
                body: Bytes::from(recording.response.get().to_owned()),
            });
        }
        Ok(Self {
            answers,
            pull,
            threads,
        })
    }

    /// The recorded answer of the document, if it is one of the recorded reads.
    pub(crate) fn answer(&self, query: &str, variables: &Value) -> Option<Response> {
        let picked = picks(variables);
        self.answers
            .iter()
            .find(|answer| answer.query == query && answer.picks == picked)
            .map(|answer| crate::json_body(answer.status, answer.body.clone()))
            .or_else(|| self.thread_detail(query, variables))
    }

    fn thread_detail(&self, query: &str, variables: &Value) -> Option<Response> {
        if !query.contains("node(id:$id)") || !query.contains("... on PullRequestReviewThread") {
            return None;
        }
        let id = variables["id"].as_str()?;
        let thread = self.threads.iter().find(|thread| thread["id"] == id)?;
        let body = json!({"data": {"node": {
            "__typename": "PullRequestReviewThread",
            "path": thread["path"],
            "pullRequest": self.pull.as_ref()?,
            "comments": {
                "pageInfo": {"hasNextPage": false, "endCursor": null},
                "nodes": thread["comments"]["nodes"],
            },
        }}});
        Some(crate::json_body(
            StatusCode::OK,
            Bytes::from(body.to_string()),
        ))
    }
}
