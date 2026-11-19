//! The signed-in user's GitHub "Viewed" mark per file of a pull request, through GraphQL with the user's token:
//! read with `pullRequest.files { path viewerViewedState }`, written with `markFileAsViewed` /
//! `unmarkFileAsViewed` (GitHub GraphQL reference, pulls, checked 2026-11-19). [`DOCUMENTS`] lists every document.

use std::collections::BTreeMap;

use contracts::api::files::FileViewedState;
use contracts::common::PrRef;
use secrecy::SecretString;
use serde::Deserialize;
use serde_json::json;

use crate::config::GithubConfig;
use crate::github::graphql::{self, GraphqlError};

/// The pull request's node id, head and one page of its files with the viewer's Viewed state.
pub const FILES_VIEWED: &str = "query($o:String!,$n:String!,$pr:Int!,$after:String){repository(owner:$o,name:$n){\
pullRequest(number:$pr){id headRefOid files(first:100,after:$after){pageInfo{hasNextPage endCursor} \
nodes{path viewerViewedState}}}}}";

/// Marks a file as viewed by the user.
pub const MARK_VIEWED: &str =
    "mutation($in:MarkFileAsViewedInput!){markFileAsViewed(input:$in){clientMutationId}}";

/// Removes the user's Viewed mark of a file.
pub const UNMARK_VIEWED: &str =
    "mutation($in:UnmarkFileAsViewedInput!){unmarkFileAsViewed(input:$in){clientMutationId}}";

/// Every GraphQL document of the Viewed sync.
pub const DOCUMENTS: [&str; 3] = [FILES_VIEWED, MARK_VIEWED, UNMARK_VIEWED];

/// Pages of 100 files read at most: GitHub lists at most 3000 files of a pull request.
pub const MAX_FILE_PAGES: usize = 30;

/// The Viewed state as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewedFiles {
    /// GraphQL node id of the pull request (the mutations' `pullRequestId`).
    pub pull_request_id: String,
    /// The pull request's current head.
    pub head_oid: String,
    /// The viewer's state by path.
    pub files: BTreeMap<String, FileViewedState>,
    /// More files exist than were read.
    pub incomplete: bool,
}

/// Reading the Viewed state failed.
#[derive(Debug, thiserror::Error)]
pub enum ViewedReadError {
    /// The GraphQL call failed.
    #[error("the GitHub GraphQL call failed")]
    Graphql(#[from] GraphqlError),
    /// The pull request is not visible to the user.
    #[error("GitHub did not return the pull request")]
    MissingPullRequest,
}

#[derive(Deserialize)]
struct Data {
    repository: Option<Repository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    pull_request: Option<PullRequest>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequest {
    id: String,
    head_ref_oid: String,
    files: Option<Files>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Files {
    page_info: PageInfo,
    nodes: Vec<FileNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileNode {
    path: String,
    viewer_viewed_state: Option<String>,
}

fn state(name: Option<&str>) -> Option<FileViewedState> {
    match name? {
        "VIEWED" => Some(FileViewedState::Viewed),
        "UNVIEWED" => Some(FileViewedState::Unviewed),
        "DISMISSED" => Some(FileViewedState::Dismissed),
        _ => None,
    }
}

/// The user's Viewed state of every file of `pr` (up to [`MAX_FILE_PAGES`] pages), with the node id and head.
pub async fn read(
    config: &GithubConfig,
    token: &SecretString,
    pr: &PrRef,
) -> Result<ViewedFiles, ViewedReadError> {
    let mut out: Option<ViewedFiles> = None;
    let mut after: Option<String> = None;
    for _ in 0..MAX_FILE_PAGES {
        let variables =
            json!({"o": pr.repo.owner, "n": pr.repo.name, "pr": pr.number, "after": after});
        let data: Data = graphql::execute(config, token, FILES_VIEWED, variables).await?;
        let pull = data
            .repository
            .and_then(|repository| repository.pull_request)
            .ok_or(ViewedReadError::MissingPullRequest)?;
        let read = out.get_or_insert_with(|| ViewedFiles {
            pull_request_id: pull.id,
            head_oid: pull.head_ref_oid,
            files: BTreeMap::new(),
            incomplete: false,
        });
        let Some(files) = pull.files else {
            read.incomplete = true;
            break;
        };
        for node in files.nodes {
            if let Some(state) = state(node.viewer_viewed_state.as_deref()) {
                read.files.insert(node.path, state);
            }
        }
        match (files.page_info.has_next_page, files.page_info.end_cursor) {
            (false, _) => return out.ok_or(ViewedReadError::MissingPullRequest),
            (true, None) => {
                read.incomplete = true;
                break;
            }
            (true, Some(cursor)) => after = Some(cursor),
        }
    }
    let mut read = out.ok_or(ViewedReadError::MissingPullRequest)?;
    if after.is_some() {
        read.incomplete = true;
    }
    if read.incomplete {
        tracing::debug!(
            files = read.files.len(),
            "the pull request has more files than the Viewed read covers"
        );
    }
    Ok(read)
}

/// Writing the Viewed mark failed.
#[derive(Debug, thiserror::Error)]
pub enum ViewedWriteError {
    /// The GraphQL call failed.
    #[error("the GitHub GraphQL call failed")]
    Graphql(#[from] GraphqlError),
    /// GitHub answered without the mutation's payload and without an error: whether it applied is unknown.
    #[error("GitHub answered without the mutation's result")]
    NoPayload,
}

#[derive(Deserialize)]
struct Written {
    #[serde(rename = "markFileAsViewed", alias = "unmarkFileAsViewed")]
    payload: Option<serde_json::Value>,
}

/// Marks (`viewed`) or unmarks `path` of the pull request `pull_request_id` as viewed by the token's user.
pub async fn set(
    config: &GithubConfig,
    token: &SecretString,
    pull_request_id: &str,
    path: &str,
    viewed: bool,
) -> Result<(), ViewedWriteError> {
    let document = if viewed { MARK_VIEWED } else { UNMARK_VIEWED };
    let variables = json!({"in": {"pullRequestId": pull_request_id, "path": path}});
    let written: Written = graphql::execute(config, token, document, variables).await?;
    written
        .payload
        .map(|_| ())
        .ok_or(ViewedWriteError::NoPayload)
}
