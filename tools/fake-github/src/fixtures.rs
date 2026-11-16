//! Recordings of `fixtures/pr-482/github`: `{request, status, response}` per file.

use std::path::Path;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use url::Url;

use crate::StartError;

/// REST endpoints served exactly as recorded; method and path come from the recording's `request`.
/// Adding an endpoint is one entry.
pub(crate) const RECORDED_ENDPOINTS: &[&str] = &[
    "rest/pull.json",
    "rest/pull-commits.json",
    "rest/pull-files.json",
];
/// `GET /user` of the fixture user.
pub(crate) const USER: &str = "rest/user.json";
/// The OAuth code exchange: client credentials, first code and the token response.
pub(crate) const ACCESS_TOKEN: &str = "oauth/access-token.json";

/// One recording; `B` is the request body, `R` the response.
#[derive(Deserialize)]
pub(crate) struct Recording<B, R> {
    pub(crate) request: Request<B>,
    pub(crate) status: u16,
    pub(crate) response: R,
}

/// The recorded request.
#[derive(Deserialize)]
pub(crate) struct Request<B> {
    pub(crate) method: String,
    pub(crate) url: Url,
    pub(crate) body: B,
}

/// A recording whose response is kept byte for byte.
pub(crate) type RawRecording = Recording<serde_json::Value, Box<RawValue>>;

/// Reads and parses `<dir>/<file>`.
pub(crate) fn load<T: DeserializeOwned>(dir: &Path, file: &str) -> Result<T, StartError> {
    let path = dir.join(file);
    let text = std::fs::read_to_string(&path).map_err(|source| StartError::ReadFixture {
        path: path.clone(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| StartError::ParseFixture { path, source })
}
