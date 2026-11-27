//! Repeats (plan §2.5, RQ3, spike S13-1 E2/K1/K2, S13-4): when a decided item is raised again its decision is
//! carried, unless the bot edited the finding text since (normalised hash) or a later commit changed the code at
//! the decided lines; then it is asked again. Code changes are found by mapping the decided range forward through
//! the diff between the decision's head and the PR's head; a mapping that cannot be made counts as changed.

use analysis::diff::{FileDiff, FileStatus};
use contracts::api::review_loop::{
    AuthorKind, BotThreadStatus, CarryEvidence, CommentItemKind, ReaskReason,
};
use contracts::common::LineRange;
use contracts::ids::CommitSha;
use db::repo::review_loop::{ItemRecord, ObservedItem};

/// How a stored thread item is raised again by what this read observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Raised {
    /// A bot's repeat: a new thread carries its key (the URL changes), the bot removed its status block or rewrote
    /// its finding in place (normalised text), or the thread was unresolved again; the carry-over rules apply.
    Repeat,
    /// A person's thread unresolved again or raised in a new thread: not a repeat, the owner is asked again.
    Reopened,
}

/// Whether, and how, the stored item is raised again by what this read observed.
pub fn raised(previous: &ItemRecord, observed: &ObservedItem) -> Option<Raised> {
    if previous.kind != CommentItemKind::Thread {
        return None;
    }
    let new_thread = previous.url.is_some() && previous.url != observed.url;
    let status_removed = previous.github.bot_status.is_some_and(|status| {
        status != BotThreadStatus::Disputed && observed.github.bot_status.is_none()
    });
    let rewritten = previous.body_hash != observed.body_hash;
    let unresolved_again =
        previous.github.resolved == Some(true) && observed.github.resolved == Some(false);
    let bot = previous.author_kind == AuthorKind::Bot;
    if bot && (new_thread || status_removed || rewritten || unresolved_again) {
        Some(Raised::Repeat)
    } else if !bot && (new_thread || unresolved_again) {
        Some(Raised::Reopened)
    } else {
        None
    }
}

/// Where the decided lines are at the head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapping {
    /// No change touched them; they are here now.
    Unchanged(LineRange),
    /// A change touched them.
    Changed,
    /// The mapping could not be made (no diff, renamed, binary or deleted file).
    Unknown,
}

/// Maps `range` of `path` at the old commit through the diff `files` (old → new) to the new commit.
pub fn map_range(files: &[FileDiff], path: &str, range: LineRange) -> Mapping {
    let Some(file) = files
        .iter()
        .find(|file| file.old_path.as_deref().unwrap_or(&file.path) == path)
    else {
        return Mapping::Unchanged(range);
    };
    if file.binary || matches!(file.status, FileStatus::Renamed | FileStatus::Added) {
        return Mapping::Unknown;
    }
    if file.status == FileStatus::Deleted {
        return Mapping::Changed;
    }
    let (start, end) = (i64::from(range.start_line), i64::from(range.end_line));
    let mut delta: i64 = 0;
    for hunk in &file.hunks {
        let (base_start, base_len) = (i64::from(hunk.base.start), i64::from(hunk.base.len));
        let grown = i64::from(hunk.head.len) - base_len;
        if base_len == 0 {
            // Lines added after base line `base_start`.
            if start <= base_start && base_start < end {
                return Mapping::Changed;
            }
            if base_start < start {
                delta += grown;
            }
            continue;
        }
        let base_end = base_start + base_len - 1;
        if base_start <= end && start <= base_end {
            return Mapping::Changed;
        }
        if base_end < start {
            delta += grown;
        }
    }
    match (u32::try_from(start + delta), u32::try_from(end + delta)) {
        (Ok(start_line), Ok(end_line)) if start_line >= 1 => Mapping::Unchanged(LineRange {
            start_line,
            end_line,
        }),
        _ => Mapping::Unknown,
    }
}

/// What a raised-again item gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The decision is carried.
    Carry(CarryEvidence),
    /// Asked again for this reason.
    Reask(ReaskReason, CarryEvidence),
}

/// The decision's facts the rules compare.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decided<'a> {
    /// Normalised text hash at decision time.
    pub body_hash: &'a str,
    /// Head at decision time.
    pub head: &'a CommitSha,
    /// Path and lines at decision time.
    pub location: Option<(&'a str, LineRange)>,
}

/// The rules of plan §2.5 for an item raised again: (a) the bot edited the text since the decision; (b) the code
/// at the decided lines changed (`mapping` is the forward mapping, `None` when the head did not move).
pub fn verdict(
    decided: &Decided<'_>,
    observed: &ObservedItem,
    mapping: Option<Mapping>,
) -> Verdict {
    let mut evidence = CarryEvidence {
        quote: None,
        url: observed.url.clone(),
        body_hash_before: Some(decided.body_hash.to_owned()),
        body_hash_after: Some(observed.body_hash.clone()),
        commit: None,
        lines: None,
    };
    if decided.body_hash != observed.body_hash {
        return Verdict::Reask(ReaskReason::BotEdited, evidence);
    }
    match mapping {
        None => {
            evidence.lines = decided.location.map(|(_, lines)| lines);
            Verdict::Carry(evidence)
        }
        Some(Mapping::Unchanged(lines)) => {
            evidence.commit = Some(observed.head.clone());
            evidence.lines = Some(lines);
            Verdict::Carry(evidence)
        }
        Some(Mapping::Changed | Mapping::Unknown) => {
            evidence.commit = Some(observed.head.clone());
            Verdict::Reask(ReaskReason::CodeChanged, evidence)
        }
    }
}

#[cfg(test)]
mod tests {
    use analysis::diff::{Hunk, LineSpan};
    use contracts::api::review_loop::{ItemGithubFacts, ItemLocation, ItemState};
    use contracts::ids::{CommentItemId, HunkId};
    use contracts::time::Timestamp;

    use super::*;

    fn range(start_line: u32, end_line: u32) -> LineRange {
        LineRange {
            start_line,
            end_line,
        }
    }

    fn hunk(base: (u32, u32), head: (u32, u32)) -> Hunk {
        Hunk {
            id: HunkId("h1".into()),
            base: LineSpan {
                start: base.0,
                len: base.1,
            },
            head: LineSpan {
                start: head.0,
                len: head.1,
            },
            added: Vec::new(),
            removed: Vec::new(),
        }
    }

    fn file(status: FileStatus, hunks: Vec<Hunk>) -> FileDiff {
        FileDiff {
            path: "src/rpc/client.rs".into(),
            old_path: None,
            status,
            binary: false,
            hunks,
        }
    }

    #[test]
    fn ranges_map_forward_through_the_diff() {
        let path = "src/rpc/client.rs";
        let decided = range(96, 99);
        assert_eq!(
            map_range(&[], path, decided),
            Mapping::Unchanged(decided),
            "file untouched"
        );
        let above = file(
            FileStatus::Modified,
            vec![hunk((10, 2), (10, 5)), hunk((50, 0), (54, 1))],
        );
        assert_eq!(
            map_range(&[above], path, decided),
            Mapping::Unchanged(range(100, 103)),
            "three lines replaced and one added above move the range by four"
        );
        let below = file(FileStatus::Modified, vec![hunk((120, 3), (120, 0))]);
        assert_eq!(
            map_range(&[below], path, decided),
            Mapping::Unchanged(decided)
        );
        for touching in [
            hunk((99, 1), (99, 2)),
            hunk((90, 7), (90, 1)),
            hunk((97, 0), (98, 2)),
        ] {
            let changed = file(FileStatus::Modified, vec![touching.clone()]);
            assert_eq!(
                map_range(&[changed], path, decided),
                Mapping::Changed,
                "{touching:?}"
            );
        }
        let after_last = file(FileStatus::Modified, vec![hunk((99, 0), (100, 1))]);
        assert_eq!(
            map_range(&[after_last], path, decided),
            Mapping::Unchanged(decided),
            "lines added after the range do not touch it"
        );
        assert_eq!(
            map_range(&[file(FileStatus::Deleted, Vec::new())], path, decided),
            Mapping::Changed
        );
        let mut renamed = file(FileStatus::Renamed, Vec::new());
        renamed.old_path = Some(path.into());
        renamed.path = "src/rpc/call.rs".into();
        assert_eq!(map_range(&[renamed], path, decided), Mapping::Unknown);
    }

    fn observed(body_hash: &str) -> ObservedItem {
        ObservedItem {
            kind: CommentItemKind::Thread,
            stable_key: "821d7cdc93fec6e1".into(),
            author: "thepastaclaw".into(),
            author_kind: AuthorKind::Bot,
            status_comment: false,
            url: Some("https://github.com/acme/node-client/pull/482#discussion_r2".into()),
            location: Some(ItemLocation {
                path: "src/rpc/client.rs".into(),
                lines: Some(range(96, 99)),
            }),
            title: "t".into(),
            body: "b".into(),
            body_hash: body_hash.into(),
            head: CommitSha("b".repeat(40)),
            round: Some(2),
            bot_severity: None,
            github: ItemGithubFacts {
                resolved: Some(false),
                resolved_by: None,
                outdated: Some(false),
                bot_status: None,
                edited_after_decision: false,
                replies: 0,
                marker: None,
            },
            outside: None,
        }
    }

    #[test]
    fn bot_edit_or_code_change_reasks() {
        let head = CommitSha("a".repeat(40));
        let decided = Decided {
            body_hash: "h1",
            head: &head,
            location: Some(("src/rpc/client.rs", range(96, 99))),
        };
        assert!(matches!(
            verdict(&decided, &observed("h1"), None),
            Verdict::Carry(_)
        ));
        assert!(matches!(
            verdict(
                &decided,
                &observed("h1"),
                Some(Mapping::Unchanged(range(100, 103)))
            ),
            Verdict::Carry(CarryEvidence {
                quote: None,
                lines: Some(LineRange {
                    start_line: 100,
                    ..
                }),
                ..
            })
        ));
        let Verdict::Reask(reason, evidence) = verdict(&decided, &observed("h2"), None) else {
            panic!("a text edit asks again");
        };
        assert_eq!(reason, ReaskReason::BotEdited);
        assert_eq!(
            (
                evidence.body_hash_before.as_deref(),
                evidence.body_hash_after.as_deref()
            ),
            (Some("h1"), Some("h2"))
        );
        for mapping in [Mapping::Changed, Mapping::Unknown] {
            let Verdict::Reask(reason, evidence) =
                verdict(&decided, &observed("h1"), Some(mapping))
            else {
                panic!("{mapping:?} asks again");
            };
            assert_eq!(reason, ReaskReason::CodeChanged);
            assert_eq!(evidence.commit, Some(CommitSha("b".repeat(40))));
        }
    }

    #[test]
    fn raised_again_by_a_bot_repeat_or_reopened_by_a_person() {
        let item = ItemRecord {
            from_api: false,
            import_candidate: None,
            active_mark: None,
            source_id: None,
            quote: None,
            duplicate_of: None,
            id: CommentItemId::new_random(),
            pull_request_id: 1,
            no: 1,
            kind: CommentItemKind::Thread,
            stable_key: "821d7cdc93fec6e1".into(),
            author: "thepastaclaw".into(),
            author_kind: AuthorKind::Bot,
            status_comment: false,
            url: observed("h").url,
            location: None,
            title: "t".into(),
            body: "b".into(),
            body_hash: "h".into(),
            first_seen_head: CommitSha("a".repeat(40)),
            first_round: Some(1),
            bot_severity: None,
            gates: Vec::new(),
            github: observed("h").github,
            state: ItemState::Decided,
            revision: 2,
            outside: None,
            repeat_of: None,
            gone_at: None,
            updated_at: Timestamp::now(),
        };
        assert_eq!(raised(&item, &observed("h")), None);
        let mut moved = observed("h");
        moved.url = Some("https://github.com/acme/node-client/pull/482#discussion_r9".into());
        assert_eq!(
            raised(&item, &moved),
            Some(Raised::Repeat),
            "a new thread with the decided key"
        );
        let mut withdrawn = item.clone();
        withdrawn.github.bot_status = Some(BotThreadStatus::Withdrawn);
        assert_eq!(
            raised(&withdrawn, &observed("h")),
            Some(Raised::Repeat),
            "status block removed in place"
        );
        assert_eq!(
            raised(&item, &observed("h2")),
            Some(Raised::Repeat),
            "the bot rewrote its finding in place"
        );
        let mut resolved = item.clone();
        resolved.github.resolved = Some(true);
        assert_eq!(raised(&resolved, &observed("h")), Some(Raised::Repeat));
        let mut human = resolved.clone();
        human.author_kind = AuthorKind::Human;
        assert_eq!(
            raised(&human, &observed("h")),
            Some(Raised::Reopened),
            "a person unresolving his thread is no repeat"
        );
        human.github.resolved = Some(false);
        assert_eq!(
            raised(&human, &observed("h2")),
            None,
            "a person's edit is not a repeat"
        );
        let mut body = item;
        body.kind = CommentItemKind::ReviewBody;
        assert_eq!(raised(&body, &moved), None);
    }
}
