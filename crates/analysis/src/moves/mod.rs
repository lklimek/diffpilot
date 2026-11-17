//! Moved-block detection and whitespace-only changes: removed and added lines are compared by token-normalised
//! text (runs of whitespace collapsed), candidate pairs share a line, `similar` aligns them, and a block is a
//! cluster of aligned lines. Renamed files are moved blocks of the whole file.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use contracts::agent::analyse_flow::MovedFrom;
use contracts::api::code::MovedLines;
use contracts::api::tour::{IdenticalMove, MoveFact};
use contracts::common::{FileRange, LineRange, LineSide, ProvenanceBadge};
use contracts::ids::CommitSha;
use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffOp, capture_diff_slices};

use crate::diff::{FileDiff, FileStatus, Hunk};

/// A block moved between base and head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedBlock {
    /// Original block at the base commit.
    pub from: FileRange,
    /// New block at the head commit.
    pub to: FileRange,
    /// Equal after whitespace normalisation (badge "text identical"; a quiet row).
    pub identical: bool,
    /// Head lines that differ from the original.
    pub differing_head_lines: Vec<u32>,
    /// Base lines of the original that differ from the new block.
    pub differing_base_lines: Vec<u32>,
    /// Lines of `from` the pull request removed; a moved step claiming this block accounts for them.
    pub removed_base_lines: Vec<u32>,
}

/// Deterministic move and whitespace facts of a pull request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovesReport {
    /// Moved blocks.
    pub moved: Vec<MovedBlock>,
    /// Changed head ranges whose change is whitespace only (kept in the agent's hunks, marked as a fact).
    pub whitespace_only: Vec<FileRange>,
}

/// A moved step's claim confirmed against a deterministic moved block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmedMove {
    /// The move fact of the step.
    pub fact: MoveFact,
    /// Removed base lines of the claimed original: covered by the moved step, also across files.
    pub covered_base_lines: Vec<u32>,
}

/// Move detection failed.
#[derive(Debug, thiserror::Error)]
pub enum MovesError {
    /// `git` failed.
    #[error("git diff --color-moved failed: {message}")]
    Git {
        /// Plain git error output.
        message: String,
    },
    /// Running `git` failed.
    #[error("could not run git")]
    Io(#[from] std::io::Error),
    /// Declared but not implemented yet.
    #[error("the analysis operation {operation} is not implemented yet")]
    NotImplemented {
        /// The operation.
        operation: &'static str,
    },
}

/// Moved blocks (`git diff --color-moved` candidates confirmed by token-normalised hashes) and whitespace-only
/// changes.
pub fn detect(
    repo: &Path,
    base: &CommitSha,
    head: &CommitSha,
    files: &[FileDiff],
) -> Result<MovesReport, MovesError> {
    let _ = (repo, base, head, files);
    Err(MovesError::NotImplemented {
        operation: "moves::detect",
    })
}

/// Fewest aligned non-blank lines in a moved block.
const MIN_LINES: usize = 3;
/// Fewest alphanumeric characters in the aligned lines of a block (git's `--color-moved` default).
const MIN_ALNUM: usize = 20;
/// Most non-blank lines between two aligned stretches of one block.
const MAX_GAP: usize = 3;
/// A same-file block whose new place is at most this many lines from its old place is an edit, not a move.
const IN_PLACE_LINES: u32 = 3;
/// A line occurring in more added runs than this does not pair runs (`}`-like noise).
const MAX_ANCHOR_RUNS: usize = 64;
/// Fewest alphanumeric characters of a line that pairs runs.
const MIN_ANCHOR_ALNUM: usize = 3;
/// Longest run (non-blank lines) aligned with another.
const MAX_RUN_LINES: usize = 20_000;

/// Moved blocks and whitespace-only changes of `files`. `text` gives a side's file text; sides without text
/// (binary, not read, or excluded by the caller) take no part.
pub fn analyse<'t>(
    files: &[FileDiff],
    text: &dyn Fn(LineSide, &str) -> Option<&'t str>,
) -> MovesReport {
    let texts = Texts::new(files, text);
    let mut moved = blocks(files, &texts);
    moved.extend(renamed_files(files, &texts));
    moved.sort_by(|a, b| {
        (&a.to.file, a.to.range, &a.from.file, a.from.range).cmp(&(
            &b.to.file,
            b.to.range,
            &b.from.file,
            b.from.range,
        ))
    });
    let mut whitespace_only = Vec::new();
    for (index, file) in files.iter().enumerate() {
        for hunk in &file.hunks {
            let removed = normalised(texts.get(index, LineSide::Base), &hunk.removed);
            let added = normalised(texts.get(index, LineSide::Head), &hunk.added);
            if removed.is_some() && removed == added {
                whitespace_only.extend(hunk_range(file, hunk));
            }
        }
    }
    MovesReport {
        moved,
        whitespace_only,
    }
}

impl MovesReport {
    /// Moves whose text is identical: the quiet rows.
    pub fn identical(&self) -> Vec<IdenticalMove> {
        self.moved
            .iter()
            .filter(|block| block.identical)
            .map(|block| IdenticalMove {
                from: block.from.clone(),
                to: block.to.clone(),
                badge: ProvenanceBadge::TextIdentical,
            })
            .collect()
    }

    /// Moved blocks of the head file `path` overlapping lines `from..=to` (the lines endpoint).
    pub fn moved_lines(&self, path: &str, from: u32, to: u32) -> Vec<MovedLines> {
        let window = LineRange {
            start_line: from,
            end_line: to,
        };
        self.moved
            .iter()
            .filter(|block| block.to.file == path && overlaps(block.to.range, window))
            .map(|block| MovedLines {
                range: block.to.range,
                differing: block.differing_head_lines.clone(),
            })
            .collect()
    }

    /// Confirms a moved step at head lines `step` of `step_file` claiming `claim` as its original; `None` when no
    /// deterministic block moved code from the claimed lines into the step.
    pub fn confirm(
        &self,
        step_file: &str,
        step: LineRange,
        claim: &MovedFrom,
    ) -> Option<ConfirmedMove> {
        let claimed = LineRange {
            start_line: claim.base_start,
            end_line: claim.base_end,
        };
        let block = self
            .moved
            .iter()
            .filter(|block| {
                block.from.file == claim.file
                    && block.to.file == step_file
                    && overlaps(block.from.range, claimed)
                    && overlaps(block.to.range, step)
            })
            .min_by_key(|block| block.from.range.end_line - block.from.range.start_line)?;
        let differing_head = within(&block.differing_head_lines, step).len();
        let differing_base = within(&block.differing_base_lines, claimed).len();
        let differing = differing_head.max(differing_base);
        let from = LineRange {
            start_line: claimed.start_line.max(block.from.range.start_line),
            end_line: claimed.end_line.min(block.from.range.end_line),
        };
        Some(ConfirmedMove {
            fact: MoveFact {
                from: file_range(&claim.file, LineSide::Base, from.start_line, from.end_line),
                badge: (block.identical && differing == 0)
                    .then_some(ProvenanceBadge::TextIdentical),
                differing_lines: u32::try_from(differing).unwrap_or(u32::MAX),
            },
            covered_base_lines: within(&block.removed_base_lines, claimed),
        })
    }
}

/// The lines of each file side, split once.
struct Texts<'t> {
    sides: Vec<[Option<Vec<&'t str>>; 2]>,
}

impl<'t> Texts<'t> {
    fn new(files: &[FileDiff], text: &dyn Fn(LineSide, &str) -> Option<&'t str>) -> Self {
        let sides = files
            .iter()
            .map(|file| {
                [LineSide::Base, LineSide::Head].map(|side| {
                    file.path_on(side)
                        .filter(|_| !file.binary)
                        .and_then(|path| text(side, path))
                        .map(|text| text.lines().collect())
                })
            })
            .collect();
        Self { sides }
    }

    fn get(&self, file: usize, side: LineSide) -> Option<&[&'t str]> {
        let slot = usize::from(side == LineSide::Head);
        self.sides.get(file)?.get(slot)?.as_deref()
    }
}

/// Removed or added lines of one hunk: non-blank line numbers and their normalised text.
struct Run {
    file: usize,
    hunk: usize,
    lines: Vec<u32>,
    norm: Vec<String>,
}

/// A candidate block before overlap resolution.
struct Candidate {
    from_file: usize,
    to_file: usize,
    base: Vec<u32>,
    head: Vec<u32>,
    differing_base: Vec<u32>,
    differing_head: Vec<u32>,
    aligned: usize,
}

fn runs(files: &[FileDiff], texts: &Texts<'_>, side: LineSide) -> Vec<Run> {
    let mut out = Vec::new();
    for (file, diff) in files.iter().enumerate() {
        let Some(text) = texts.get(file, side) else {
            continue;
        };
        for (hunk, h) in diff.hunks.iter().enumerate() {
            let numbers = match side {
                LineSide::Base => &h.removed,
                LineSide::Head => &h.added,
            };
            let mut run = Run {
                file,
                hunk,
                lines: Vec::new(),
                norm: Vec::new(),
            };
            for &number in numbers {
                let norm = line_at(text, number).map(normalise).unwrap_or_default();
                if !norm.is_empty() {
                    run.lines.push(number);
                    run.norm.push(norm);
                }
            }
            if !run.lines.is_empty() && run.lines.len() <= MAX_RUN_LINES {
                out.push(run);
            }
        }
    }
    out
}

fn blocks(files: &[FileDiff], texts: &Texts<'_>) -> Vec<MovedBlock> {
    let removed = runs(files, texts, LineSide::Base);
    let added = runs(files, texts, LineSide::Head);
    let mut anchors: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, run) in added.iter().enumerate() {
        for norm in run
            .norm
            .iter()
            .filter(|norm| alnum(norm) >= MIN_ANCHOR_ALNUM)
        {
            let list = anchors.entry(norm.as_str()).or_default();
            if list.last() != Some(&index) {
                list.push(index);
            }
        }
    }
    let mut candidates = Vec::new();
    for old in &removed {
        let paired: BTreeSet<usize> = old
            .norm
            .iter()
            .filter_map(|norm| anchors.get(norm.as_str()))
            .filter(|list| list.len() <= MAX_ANCHOR_RUNS)
            .flatten()
            .copied()
            .collect();
        for new in paired.into_iter().filter_map(|index| added.get(index)) {
            if (old.file, old.hunk) != (new.file, new.hunk) {
                candidates.extend(clusters(files, old, new));
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.aligned.cmp(&a.aligned).then_with(|| {
            (a.from_file, &a.base, a.to_file, &a.head).cmp(&(
                b.from_file,
                &b.base,
                b.to_file,
                &b.head,
            ))
        })
    });
    let mut used: BTreeMap<(LineSide, usize), Vec<(u32, u32)>> = BTreeMap::new();
    let mut out = Vec::new();
    for candidate in candidates {
        let (Some(&base_start), Some(&base_end), Some(&head_start), Some(&head_end)) = (
            candidate.base.first(),
            candidate.base.last(),
            candidate.head.first(),
            candidate.head.last(),
        ) else {
            continue;
        };
        let (Some(from), Some(to)) = (
            files[candidate.from_file].path_on(LineSide::Base),
            files[candidate.to_file].path_on(LineSide::Head),
        ) else {
            continue;
        };
        let spans = [
            (
                (LineSide::Base, candidate.from_file),
                (base_start, base_end),
            ),
            ((LineSide::Head, candidate.to_file), (head_start, head_end)),
        ];
        let free = spans.iter().all(|(key, (start, end))| {
            used.get(key)
                .is_none_or(|taken| taken.iter().all(|&(s, e)| *end < s || e < *start))
        });
        if !free {
            continue;
        }
        for (key, span) in spans {
            used.entry(key).or_default().push(span);
        }
        out.push(MovedBlock {
            from: file_range(from, LineSide::Base, base_start, base_end),
            to: file_range(to, LineSide::Head, head_start, head_end),
            identical: candidate.differing_base.is_empty() && candidate.differing_head.is_empty(),
            differing_head_lines: candidate.differing_head,
            differing_base_lines: candidate.differing_base,
            removed_base_lines: (base_start..=base_end).collect(),
        });
    }
    out
}

/// Blocks of `new` aligned with `old`: stretches of equal lines with gaps of at most `MAX_GAP` lines.
fn clusters(files: &[FileDiff], old: &Run, new: &Run) -> Vec<Candidate> {
    let equal = capture_diff_slices(Algorithm::Patience, &old.norm, &new.norm)
        .into_iter()
        .filter_map(|op| match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => Some((old_index, new_index, len)),
            _ => None,
        });
    let mut groups: Vec<Vec<(usize, usize, usize)>> = Vec::new();
    for stretch in equal {
        let joins = groups
            .last()
            .and_then(|group| group.last())
            .is_some_and(|&(o, n, len)| {
                stretch.0.saturating_sub(o + len) <= MAX_GAP
                    && stretch.1.saturating_sub(n + len) <= MAX_GAP
            });
        match groups.last_mut() {
            Some(group) if joins => group.push(stretch),
            _ => groups.push(vec![stretch]),
        }
    }
    groups
        .into_iter()
        .filter_map(|group| {
            let (&(old_first, new_first, _), &(old_last, new_last, last_len)) =
                (group.first()?, group.last()?);
            let aligned_old: BTreeSet<usize> =
                group.iter().flat_map(|&(o, _, len)| o..o + len).collect();
            let aligned_new: BTreeSet<usize> =
                group.iter().flat_map(|&(_, n, len)| n..n + len).collect();
            let chars: usize = aligned_old
                .iter()
                .filter_map(|&i| old.norm.get(i))
                .map(|norm| alnum(norm))
                .sum();
            let unaligned = |run: &Run, span: std::ops::Range<usize>, aligned: &BTreeSet<usize>| {
                span.filter(|i| !aligned.contains(i))
                    .filter_map(|i| run.lines.get(i).copied())
                    .collect::<Vec<u32>>()
            };
            let old_span = old_first..old_last + last_len;
            let new_span = new_first..new_last + last_len;
            let differing_base = unaligned(old, old_span.clone(), &aligned_old);
            let differing_head = unaligned(new, new_span.clone(), &aligned_new);
            let aligned = aligned_old.len();
            let enough = aligned >= MIN_LINES
                && chars >= MIN_ALNUM
                && aligned >= differing_base.len()
                && aligned >= differing_head.len();
            let head = new.lines.get(new_span)?.to_vec();
            (enough && !in_place(files, old, new.file, &head)).then(|| Candidate {
                from_file: old.file,
                to_file: new.file,
                base: old
                    .lines
                    .get(old_span)
                    .map(<[u32]>::to_vec)
                    .unwrap_or_default(),
                head,
                differing_base,
                differing_head,
                aligned,
            })
        })
        .collect()
}

/// Lines aligned in the same file next to where they were removed: an edit (re-indent), not a move.
fn in_place(files: &[FileDiff], old: &Run, new_file: usize, head: &[u32]) -> bool {
    let (Some(hunk), Some(&first)) = (
        files
            .get(old.file)
            .and_then(|file| file.hunks.get(old.hunk)),
        head.first(),
    ) else {
        return false;
    };
    let marker = if hunk.head.len == 0 {
        hunk.head.start
    } else {
        hunk.head.start.saturating_sub(1)
    };
    new_file == old.file && (marker + 1).abs_diff(first) <= IN_PLACE_LINES
}

/// A renamed file is a moved block of the whole file; identical only when no line changed.
fn renamed_files(files: &[FileDiff], texts: &Texts<'_>) -> Vec<MovedBlock> {
    let mut out = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let (FileStatus::Renamed, Some(base_path), Some(base), Some(head)) = (
            file.status,
            file.path_on(LineSide::Base),
            texts.get(index, LineSide::Base),
            texts.get(index, LineSide::Head),
        ) else {
            continue;
        };
        let (Ok(base_end @ 1..), Ok(head_end @ 1..)) =
            (u32::try_from(base.len()), u32::try_from(head.len()))
        else {
            continue;
        };
        // Lines of hunks that change more than whitespace, blank lines left out.
        let changes: Vec<&Hunk> = file
            .hunks
            .iter()
            .filter(|hunk| {
                normalised(Some(base), &hunk.removed) != normalised(Some(head), &hunk.added)
            })
            .collect();
        let differing = |lines: &[&str], numbers: Vec<u32>| -> Vec<u32> {
            numbers
                .into_iter()
                .filter(|&line| line_at(lines, line).is_some_and(|text| !text.trim().is_empty()))
                .collect()
        };
        let added = changes.iter().flat_map(|hunk| hunk.added.iter().copied());
        let removed = changes.iter().flat_map(|hunk| hunk.removed.iter().copied());
        out.push(MovedBlock {
            from: file_range(base_path, LineSide::Base, 1, base_end),
            to: file_range(&file.path, LineSide::Head, 1, head_end),
            identical: file.hunks.is_empty(),
            differing_head_lines: differing(head, added.collect()),
            differing_base_lines: differing(base, removed.collect()),
            removed_base_lines: file
                .hunks
                .iter()
                .flat_map(|hunk| hunk.removed.clone())
                .collect(),
        });
    }
    out
}

/// The non-blank lines of `numbers`, normalised; `None` when the text or a line is missing.
fn normalised(text: Option<&[&str]>, numbers: &[u32]) -> Option<Vec<String>> {
    let text = text?;
    let mut out = Vec::with_capacity(numbers.len());
    for &number in numbers {
        let norm = normalise(line_at(text, number)?);
        if !norm.is_empty() {
            out.push(norm);
        }
    }
    Some(out)
}

/// The changed range of a hunk on the head side, or on the base side when it only removes lines.
fn hunk_range(file: &FileDiff, hunk: &Hunk) -> Option<FileRange> {
    let (side, span) = if hunk.head.len > 0 {
        (LineSide::Head, hunk.head)
    } else {
        (LineSide::Base, hunk.base)
    };
    if span.len == 0 {
        return None;
    }
    let path = file.path_on(side)?;
    Some(file_range(
        path,
        side,
        span.start,
        span.start + span.len - 1,
    ))
}

fn line_at<'t>(lines: &[&'t str], number: u32) -> Option<&'t str> {
    lines
        .get(usize::try_from(number).ok()?.checked_sub(1)?)
        .copied()
}

/// Whitespace runs collapsed to one space, ends trimmed.
fn normalise(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn alnum(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

fn within(lines: &[u32], range: LineRange) -> Vec<u32> {
    lines
        .iter()
        .copied()
        .filter(|line| (range.start_line..=range.end_line).contains(line))
        .collect()
}

fn file_range(file: &str, side: LineSide, start_line: u32, end_line: u32) -> FileRange {
    FileRange {
        file: file.to_owned(),
        side,
        range: LineRange {
            start_line,
            end_line,
        },
    }
}

fn overlaps(a: LineRange, b: LineRange) -> bool {
    a.start_line <= b.end_line && b.start_line <= a.end_line
}

#[cfg(test)]
mod tests {
    use contracts::ids::HunkId;

    use super::*;
    use crate::diff::LineSpan;

    fn hunk(base: (u32, u32), head: (u32, u32)) -> Hunk {
        let base = LineSpan {
            start: base.0,
            len: base.1,
        };
        let head = LineSpan {
            start: head.0,
            len: head.1,
        };
        Hunk {
            id: HunkId("h".to_owned()),
            base,
            head,
            added: (head.start..head.start + head.len).collect(),
            removed: (base.start..base.start + base.len).collect(),
        }
    }

    fn file(path: &str, old_path: Option<&str>, status: FileStatus, hunks: Vec<Hunk>) -> FileDiff {
        FileDiff {
            path: path.to_owned(),
            old_path: old_path.map(str::to_owned),
            status,
            binary: false,
            hunks,
        }
    }

    fn range(start_line: u32, end_line: u32) -> LineRange {
        LineRange {
            start_line,
            end_line,
        }
    }

    type Sources = HashMap<(LineSide, String), String>;

    fn src(side: LineSide, path: &str, lines: &[&str]) -> ((LineSide, String), String) {
        let text = lines.iter().map(|line| format!("{line}\n")).collect();
        ((side, path.to_owned()), text)
    }

    fn sources<const N: usize>(entries: [((LineSide, String), String); N]) -> Sources {
        entries.into_iter().collect()
    }

    fn run(files: &[FileDiff], sources: &Sources) -> MovesReport {
        analyse(files, &|side, path| {
            sources.get(&(side, path.to_owned())).map(String::as_str)
        })
    }

    const FUNCTION: [&str; 5] = [
        "fn build(config: &Config) -> Request {",
        "    let url = config.endpoint.join(PATH);",
        "    let request = Request::new(url).header(CLIENT, NAME);",
        "    request.with_timeout(config.timeout)",
        "}",
    ];

    fn indented(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| format!("    {line}")).collect()
    }

    #[test]
    fn identical_cross_file_move_with_reindent_is_text_identical() {
        let moved = indented(&FUNCTION);
        let mut head_b = vec!["mod x {"];
        head_b.extend(moved.iter().map(String::as_str));
        head_b.push("}");
        let mut base_a = vec!["use x;", ""];
        base_a.extend(FUNCTION);
        base_a.extend(["", "fn other() {}"]);
        let files = vec![
            file(
                "src/a.rs",
                None,
                FileStatus::Modified,
                vec![hunk((3, 6), (2, 0))],
            ),
            file(
                "src/b.rs",
                None,
                FileStatus::Added,
                vec![hunk((0, 0), (1, 7))],
            ),
        ];
        let sources = sources([
            src(LineSide::Base, "src/a.rs", &base_a),
            src(LineSide::Head, "src/a.rs", &["use x;", "", "fn other() {}"]),
            src(LineSide::Head, "src/b.rs", &head_b),
        ]);
        let report = run(&files, &sources);
        assert_eq!(report.moved.len(), 1, "{report:?}");
        let block = &report.moved[0];
        assert_eq!(
            (block.from.clone(), block.to.clone()),
            (
                file_range("src/a.rs", LineSide::Base, 3, 7),
                file_range("src/b.rs", LineSide::Head, 2, 6)
            ),
            "the trailing blank line is not part of the block"
        );
        assert!(block.identical);
        assert!(block.differing_head_lines.is_empty() && block.differing_base_lines.is_empty());
        assert_eq!(block.removed_base_lines, [3, 4, 5, 6, 7]);
        assert_eq!(
            report.identical(),
            [IdenticalMove {
                from: block.from.clone(),
                to: block.to.clone(),
                badge: ProvenanceBadge::TextIdentical
            }]
        );
        assert_eq!(
            report.moved_lines("src/b.rs", 1, 3),
            [MovedLines {
                range: range(2, 6),
                differing: vec![]
            }]
        );
        assert!(report.moved_lines("src/b.rs", 7, 9).is_empty());
        let claim = MovedFrom {
            file: "src/a.rs".to_owned(),
            base_start: 3,
            base_end: 7,
        };
        let confirmed = report
            .confirm("src/b.rs", range(2, 6), &claim)
            .expect("confirmed");
        assert_eq!(confirmed.fact.badge, Some(ProvenanceBadge::TextIdentical));
        assert_eq!(confirmed.fact.differing_lines, 0);
    }

    #[test]
    fn differing_cross_file_move_covers_its_removed_lines() {
        let mut changed = FUNCTION.to_vec();
        changed[3] = "    request.with_timeout(config.timeout * 2)";
        let mut head_b = vec!["use y;"];
        head_b.extend(&changed);
        let files = vec![
            file(
                "src/a.rs",
                None,
                FileStatus::Deleted,
                vec![hunk((1, 5), (0, 0))],
            ),
            file(
                "src/b.rs",
                None,
                FileStatus::Modified,
                vec![hunk((1, 0), (2, 5))],
            ),
        ];
        let sources = sources([
            src(LineSide::Base, "src/a.rs", &FUNCTION),
            src(LineSide::Base, "src/b.rs", &["use y;"]),
            src(LineSide::Head, "src/b.rs", &head_b),
        ]);
        let report = run(&files, &sources);
        assert_eq!(report.moved.len(), 1, "{report:?}");
        let block = &report.moved[0];
        assert!(!block.identical);
        assert_eq!(block.differing_head_lines, [5]);
        assert_eq!(block.differing_base_lines, [4]);
        assert!(
            report.identical().is_empty(),
            "differing moves are no quiet rows"
        );
        let claim = MovedFrom {
            file: "src/a.rs".to_owned(),
            base_start: 1,
            base_end: 5,
        };
        let confirmed = report
            .confirm("src/b.rs", range(2, 6), &claim)
            .expect("confirmed");
        assert_eq!(
            confirmed.fact,
            MoveFact {
                from: file_range("src/a.rs", LineSide::Base, 1, 5),
                badge: None,
                differing_lines: 1
            }
        );
        assert_eq!(
            confirmed.covered_base_lines,
            [1, 2, 3, 4, 5],
            "the removed lines of the other file belong to the moved step"
        );
        let elsewhere = MovedFrom {
            file: "src/c.rs".to_owned(),
            ..claim
        };
        assert_eq!(report.confirm("src/b.rs", range(2, 6), &elsewhere), None);
    }

    #[test]
    fn renamed_file_is_a_moved_block_and_confirms_a_claimed_part() {
        let base: Vec<String> = (1..=20)
            .map(|n| format!("const VALUE_{n}: u32 = {n};"))
            .collect();
        let mut head = base.clone();
        head[9] = "const VALUE_10: u32 = 100;".to_owned();
        head.insert(0, "// moved here".to_owned());
        let files = vec![file(
            "src/new.rs",
            Some("src/old.rs"),
            FileStatus::Renamed,
            vec![hunk((0, 0), (1, 1)), hunk((10, 1), (11, 1))],
        )];
        let base: Vec<&str> = base.iter().map(String::as_str).collect();
        let head: Vec<&str> = head.iter().map(String::as_str).collect();
        let report = run(
            &files,
            &sources([
                src(LineSide::Base, "src/old.rs", &base),
                src(LineSide::Head, "src/new.rs", &head),
            ]),
        );
        assert_eq!(report.moved.len(), 1, "{report:?}");
        let block = &report.moved[0];
        assert_eq!(block.from, file_range("src/old.rs", LineSide::Base, 1, 20));
        assert_eq!(block.to, file_range("src/new.rs", LineSide::Head, 1, 21));
        assert!(!block.identical);
        assert_eq!(block.differing_head_lines, [1, 11]);
        assert_eq!(block.removed_base_lines, [10]);
        let claim = MovedFrom {
            file: "src/old.rs".to_owned(),
            base_start: 5,
            base_end: 20,
        };
        let confirmed = report
            .confirm("src/new.rs", range(6, 21), &claim)
            .expect("confirmed");
        assert_eq!(confirmed.fact.differing_lines, 1);
        assert_eq!(confirmed.fact.badge, None);
        assert_eq!(confirmed.fact.from.range, range(5, 20));
        assert_eq!(confirmed.covered_base_lines, [10]);

        let pure = vec![file(
            "src/new.rs",
            Some("src/old.rs"),
            FileStatus::Renamed,
            vec![],
        )];
        let report = run(
            &pure,
            &sources([
                src(LineSide::Base, "src/old.rs", &["a", "b"]),
                src(LineSide::Head, "src/new.rs", &["a", "b"]),
            ]),
        );
        assert_eq!(
            report.identical().len(),
            1,
            "a pure rename is an identical move"
        );
    }

    #[test]
    fn in_place_reindent_and_short_coincidences_are_not_moves() {
        let body = &FUNCTION[1..4];
        let wrapped = indented(body);
        let mut head: Vec<&str> = vec!["if ready {"];
        head.extend(wrapped.iter().map(String::as_str));
        head.push("}");
        // Wrapping code in a block re-indents it in place, also when the diff splits it in two hunks.
        let split = vec![file(
            "src/a.rs",
            None,
            FileStatus::Modified,
            vec![hunk((1, 3), (0, 0)), hunk((3, 0), (2, 3))],
        )];
        let in_place = sources([
            src(LineSide::Base, "src/a.rs", body),
            src(LineSide::Head, "src/a.rs", &head),
        ]);
        assert!(run(&split, &in_place).moved.is_empty());
        // The same lines added far from where they were removed are a move.
        let mut far: Vec<&str> = vec!["x();"; 9];
        far.extend(wrapped.iter().map(String::as_str));
        let moved = vec![file(
            "src/a.rs",
            None,
            FileStatus::Modified,
            vec![hunk((1, 3), (0, 0)), hunk((3, 0), (10, 3))],
        )];
        let far = sources([
            src(LineSide::Base, "src/a.rs", body),
            src(LineSide::Head, "src/a.rs", &far),
        ]);
        assert_eq!(run(&moved, &far).identical().len(), 1);

        // Two equal lines in otherwise different code are no move.
        let files = vec![
            file(
                "src/a.rs",
                None,
                FileStatus::Modified,
                vec![hunk((1, 3), (0, 0))],
            ),
            file(
                "src/b.rs",
                None,
                FileStatus::Modified,
                vec![hunk((0, 0), (1, 3))],
            ),
        ];
        let sources = sources([
            src(
                LineSide::Base,
                "src/a.rs",
                &[FUNCTION[1], FUNCTION[2], "send_once(request);"],
            ),
            src(LineSide::Head, "src/a.rs", &[]),
            src(LineSide::Base, "src/b.rs", &[]),
            src(
                LineSide::Head,
                "src/b.rs",
                &[FUNCTION[1], FUNCTION[2], "loop { retry(request); }"],
            ),
        ]);
        assert!(run(&files, &sources).moved.is_empty());
    }

    #[test]
    fn whitespace_only_hunks_are_reported_on_their_side() {
        let files = vec![file(
            "src/a.rs",
            None,
            FileStatus::Modified,
            vec![
                hunk((1, 1), (1, 1)),
                hunk((3, 1), (3, 1)),
                hunk((5, 1), (4, 0)),
            ],
        )];
        let sources = sources([
            src(
                LineSide::Base,
                "src/a.rs",
                &["let a =  1;", "x", "let b = 2;", "y", ""],
            ),
            src(
                LineSide::Head,
                "src/a.rs",
                &["    let a = 1;", "x", "let b = 3;", "y"],
            ),
        ]);
        assert_eq!(
            run(&files, &sources).whitespace_only,
            [
                file_range("src/a.rs", LineSide::Head, 1, 1),
                file_range("src/a.rs", LineSide::Base, 5, 5)
            ]
        );
        assert!(
            run(&files, &HashMap::new()).whitespace_only.is_empty(),
            "no text, no fact"
        );
    }
}
