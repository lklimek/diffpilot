//! R1b paging (plan §E.5 E5-4, E5-5): filters, the opaque cursor bound to the snapshot revision, the revision
//! itself and the weak `ETag` of a page.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use contracts::api::dashboard::{AwaitingReason, GithubKind, ItemKind, ItemQuery};
use db::repo::dashboard::RefreshRecord;
use sha2::{Digest, Sha256};

use super::{Snapshot, View, ViewItem};

/// Revision of a user without a readable snapshot.
pub const NO_SNAPSHOT: &str = "none";

/// A cursor R1b cannot use.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    /// Not a cursor of this server.
    #[error("the cursor is not valid")]
    Malformed,
    /// A cursor of an older revision: read again from the start (409 `cursor_expired`).
    #[error("the cursor belongs to an older revision of the dashboard")]
    Expired,
}

fn short_hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .take(12)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Opaque revision of `snapshot`, held by `record`, as `view` shows it: changes when the snapshot or its ranking
/// changes (state, attempt and judgement counts; WP11-JUDGE records the counts as judgements land), when TODOs are
/// hidden, when GitHub items are re-read, when the shown plug-in statements change (ADR-022) and when the user's
/// snoozes or repository live status change.
pub fn revision(record: &RefreshRecord, snapshot: &Snapshot, view: &View) -> String {
    let mut source = format!(
        "{}|{:?}|{:?}|{}|{}|{}|{}|{:?}|{:?}|{}|{:?}",
        record.id,
        record.state,
        record.ranking_state,
        record.ranking_attempt,
        record.counts.judged,
        record.counts.reused,
        record.counts.left_provisional,
        record.scanned_at,
        record.finished_at,
        snapshot.todos_hidden,
        snapshot.mentions.iter().map(|m| &m.id).collect::<Vec<_>>(),
    );
    if let Some(at) = snapshot.prs_reread_at {
        source.push_str(&format!("|reread {at}"));
    }
    if let Some(at) = snapshot.last_event_at {
        source.push_str(&format!("|webhook {at}"));
    }
    if !snapshot.snoozes.is_empty() {
        let snoozed: Vec<_> = snapshot
            .snoozes
            .iter()
            .map(|s| (s.key.as_str(), s.until))
            .collect();
        source.push_str(&format!("|snoozed {snoozed:?}"));
    }
    let notes: Vec<_> = view
        .items
        .iter()
        .filter(|item| !item.item.plugin_notes.is_empty())
        .map(|item| (&item.item.key, &item.item.plugin_notes))
        .collect();
    let plugin_waits: Vec<_> = view
        .awaiting
        .iter()
        .filter(|item| matches!(item.reason, AwaitingReason::Plugin { .. }))
        .map(|item| (&item.id, &item.reason, &item.waiting_on))
        .collect();
    if !notes.is_empty() || !plugin_waits.is_empty() {
        let ready: Vec<_> = view.ready_to_merge.iter().map(|item| &item.id).collect();
        source.push_str(&format!("|plugins {notes:?} {plugin_waits:?} {ready:?}"));
    }
    let live: Vec<_> = view
        .items
        .iter()
        .filter_map(|item| item.item.live.map(|live| (&item.item.key, live)))
        .collect();
    if !live.is_empty() {
        source.push_str(&format!("|live {live:?}"));
    }
    let judgements: Vec<_> = view
        .items
        .iter()
        .filter_map(|item| {
            item.item
                .judgement
                .map(|judgement| (&item.item.key, judgement))
        })
        .collect();
    source.push_str(&format!("|judgements {judgements:?}"));
    format!("r-{}", short_hash(source.as_bytes()))
}

fn cursor(revision: &str, offset: usize) -> String {
    URL_SAFE_NO_PAD.encode(format!("{revision}~{offset}"))
}

fn offset(cursor: &str, revision: &str) -> Result<usize, CursorError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| CursorError::Malformed)?;
    let text = String::from_utf8(decoded).map_err(|_| CursorError::Malformed)?;
    let (of, at) = text.rsplit_once('~').ok_or(CursorError::Malformed)?;
    let at = at.parse().map_err(|_| CursorError::Malformed)?;
    if of == revision {
        Ok(at)
    } else {
        Err(CursorError::Expired)
    }
}

fn wanted(item: &ViewItem, query: &ItemQuery) -> bool {
    let item = &item.item;
    let kind_ok = match (item.kind, query.kind) {
        (ItemKind::Todo, _) => query.todos != Some(false),
        (_, None) => true,
        (ItemKind::Pr, Some(kind)) => kind == GithubKind::Pr,
        (ItemKind::Issue, Some(kind)) => kind == GithubKind::Issue,
    };
    let repo_ok = query.repo.as_deref().is_none_or(|wanted| {
        item.repo.as_ref().is_some_and(|repo| {
            wanted.eq_ignore_ascii_case(&format!("{}/{}", repo.owner, repo.name))
        })
    });
    kind_ok
        && repo_ok
        && query.tier.is_none_or(|tier| item.score.tier == tier)
        && query.state.is_none_or(|state| item.score.state == state)
        && query
            .updated_since
            .is_none_or(|since| item.updated_at >= since)
}

/// The page `query` asks for from `items` (rank order) of `revision`, and the cursor of the next page.
pub fn page(
    items: &[ViewItem],
    query: &ItemQuery,
    revision: &str,
) -> Result<(Vec<ViewItem>, Option<String>), CursorError> {
    let start = query
        .cursor
        .as_deref()
        .map(|c| offset(c, revision))
        .transpose()?
        .unwrap_or(0);
    let limit = query
        .limit
        .unwrap_or(ItemQuery::DEFAULT_LIMIT)
        .clamp(1, ItemQuery::MAX_LIMIT) as usize;
    let matching: Vec<&ViewItem> = items.iter().filter(|i| wanted(i, query)).collect();
    let end = start.saturating_add(limit).min(matching.len());
    let next = (end < matching.len()).then(|| cursor(revision, end));
    let page = matching
        .get(start.min(matching.len())..end)
        .unwrap_or_default()
        .iter()
        .map(|item| (*item).clone())
        .collect();
    Ok((page, next))
}

/// Weak `ETag` of a response body.
pub fn etag(body: &[u8]) -> String {
    format!("W/\"{}\"", short_hash(body))
}

/// Whether an `If-None-Match` header value names `etag` (weak comparison, lists and `*`).
pub fn etag_matches(if_none_match: &str, etag: &str) -> bool {
    let bare = |tag: &str| tag.trim().trim_start_matches("W/").to_owned();
    if_none_match
        .split(',')
        .any(|tag| tag.trim() == "*" || bare(tag) == bare(etag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_of_another_revision_is_expired_and_garbage_is_malformed() {
        let next = cursor("r-1", 100);
        assert_eq!(offset(&next, "r-1"), Ok(100));
        assert_eq!(offset(&next, "r-2"), Err(CursorError::Expired));
        assert_eq!(offset("%%%", "r-1"), Err(CursorError::Malformed));
        assert_eq!(
            offset(&URL_SAFE_NO_PAD.encode("r-1"), "r-1"),
            Err(CursorError::Malformed)
        );
    }

    #[test]
    fn etag_matching_is_weak_and_takes_lists() {
        let tag = etag(b"body");
        assert!(tag.starts_with("W/\""), "{tag}");
        assert!(etag_matches(&tag, &tag));
        assert!(etag_matches(&tag.replace("W/", ""), &tag));
        assert!(etag_matches(&format!("\"x\", {tag}"), &tag));
        assert!(etag_matches("*", &tag));
        assert!(!etag_matches("W/\"other\"", &tag));
        assert_ne!(etag(b"body"), etag(b"body2"));
    }
}
