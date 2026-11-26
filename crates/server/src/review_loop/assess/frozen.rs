//! Immutable non-secret input archive; neither method reads later source/decision revisions.

use super::{AssessError, host};
use crate::state::AppState;
use contracts::agent::comment_report::RecordedDecisionKind;
use contracts::agent::pr_data::{PrData, PrDataItem, StandingDecision};
use contracts::agent::verify_comments::VerifyCommentsInput;
use contracts::api::review_loop::{CommentItemKind, DecisionTarget};
use contracts::assessment::AssessmentPass;
use contracts::common::LlmLanguage;
use contracts::ids::PassId;
use db::repo::review_loop::{ItemRecord, SentItem};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

#[derive(Serialize, Deserialize)]
pub(super) struct Frozen {
    pub data: PrData,
    #[serde(default)]
    pub language: Option<LlmLanguage>,
    #[serde(default)]
    pub authorities: super::imports::Authorities,
    #[serde(default)]
    pub own_reply_urls: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub own_reply_texts: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub written: std::collections::BTreeMap<String, contracts::api::review_loop::Written>,
    pub items: Vec<ItemRecord>,
    pub sent: Vec<SentItem>,
    pub native: VerifyCommentsInput,
    #[serde(default)]
    pub standing: std::collections::BTreeMap<String, FrozenDecision>,
    #[serde(default)]
    pub fingerprints: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub purpose_fingerprint: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct FrozenDecision {
    pub id: contracts::ids::DecisionId,
    pub kind: contracts::api::review_loop::DecisionKind,
    pub target: Option<DecisionTarget>,
    pub changed: bool,
}

pub(super) fn reference(id: PassId) -> String {
    format!("assessment-snapshots/{id}.json")
}

impl Frozen {
    pub async fn write(&self, state: &AppState, id: PassId) -> Result<String, AssessError> {
        let path = state.config.data_dir.join(reference(id));
        tokio::fs::create_dir_all(state.config.data_dir.join("assessment-snapshots")).await?;
        let bytes = serde_json::to_vec(self)?;
        let mut file = tokio::fs::OpenOptions::new()
            .mode(0o600)
            .create_new(true)
            .write(true)
            .open(path)
            .await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        Ok(crate::review_loop::handin::hash(bytes))
    }

    pub async fn read(state: &AppState, pass: &AssessmentPass) -> Result<Self, AssessError> {
        if pass.input.snapshot_ref != reference(pass.id) {
            return Err(AssessError::Snapshot);
        }
        let bytes = tokio::fs::read(state.config.data_dir.join(reference(pass.id))).await?;
        if crate::review_loop::handin::hash(&bytes) != pass.input.input_hash {
            return Err(AssessError::Snapshot);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn own_reply(&self, url: &str) -> bool {
        let confirmed = crate::github::threads::github_web_url(url)
            .is_some_and(|url| self.own_reply_urls.contains(&url));
        confirmed
            || host::all_comments(&self.data).any(|comment| {
                host::url(comment) == url
                    && self.own_reply_texts.contains(host::body(comment).trim())
            })
    }

    pub async fn ids(
        &mut self,
        state: &AppState,
        pr: i64,
        summary: &contracts::api::pr::PrSummary,
        snapshot: Option<&crate::review_loop::observe::Snapshot>,
    ) -> Result<(), AssessError> {
        self.data.ids.next_id = self
            .items
            .iter()
            .map(|i| i.no)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let decisions = state.db.review_loop().pr_decisions(pr).await?;
        for row in &decisions {
            for effect in &row.decision.effects {
                use contracts::api::review_loop::{EffectKind, ReportOutcome};
                if matches!(effect.kind, EffectKind::Reply | EffectKind::PrComment) {
                    self.own_reply_texts
                        .extend(effect.params.text.iter().cloned());
                    self.own_reply_urls.extend(
                        effect
                            .reports
                            .iter()
                            .filter(|report| report.outcome == ReportOutcome::Done)
                            .filter_map(|report| report.reference.as_deref())
                            .filter_map(crate::github::threads::github_web_url),
                    );
                }
            }
        }
        let standing = crate::review_loop::standing(&decisions);
        let mut code = super::CodeReader::new(state, summary);
        let description = snapshot
            .and_then(|s| s.description.as_deref())
            .and_then(super::clean::description);
        let purpose = super::purpose_hash(
            &self.native.title,
            description
                .as_deref()
                .or(self.native.description.as_deref()),
        );
        for item in &self.items {
            if item.status_comment
                || item.kind == CommentItemKind::PrQuestion
                || item.gone_at.is_some()
            {
                continue;
            }
            let marked = item.active_mark.is_some();
            let item_id = item.id.to_string();
            let stored = standing.get(&item_id).filter(|_| !marked);
            let decision = stored.and_then(|d| {
                let kind =
                    serde_json::from_value(serde_json::to_value(d.decision.kind).ok()?).ok()?;
                let (target_pr, target_url, title) = match &d.decision.target {
                    Some(DecisionTarget::ExistingPr { number }) => (Some(*number), None, None),
                    Some(DecisionTarget::FollowUpPr { title }) => (None, None, title.clone()),
                    Some(DecisionTarget::UpstreamIssue { url }) => (None, url.clone(), None),
                    Some(DecisionTarget::Todo { title }) => (None, None, Some(title.clone())),
                    None => (None, None, None),
                };
                let proposal = contracts::agent::comment_report::ProposedDecision {
                    kind,
                    target_pr,
                    target_url: target_url.clone(),
                    title: title.clone(),
                    fixed_in: None,
                };
                if !host::valid_proposal(&proposal) {
                    tracing::debug!(
                        item_no = item.no,
                        kind = ?d.decision.kind,
                        "standing decision omitted from assessment ids: required target missing"
                    );
                    return None;
                }
                Some(StandingDecision {
                    kind: RecordedDecisionKind::Decision(kind),
                    target_pr,
                    target_url,
                    title,
                })
            });
            if !marked {
                let current = super::fingerprint(
                    item,
                    &summary.head_sha,
                    code.lines(item).await.as_deref(),
                    &purpose,
                    snapshot.and_then(|s| s.item_context.get(&item.stable_key)),
                );
                self.fingerprints.insert(item_id.clone(), current.clone());
                if let Some(stored) = stored {
                    let previous = state.db.review_loop().latest_assessment(item.id).await?;
                    self.standing.insert(
                        item_id,
                        FrozenDecision {
                            id: stored.decision.id,
                            kind: stored.decision.kind,
                            target: stored.decision.target.clone(),
                            changed: previous.is_none_or(|p| p.fingerprint != current),
                        },
                    );
                }
            }
            let key = if item.kind == CommentItemKind::Thread {
                host::threads(&self.data)
                    .find(|t| {
                        t["id"] == item.stable_key
                            || item.url.as_ref().is_some_and(|url| {
                                t.pointer("/comments/nodes/0/url")
                                    .or_else(|| t.pointer("/comments/0/url"))
                                    .and_then(serde_json::Value::as_str)
                                    == Some(url.as_str())
                            })
                    })
                    .and_then(|t| t["id"].as_str())
                    .unwrap_or(&item.stable_key)
                    .to_owned()
            } else {
                item.stable_key.clone()
            };
            self.data.ids.items.push(PrDataItem {
                id: format!("CMT-{:03}", item.no),
                key,
                source_kind: Some(match item.kind {
                    CommentItemKind::Thread => contracts::agent::comment_report::SourceKind::Inline,
                    CommentItemKind::ReviewBody => {
                        contracts::agent::comment_report::SourceKind::ReviewBody
                    }
                    _ => contracts::agent::comment_report::SourceKind::PrComment,
                }),
                // Shared sources can be sent for a sibling. Keep the marked point's identity for reuse.
                quote: (marked || super::assessable(item) || stored.is_some())
                    .then(|| item.quote.clone().unwrap_or_else(|| item.body.clone())),
                decision,
            });
        }
        self.purpose_fingerprint = Some(purpose);
        Ok(())
    }
}
