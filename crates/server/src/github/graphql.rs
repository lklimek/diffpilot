//! GitHub GraphQL transport for calls made with the signed-in user's token: one POST per document, GraphQL
//! errors mapped to their `type` (their message text is never kept: it may quote request content).

use std::time::Duration;

use reqwest::header::{ACCEPT, USER_AGENT};
use reqwest::{Client, StatusCode, redirect};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::config::GithubConfig;
use crate::github::oauth::endpoint;

const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const CLIENT_NAME: &str = "smart-review";

/// A GraphQL call failed.
#[derive(Debug, thiserror::Error)]
pub enum GraphqlError {
    /// The HTTP client could not be built.
    #[error("the HTTP client for GitHub could not be created")]
    Client(#[source] reqwest::Error),
    /// The request did not complete; GitHub may or may not have applied a mutation.
    #[error("the request to GitHub did not complete")]
    Http(#[source] reqwest::Error),
    /// GitHub rejected the user token.
    #[error("GitHub rejected the user token")]
    Unauthorized,
    /// GitHub's rate limit is exhausted.
    #[error("the GitHub rate limit is exhausted")]
    RateLimited,
    /// GitHub answered with another unexpected HTTP status.
    #[error("GitHub answered with HTTP status {0}")]
    Status(StatusCode),
    /// The answer is not the expected JSON.
    #[error("GitHub answered with unexpected JSON")]
    Malformed(#[source] serde_json::Error),
    /// The answer has no data and no error.
    #[error("GitHub answered without data")]
    NoData,
    /// GitHub answered with a GraphQL error of this type during execution (`data` present): a mutation may
    /// have taken effect.
    #[error("GitHub rejected the request ({0:?})")]
    Rejected(GraphqlErrorType),
    /// GitHub refused the request before executing it (errors without `data`): nothing was changed.
    #[error("GitHub refused the request before executing it ({0:?})")]
    NotExecuted(GraphqlErrorType),
}

/// The `type` of the first GraphQL error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphqlErrorType {
    /// `NOT_FOUND`: the node does not exist or is not visible to the user.
    NotFound,
    /// `FORBIDDEN`: the token may not do this (e.g. the GitHub App is not installed on the repository).
    Forbidden,
    /// `UNPROCESSABLE`: GitHub refused the change.
    Unprocessable,
    /// `RATE_LIMITED`.
    RateLimited,
    /// Any other or a missing type.
    Other,
}

impl GraphqlErrorType {
    fn of(kind: Option<&str>) -> Self {
        match kind {
            Some("NOT_FOUND") => Self::NotFound,
            Some("FORBIDDEN") => Self::Forbidden,
            Some("UNPROCESSABLE") => Self::Unprocessable,
            Some("RATE_LIMITED") => Self::RateLimited,
            _ => Self::Other,
        }
    }
}

#[derive(Serialize)]
struct Request<'a, V> {
    query: &'a str,
    variables: V,
}

#[derive(Deserialize)]
struct Envelope {
    /// Absent when the request failed before execution; `null` when execution failed at the root.
    #[serde(default, deserialize_with = "present")]
    data: Option<serde_json::Value>,
    #[serde(default)]
    errors: Vec<ErrorEntry>,
}

/// Keeps a present `data: null` apart from a missing `data`.
fn present<'de, De: serde::Deserializer<'de>>(
    deserializer: De,
) -> Result<Option<serde_json::Value>, De::Error> {
    serde_json::Value::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct ErrorEntry {
    #[serde(rename = "type")]
    kind: Option<String>,
}

fn client() -> Result<Client, GraphqlError> {
    Client::builder()
        .redirect(redirect::Policy::none())
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(GraphqlError::Client)
}

/// Runs `document` with `variables` against `<api_url>/graphql` with the user's token; returns `data`.
pub async fn execute<V: Serialize, D: DeserializeOwned>(
    config: &GithubConfig,
    token: &SecretString,
    document: &str,
    variables: V,
) -> Result<D, GraphqlError> {
    let response = client()?
        .post(endpoint(&config.api_url, "graphql"))
        .bearer_auth(token.expose_secret())
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, CLIENT_NAME)
        .json(&Request {
            query: document,
            variables,
        })
        .send()
        .await
        .map_err(GraphqlError::Http)?;
    let status = response.status();
    let exhausted = response
        .headers()
        .get("x-ratelimit-remaining")
        .is_some_and(|value| value.as_bytes() == b"0");
    match status {
        StatusCode::OK => {}
        StatusCode::UNAUTHORIZED => return Err(GraphqlError::Unauthorized),
        StatusCode::TOO_MANY_REQUESTS => return Err(GraphqlError::RateLimited),
        StatusCode::FORBIDDEN if exhausted => return Err(GraphqlError::RateLimited),
        status => return Err(GraphqlError::Status(status)),
    }
    let bytes = response.bytes().await.map_err(GraphqlError::Http)?;
    let envelope: Envelope = serde_json::from_slice(&bytes).map_err(GraphqlError::Malformed)?;
    if let Some(error) = envelope.errors.first() {
        let kind = GraphqlErrorType::of(error.kind.as_deref());
        return Err(match envelope.data {
            Some(_) => GraphqlError::Rejected(kind),
            None => GraphqlError::NotExecuted(kind),
        });
    }
    match envelope.data {
        None | Some(serde_json::Value::Null) => Err(GraphqlError::NoData),
        Some(data) => serde_json::from_value(data).map_err(GraphqlError::Malformed),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::{Value, json};
    use url::Url;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::config::tests::test_config;

    /// GitHub settings whose API base is `server` under `/api`.
    pub(crate) fn github_config(server: &MockServer) -> GithubConfig {
        let mut config = test_config().github;
        config.api_url = Url::parse(&format!("{}/api", server.uri())).expect("url");
        config
    }

    async fn answer(template: ResponseTemplate) -> Result<Value, GraphqlError> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .and(header("authorization", "Bearer ghu_graphql_token"))
            .and(header("user-agent", CLIENT_NAME))
            .and(body_partial_json(
                json!({"query": "query{viewer{login}}", "variables": {"a": 1}}),
            ))
            .respond_with(template)
            .expect(1)
            .mount(&server)
            .await;
        execute(
            &github_config(&server),
            &SecretString::from("ghu_graphql_token"),
            "query{viewer{login}}",
            json!({"a": 1}),
        )
        .await
    }

    #[tokio::test]
    async fn data_is_returned_and_errors_map_to_their_type() {
        let data = answer(ResponseTemplate::new(200).set_body_json(json!({"data": {"x": 1}})))
            .await
            .expect("data");
        assert_eq!(data, json!({"x": 1}));
        for (kind, expected) in [
            ("NOT_FOUND", GraphqlErrorType::NotFound),
            ("FORBIDDEN", GraphqlErrorType::Forbidden),
            ("UNPROCESSABLE", GraphqlErrorType::Unprocessable),
            ("RATE_LIMITED", GraphqlErrorType::RateLimited),
            ("SOMETHING", GraphqlErrorType::Other),
        ] {
            let body = json!({"data": {"x": null}, "errors": [{"type": kind, "message": "m"}]});
            let result = answer(ResponseTemplate::new(200).set_body_json(body)).await;
            assert!(
                matches!(result, Err(GraphqlError::Rejected(found)) if found == expected),
                "{kind}: {result:?}"
            );
            let body = json!({"errors": [{"type": kind, "message": "m"}]});
            let result = answer(ResponseTemplate::new(200).set_body_json(body)).await;
            assert!(
                matches!(result, Err(GraphqlError::NotExecuted(found)) if found == expected),
                "errors without data are refused before execution: {kind}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn http_failures_map_to_typed_errors() {
        let result = answer(ResponseTemplate::new(401)).await;
        assert!(
            matches!(result, Err(GraphqlError::Unauthorized)),
            "{result:?}"
        );
        let result = answer(ResponseTemplate::new(429)).await;
        assert!(
            matches!(result, Err(GraphqlError::RateLimited)),
            "{result:?}"
        );
        let result =
            answer(ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "0")).await;
        assert!(
            matches!(result, Err(GraphqlError::RateLimited)),
            "{result:?}"
        );
        let result = answer(ResponseTemplate::new(502)).await;
        assert!(
            matches!(result, Err(GraphqlError::Status(StatusCode::BAD_GATEWAY))),
            "{result:?}"
        );
        let result = answer(ResponseTemplate::new(200).set_body_string("<html>")).await;
        assert!(
            matches!(result, Err(GraphqlError::Malformed(_))),
            "{result:?}"
        );
        let result = answer(ResponseTemplate::new(200).set_body_json(json!({}))).await;
        assert!(matches!(result, Err(GraphqlError::NoData)), "{result:?}");
        let result = answer(ResponseTemplate::new(200).set_body_json(json!({"data": null}))).await;
        assert!(matches!(result, Err(GraphqlError::NoData)), "{result:?}");
    }
}
