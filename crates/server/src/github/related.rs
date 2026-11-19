//! Strict related-work targets, checked with GitHub's issue endpoint (which includes pull requests).

use std::time::Duration;

use contracts::agent::pr_context::RelatedWork;
use contracts::api::tour::{RelatedWorkLink, RelatedWorkTargetKind};
use contracts::common::{RelatedWorkKind, RepoRef};
use reqwest::{Client, redirect};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use super::{oauth::endpoint, rest::is_valid_repo};
use crate::config::GithubConfig;

/// Parses only an entire canonical reference, never prose or an arbitrary URL.
pub fn target(item: &RelatedWork, current: &RepoRef) -> Option<RelatedWorkLink> {
    let reference = item.reference.trim();
    let inferred = if item.kind == RelatedWorkKind::LinkedIssue {
        RelatedWorkTargetKind::Issue
    } else {
        RelatedWorkTargetKind::PullRequest
    };
    let (repo, number, kind) = if let Some(path) = reference.strip_prefix("https://github.com/") {
        let parts: Vec<_> = path.split('/').collect();
        let [owner, name, kind, number] = parts.as_slice() else {
            return None;
        };
        let kind = match *kind {
            "pull" => RelatedWorkTargetKind::PullRequest,
            "issues" => RelatedWorkTargetKind::Issue,
            _ => return None,
        };
        (format!("{owner}/{name}"), *number, kind)
    } else {
        let (repo, number) = reference.split_once('#')?;
        (
            if repo.is_empty() {
                format!("{}/{}", current.owner, current.name)
            } else {
                repo.to_owned()
            },
            number,
            inferred,
        )
    };
    let (owner, name) = repo.split_once('/')?;
    if !is_valid_repo(&RepoRef {
        owner: owner.to_owned(),
        name: name.to_owned(),
    }) || !number.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let number: u64 = number.parse().ok()?;
    if number == 0 || number > 9_007_199_254_740_991 {
        return None;
    }
    Some(RelatedWorkLink {
        reference: item.reference.clone(),
        repo,
        number,
        kind,
        verified: false,
    })
}

#[derive(Deserialize)]
struct Issue {
    number: u64,
    pull_request: Option<serde_json::Value>,
}

/// Checks at most 20 validated targets; unavailable/denied resources remain explicitly unverified.
pub async fn verify(config: &GithubConfig, token: &SecretString, links: &mut [RelatedWorkLink]) {
    let Ok(client) = Client::builder()
        .redirect(redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
    else {
        return;
    };
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        for link in links.iter_mut().take(20) {
            let response = client
                .get(endpoint(
                    &config.api_url,
                    &format!("repos/{}/issues/{}", link.repo, link.number),
                ))
                .bearer_auth(token.expose_secret())
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "diffpilot")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .send()
                .await;
            let Ok(response) = response else {
                continue;
            };
            if response.status() != reqwest::StatusCode::OK {
                continue;
            }
            let Ok(issue) = response.json::<Issue>().await else {
                continue;
            };
            if issue.number != link.number {
                continue;
            }
            link.kind = if issue.pull_request.is_some() {
                RelatedWorkTargetKind::PullRequest
            } else {
                RelatedWorkTargetKind::Issue
            };
            link.verified = true;
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::graphql::tests::github_config;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    fn item(reference: &str) -> RelatedWork {
        RelatedWork {
            reference: reference.to_owned(),
            kind: RelatedWorkKind::StackedPr,
            overlap: String::new(),
        }
    }
    fn repo() -> RepoRef {
        RepoRef {
            owner: "acme".to_owned(),
            name: "node".to_owned(),
        }
    }

    #[test]
    fn links_accept_only_structured_github_references() {
        for reference in [
            "acme/node#42",
            "#42",
            "https://github.com/acme/node/pull/42",
            "https://github.com/acme/node/issues/42",
        ] {
            let parsed = target(&item(reference), &repo()).unwrap();
            assert_eq!(parsed.repo, "acme/node");
            assert_eq!(parsed.number, 42);
            assert!(!parsed.verified);
        }
        for reference in [
            "https://evil.test/acme/node/pull/42",
            "javascript:alert(1)",
            "acme/../node#42",
            "acme/node#0",
            "acme/node#9007199254740992",
            "see acme/node#42",
            "https://github.com/acme/node/pull/42?x=y",
        ] {
            assert!(target(&item(reference), &repo()).is_none(), "{reference}");
        }
    }

    #[tokio::test]
    async fn github_confirms_kind_and_denial_stays_unverified() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/node/issues/42"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"number":42})),
            )
            .mount(&server)
            .await;
        let mut links = [
            target(&item("acme/node#42"), &repo()).unwrap(),
            target(&item("acme/node#43"), &repo()).unwrap(),
        ];
        verify(
            &github_config(&server),
            &SecretString::from("test-token"),
            &mut links,
        )
        .await;
        assert!(links[0].verified);
        assert_eq!(links[0].kind, RelatedWorkTargetKind::Issue);
        assert!(!links[1].verified);
    }
}
