//! Transactional dispositions derived from a primary decision or grounded GitHub evidence.

use std::collections::HashMap;

use contracts::api::review_loop::{
    CommentItemKind, DecideRequest, DecisionKind, DecisionTarget, DecisionVia, EffectKind,
    ImportedGithubDecision, ItemState,
};
use contracts::common::PrRef;
use contracts::ids::{CommentItemId, DecisionId};
use contracts::time::Timestamp;
use db::repo::review_loop::{
    ActiveMark, DecideOutcome, ImportCandidate, ItemRecord, NewDecision, NewOrigin, StoredDecision,
};

use super::{Loaded, LoopError, PrHandle, effects, new_effects};
use crate::state::AppState;

/// Applies eligible imports and followers to the current PR view.
pub(crate) async fn propagate(state: &AppState, tenant: &str, pr: i64) -> Result<(), LoopError> {
    let stored = state.db.pull_requests().get(pr).await?;
    let handle = PrHandle {
        id: pr,
        pr: PrRef {
            repo: stored.pr.repo,
            number: stored.pr.number,
        },
    };
    let outcome = state
        .db
        .review_loop()
        .decide_planned(tenant, pr, Timestamp::now(), |view| {
            let loaded = Loaded::from_view(handle, view).ok_or(LoopError::NotFound)?;
            let mut rows = Vec::new();
            loaded.append_propagated(&mut rows, true);
            Ok::<_, LoopError>(rows)
        })
        .await?;
    if matches!(outcome, DecideOutcome::Stored(_)) {
        state.feed.wake_sync();
    }
    Ok(())
}

enum Primary<'a> {
    Pending(&'a NewDecision),
    Existing(&'a StoredDecision),
    Marked(&'a ActiveMark),
}

struct Choice<'a> {
    user: &'a str,
    kind: DecisionKind,
    target: Option<DecisionTarget>,
    reason: Option<String>,
    resolve: bool,
    imported: bool,
}

impl Primary<'_> {
    fn id(&self) -> Option<DecisionId> {
        match self {
            Self::Pending(_) | Self::Marked(_) => None,
            Self::Existing(row) => Some(row.decision.id),
        }
    }

    fn choice(&self) -> Option<Choice<'_>> {
        match self {
            Self::Pending(row) => {
                let imported = match &row.origin {
                    Some(NewOrigin::Imported(import)) if import.author_withdrawal => return None,
                    Some(NewOrigin::Imported(_)) => true,
                    Some(NewOrigin::Follows(_)) => return None,
                    None => false,
                };
                Some(Choice {
                    user: &row.user_id,
                    kind: row.kind,
                    target: row.target.clone(),
                    reason: row.reason.clone(),
                    resolve: row.resolve_ticked,
                    imported,
                })
            }
            Self::Marked(active) => Some(Choice {
                user: &active.user_id,
                kind: DecisionKind::NotAReviewPoint,
                target: None,
                reason: None,
                resolve: false,
                imported: false,
            }),
            Self::Existing(row)
                if !row.author_withdrawal
                    && row.primary_decision.is_none()
                    && !row
                        .decision
                        .imported
                        .as_ref()
                        .is_some_and(|e| e.relay.is_some()) =>
            {
                let d = &row.decision;
                Some(Choice {
                    user: &row.user_id,
                    kind: d.kind,
                    target: d.target.clone(),
                    reason: d.reason.clone(),
                    resolve: d.resolve_ticked,
                    imported: d.via == DecisionVia::ImportedGithub,
                })
            }
            _ => None,
        }
    }
}

impl Loaded {
    pub(super) fn current_import<'a>(&self, record: &'a ItemRecord) -> Option<&'a ImportCandidate> {
        if record.active_mark.is_some() {
            return None;
        }
        let import = record.import_candidate.as_ref()?;
        let data = self.snapshot.as_ref()?.pr_data.as_ref()?;
        let version = super::assess::imports::version(
            data,
            record.url.as_deref()?,
            &import.evidence.evidence_url,
        )?;
        (record.gone_at.is_none() && !record.status_comment && import.version == version)
            .then_some(import)
    }

    pub(super) fn import_proposal(&self, record: &ItemRecord) -> Option<ImportedGithubDecision> {
        let import = self.current_import(record)?;
        let kind = import.kind.review_kind()?;
        (!import.binding || import.evidence.relay.is_some()).then(|| ImportedGithubDecision {
            kind,
            by: import.evidence.by.clone(),
            at: import.evidence.at,
            evidence_url: import.evidence.evidence_url.clone(),
        })
    }

    pub(super) fn follower_link(&self, item: &ItemRecord) -> Option<CommentItemId> {
        item.duplicate_of.filter(|primary| {
            *primary != item.id
                && item.kind != CommentItemKind::PrQuestion
                && !matches!(item.state, ItemState::Reopened | ItemState::AskedBack)
                && item.gone_at.is_none()
                && !item.status_comment
                && self.gates(item).is_empty()
        })
    }

    pub(super) fn append_propagated(&self, rows: &mut Vec<NewDecision>, from_assessment: bool) {
        let Ok(snapshot) = self.snapshot() else {
            return;
        };
        let standing = self.standing();
        let propagate_decisions = from_assessment
            || rows
                .iter()
                .any(|row| row.kind != DecisionKind::NotAReviewPoint);
        let explicit: std::collections::HashSet<_> = rows.iter().map(|r| r.item).collect();
        rows.extend(self.items.iter().filter_map(|item| {
            if !from_assessment
                || explicit.contains(&item.id)
                || standing.contains_key(&item.id.to_string())
            {
                return None;
            }
            let import = self
                .current_import(item)
                .filter(|import| import.binding && import.evidence.relay.is_none())?;
            // An undo must not automatically re-import the same GitHub statement.
            if self.decisions.iter().any(|old| {
                old.item == item.id
                    && (old
                        .decision
                        .undone_at
                        .is_some_and(|at| import.evidence.at <= at)
                        || old
                            .decision
                            .imported
                            .as_ref()
                            .is_some_and(|e| e.evidence_url == import.evidence.evidence_url))
            }) {
                return None;
            }
            Some(NewDecision {
                origin: Some(NewOrigin::Imported(Box::new(import.clone()))),
                item: item.id,
                item_revision: item.revision,
                user_id: import.user_id.clone(),
                kind: import.kind,
                reason: (import.kind == DecisionKind::Reject)
                    .then(|| import.evidence.quote.clone()),
                target: import.target.clone(),
                r#override: None,
                resolve_ticked: false,
                reply_text: None,
                comment: None,
                recommendation: None,
                via: DecisionVia::ImportedGithub,
                carried_from: None,
                head: contracts::ids::CommitSha(snapshot.head.clone()),
                effects: Vec::new(),
            })
        }));

        let pending: HashMap<_, _> = rows.iter().map(|row| (row.item, row)).collect();
        let scope = effects::PrScope {
            number: self.handle.pr.number,
            remaining: Vec::new(),
            stale_reviews: Vec::new(),
        };
        let followers: Vec<_> = self
            .items
            .iter()
            .filter_map(|item| {
                let primary_id = self.follower_link(item)?;
                if pending.contains_key(&item.id)
                    || standing
                        .get(&item.id.to_string())
                        .is_some_and(|d| d.primary_decision.is_none())
                {
                    return None;
                }
                let source = self.items.iter().find(|i| i.id == primary_id)?;
                let primary = if let Some(row) = pending.get(&primary_id) {
                    Primary::Pending(row)
                } else {
                    if source.gone_at.is_some() {
                        return None;
                    }
                    if let Some(active) = &source.active_mark {
                        if active.mark.follows.is_some() {
                            return None;
                        }
                        Primary::Marked(active)
                    } else {
                        if matches!(source.state, ItemState::Reopened | ItemState::AskedBack) {
                            return None;
                        }
                        Primary::Existing(standing.get(&primary_id.to_string())?)
                    }
                };
                if let Some(id) = primary.id()
                    && self
                        .decisions
                        .iter()
                        .any(|row| row.item == item.id && row.primary_decision == Some(id))
                {
                    return None;
                }
                let Choice {
                    user,
                    kind,
                    target,
                    reason,
                    resolve,
                    imported,
                } = primary.choice()?;
                let mark = kind == DecisionKind::NotAReviewPoint;
                if let Some(active) = &item.active_mark
                    && (mark
                        || active.mark.follows != Some(source.no)
                        || !matches!(primary, Primary::Pending(_)))
                {
                    return None;
                }
                if (mark && item.state == ItemState::Outside) || (!mark && !propagate_decisions) {
                    return None;
                }
                let reason = if mark { None } else { reason };
                let request = DecideRequest {
                    kind,
                    reason: reason.clone(),
                    target: target.clone().or_else(|| {
                        (imported && kind == DecisionKind::Todo).then(|| DecisionTarget::Todo {
                            title: item.title.clone(),
                        })
                    }),
                    r#override: None,
                    resolve_ticked: resolve || (imported && kind == DecisionKind::Resolve),
                    reply_text: None,
                    comment: None,
                    item_revision: item.revision,
                    plan_digest: None,
                };
                let mut planned =
                    effects::plan(&self.effect_item(item).ok()?, &request, &scope).ok()?;
                if let Some(artefact) = planned
                    .effects
                    .iter()
                    .find(|effect| is_artefact(effect.kind))
                {
                    let artefact_seq = artefact.seq;
                    let link = source
                        .url
                        .as_deref()
                        .and_then(crate::github::threads::github_web_url)
                        .unwrap_or_else(|| snapshot.url.clone());
                    let text = format!("Duplicate of {link}");
                    planned.reply_text = Some(text.clone());
                    planned.effects.retain(|effect| !is_artefact(effect.kind));
                    for effect in &mut planned.effects {
                        if matches!(effect.kind, EffectKind::Reply | EffectKind::PrComment) {
                            effect.params.text = Some(text.clone());
                        }
                        effect.seq -= u32::from(effect.seq > artefact_seq);
                        effect.after = effect
                            .after
                            .filter(|seq| *seq != artefact_seq)
                            .map(|seq| seq - u32::from(seq > artefact_seq));
                    }
                }
                Some(NewDecision {
                    origin: Some(NewOrigin::Follows(primary_id)),
                    item: item.id,
                    item_revision: item.revision,
                    user_id: user.to_owned(),
                    kind,
                    reason,
                    target,
                    r#override: None,
                    resolve_ticked: !imported && resolve,
                    reply_text: if imported { None } else { planned.reply_text },
                    comment: None,
                    recommendation: self
                        .assessments
                        .get(&item.no)
                        .map(|a| a.assessment.recommendation.clone()),
                    via: DecisionVia::FollowsPrimary,
                    carried_from: None,
                    head: contracts::ids::CommitSha(snapshot.head.clone()),
                    effects: if imported {
                        Vec::new()
                    } else {
                        new_effects(planned.effects)
                    },
                })
            })
            .collect();
        rows.extend(followers);
    }
}

fn is_artefact(kind: EffectKind) -> bool {
    matches!(
        kind,
        EffectKind::Fix | EffectKind::FollowUpPr | EffectKind::UpstreamIssue | EffectKind::Todo
    )
}
