//! Recorded decisions bind only after the host checks the writer, topic, chronology and cue.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use contracts::agent::comment_report::{ProposedDecision, RecordedDecision, RecordedDecisionKind};
use contracts::agent::pr_data::PrData;
use contracts::api::review_loop::{DecisionKind, DecisionTarget, ImportedDecision};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::review_loop::ImportCandidate;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{adapter, host};
use crate::github::{rest, threads::github_web_url};
use crate::review_loop::{handin, markers};
use crate::state::AppState;

type Resolution = (Instant, u64);

/// Positive resolutions expire after 24 hours. A failed lookup carries no authority.
#[derive(Clone, Debug, Default)]
pub struct RelayIds(Arc<Mutex<HashMap<String, Resolution>>>);

impl RelayIds {
    async fn resolve(&self, state: &AppState, token: &SecretString, login: &str) -> Option<u64> {
        let ttl = Duration::from_secs(24 * 60 * 60);
        {
            let mut cache = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            cache.retain(|_, (at, _)| at.elapsed() < ttl);
            if let Some((_, id)) = cache.get(login) {
                return Some(*id);
            }
        }
        let id = rest::user_id(&state.config.github, token, login).await?;
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(login.to_owned(), (Instant::now(), id));
        Some(id)
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Principal {
    user: String,
    login: String,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Authorities {
    members: BTreeMap<u64, Principal>,
    relays: BTreeMap<u64, Option<Principal>>,
}

impl Authorities {
    fn add_relay(&mut self, id: u64, principal: Principal) {
        self.relays
            .entry(id)
            .and_modify(|entry| {
                if entry
                    .as_ref()
                    .is_some_and(|other| other.user != principal.user)
                {
                    *entry = None;
                }
            })
            .or_insert(Some(principal));
    }

    pub(super) async fn read(
        state: &AppState,
        tenant: &str,
        payer: &str,
        manifest: &mut contracts::agent::pr_data::Manifest,
    ) -> Result<Self, DbError> {
        let mut authorities = Self::default();
        let mut linked = BTreeMap::<String, Vec<Principal>>::new();
        manifest.maintainers.clear();
        manifest.delegates.clear();
        for member in state.db.users().members(tenant).await? {
            let principal = Principal {
                user: member.id,
                login: member.profile.login,
            };
            manifest.maintainers.push(principal.login.clone());
            if let Ok(id) = u64::try_from(member.profile.github_id) {
                authorities.members.insert(id, principal.clone());
            }
            if let Some(settings) = state
                .db
                .dashboard()
                .settings(tenant, &principal.user)
                .await?
            {
                for login in settings.settings.linked_logins {
                    linked
                        .entry(login.to_ascii_lowercase())
                        .or_default()
                        .push(principal.clone());
                }
            }
        }
        manifest.maintainers.sort_unstable();
        if linked.is_empty() {
            return Ok(authorities);
        }
        if let Ok(token) = crate::auth::user_access_token(state, payer).await {
            let mut resolved = Vec::new();
            for (login, owners) in linked {
                if let Some(id) = state.relay_ids.resolve(state, &token, &login).await {
                    for owner in owners {
                        authorities.add_relay(id, owner);
                    }
                    resolved.push((id, login));
                }
            }
            manifest
                .delegates
                .extend(resolved.into_iter().filter_map(|(id, login)| {
                    authorities
                        .relays
                        .get(&id)
                        .is_some_and(Option::is_some)
                        .then_some(login)
                }));
        }
        Ok(authorities)
    }
}

pub(super) struct Point<'a> {
    pub url: &'a str,
    pub thread: Option<&'a str>,
    pub quote: &'a str,
    pub written_at: Option<Timestamp>,
    pub shared_url: bool,
}

fn author_id(comment: &Value) -> Option<u64> {
    comment
        .pointer("/author/databaseId")
        .or_else(|| comment.pointer("/user/databaseId"))?
        .as_u64()
        .filter(|id| *id != 0)
}

fn at(comment: &Value) -> Option<Timestamp> {
    ["createdAt", "created_at", "submitted_at"]
        .iter()
        .find_map(|key| comment[*key].as_str())?
        .parse()
        .ok()
}

fn kind_of(d: &RecordedDecision) -> Option<DecisionKind> {
    match d.kind {
        RecordedDecisionKind::Deferred => Some(DecisionKind::Todo),
        RecordedDecisionKind::Withdrawn => Some(DecisionKind::Reject),
        RecordedDecisionKind::Decision(kind) => {
            serde_json::from_value(serde_json::to_value(kind).ok()?).ok()
        }
    }
}

fn target_of(data: &PrData, d: &RecordedDecision, kind: DecisionKind) -> Option<DecisionTarget> {
    match kind {
        DecisionKind::HandledInPr => {
            let proposal = ProposedDecision {
                kind: contracts::agent::comment_report::ProposedDecisionKind::HandledInPr,
                target_pr: d.target_pr.filter(|n| *n != 0),
                target_url: match &d.target_url {
                    Some(url) => Some(github_web_url(url)?),
                    None => None,
                },
                title: None,
                fixed_in: None,
            };
            adapter::local_pr(&proposal, &data.manifest.repo)
                .map(|number| DecisionTarget::ExistingPr { number })
        }
        DecisionKind::UpstreamIssue => d
            .target_url
            .as_deref()
            .and_then(github_web_url)
            .map(|url| DecisionTarget::UpstreamIssue { url: Some(url) }),
        _ => None,
    }
}

fn source<'a>(data: &'a PrData, url: &str) -> Option<&'a Value> {
    host::all_comments(data).find(|comment| host::url(comment) == url)
}

fn evidence_version(point: &Value, evidence: &Value) -> String {
    handin::hash(format!("{point}\n{evidence}"))
}

pub(crate) fn version(data: &PrData, point: &str, evidence: &str) -> Option<String> {
    Some(evidence_version(
        source(data, point)?,
        source(data, evidence)?,
    ))
}

pub(super) fn candidate(
    data: &PrData,
    authorities: &Authorities,
    payer: &str,
    point: Point<'_>,
    d: &RecordedDecision,
) -> Option<ImportCandidate> {
    let original = source(data, point.url)?;
    let evidence = source(data, &d.comment_url)?;
    let written = at(evidence)?;
    let named = d.on_behalf_of.as_deref();
    let id = author_id(evidence)?;
    let kind = kind_of(d)?;
    if written <= point.written_at.or_else(|| at(original))? || !host::grounded(evidence, d) {
        return None;
    }
    let same_topic = if let Some(thread) = point.thread {
        host::threads(data)
            .find(|node| node["id"] == thread)
            .is_some_and(|node| host::comments(node).any(|c| host::url(c) == d.comment_url))
    } else {
        let body = host::body(evidence);
        let linked = |url: &str| {
            body.match_indices(url).any(|(i, _)| {
                body[..i]
                    .chars()
                    .next_back()
                    .is_none_or(|c| c.is_whitespace() || "(<['\"".contains(c))
                    && body[i + url.len()..]
                        .chars()
                        .next()
                        .is_none_or(|c| c.is_whitespace() || ")]>.,'\"".contains(c))
            })
        };
        let other_mentions = data.comments.iter().chain(&data.reviews).any(|other| {
            let url = host::url(other);
            !url.is_empty()
                && url != point.url
                && url != d.comment_url
                && (linked(url) || host::distinct_quote(host::body(other), point.quote))
        });
        (!point.shared_url && linked(point.url))
            || (!other_mentions && host::distinct_quote(body, point.quote))
    };
    if !same_topic {
        return None;
    }
    let marker = evidence["relayMarker"]
        .as_str()
        .and_then(markers::relay_kind)
        == Some(kind);
    let withdrawal = d.kind == RecordedDecisionKind::Withdrawn;
    if withdrawal && author_id(original) != Some(id) {
        return None;
    }
    let (user, relay) = if withdrawal {
        (
            authorities
                .members
                .get(&id)
                .map_or(payer, |p| p.user.as_str()),
            None,
        )
    } else if let Some(member) = authorities.members.get(&id) {
        (&*member.user, None)
    } else if marker {
        let member = authorities.relays.get(&id)?.as_ref()?;
        if named.is_some_and(|name| !name.eq_ignore_ascii_case(&member.login)) {
            return None;
        }
        (&*member.user, Some(host::author(evidence).to_owned()))
    } else {
        return None;
    };
    Some(ImportCandidate {
        user_id: user.into(),
        kind,
        target: target_of(data, d, kind),
        binding: relay.is_some() || markers::decision_cue(kind, &d.quote, withdrawal),
        author_withdrawal: withdrawal,
        version: evidence_version(original, evidence),
        evidence: ImportedDecision {
            by: host::author(evidence).into(),
            by_id: Some(id),
            at: written,
            evidence_url: github_web_url(&d.comment_url)?,
            quote: d.quote.clone(),
            relay,
        },
    })
}

#[cfg(test)]
mod tests;
