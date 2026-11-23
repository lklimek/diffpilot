//! What diffpilot reads back after agents act (plan §3.4, §4.1): a read of the pull request [`REPORT_READ_DELAY`]
//! after an agent's `done` report, and the fact "seen on GitHub" for each done effect a read finds there.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Duration;

use contracts::api::review_loop::{Effect, EffectKind, EffectState, ReportOutcome};
use contracts::common::PrRef;
use contracts::time::Timestamp;
use db::error::DbError;

use super::{LoopError, PrHandle, ReadOutcome, assess, standing};
use crate::github::review_loop::PrRead;
use crate::github::threads::github_web_url;
use crate::state::AppState;

/// How long after a `done` report the pull request is read again; reports in between share the read.
pub const REPORT_READ_DELAY: Duration = Duration::from_secs(10);

/// Pull requests with a read scheduled after a report.
static SCHEDULED: LazyLock<Mutex<HashSet<i64>>> = LazyLock::new(Mutex::default);

/// After an agent's `done` report on pull request `pull_request_id`: reads it from GitHub [`REPORT_READ_DELAY`]
/// later with the GitHub sign-in of `user` (the token's owner), which marks the effects it finds as seen.
pub fn read_after_report(state: &AppState, tenant: &str, user: &str, pull_request_id: i64) {
    if !SCHEDULED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(pull_request_id)
    {
        return;
    }
    let (state, tenant, user) = (state.clone(), tenant.to_owned(), user.to_owned());
    tokio::spawn(async move {
        tokio::time::sleep(REPORT_READ_DELAY).await;
        SCHEDULED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&pull_request_id);
        if let Err(error) = read(&state, &tenant, &user, pull_request_id).await {
            tracing::warn!(
                ?error,
                pr = pull_request_id,
                "the pull request could not be read from GitHub after an agent's report; the next read marks what agents did"
            );
        }
    });
}

async fn read(state: &AppState, tenant: &str, user: &str, id: i64) -> Result<(), LoopError> {
    let stored = state.db.pull_requests().get(id).await?.pr;
    let pr = PrRef {
        repo: stored.repo,
        number: stored.number,
    };
    let read = match super::read_if_due(state, tenant, user, &pr, true).await {
        Ok(read) => read,
        // A read moments ago already saw the report's effect.
        Err(LoopError::TooSoon) => return Ok(()),
        Err(error) => return Err(error),
    };
    if let ReadOutcome::Read { new } = read.outcome
        && new > 0
        && let Err(error) =
            assess::request_pass_for(state, id, assess::PassReason::NewComments, user).await
    {
        tracing::warn!(
            ?error,
            pr = id,
            "assessment pass for comments found after an agent's report could not be requested"
        );
    }
    Ok(())
}

/// The link of the effect's latest `done` report, as a GitHub web link.
fn reported_link(effect: &Effect) -> Option<String> {
    effect
        .reports
        .iter()
        .rev()
        .find(|report| report.outcome == ReportOutcome::Done)
        .and_then(|report| report.reference.as_deref())
        .and_then(github_web_url)
}

/// Whether `read` shows the done effect on GitHub: its thread resolved, its reply or PR-level comment at the reported
/// link, its review dismissed. Other kinds are not visible in a pull request read.
fn found(effect: &Effect, read: &PrRead) -> bool {
    let thread = || {
        let id = effect.params.thread_id.as_deref();
        read.threads
            .iter()
            .find(|thread| Some(thread.id.as_str()) == id)
    };
    match effect.kind {
        EffectKind::Resolve => thread().is_some_and(|thread| thread.is_resolved),
        EffectKind::Reply => {
            reported_link(effect).is_some_and(|link| {
                thread().is_some_and(|thread| {
                    thread.comments.nodes.iter().any(|comment| {
                        github_web_url(&comment.url).as_deref() == Some(link.as_str())
                    })
                })
            })
        }
        EffectKind::PrComment => reported_link(effect).is_some_and(|link| {
            read.comments
                .iter()
                .any(|comment| github_web_url(&comment.url).as_deref() == Some(link.as_str()))
        }),
        EffectKind::DismissReview => read.reviews.iter().any(|review| {
            Some(review.id.as_str()) == effect.params.review_id.as_deref()
                && review.state == "DISMISSED"
        }),
        EffectKind::Fix | EffectKind::FollowUpPr | EffectKind::UpstreamIssue | EffectKind::Todo => {
            false
        }
    }
}

/// Records "seen on GitHub" for every done effect of the pull request's standing decisions that `read` shows.
pub(super) async fn mark(
    state: &AppState,
    handle: &PrHandle,
    read: &PrRead,
) -> Result<(), DbError> {
    let rl = state.db.review_loop();
    let decisions = rl.pr_decisions(handle.id).await?;
    let now = Timestamp::now();
    let mut seen = 0_u32;
    for stored in standing(&decisions).values() {
        for effect in &stored.decision.effects {
            if effect.state == EffectState::Done
                && effect.seen_on_github_at.is_none()
                && found(effect, read)
            {
                rl.effect_seen_on_github(effect.id, now).await?;
                seen += 1;
            }
        }
    }
    if seen > 0 {
        tracing::debug!(
            pr = handle.id,
            seen,
            "effects agents reported found on GitHub"
        );
    }
    Ok(())
}
