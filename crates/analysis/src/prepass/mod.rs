//! The deterministic pre-pass (brief §5 Pipeline 2) over plain inputs: classification by the tech plugin, moved
//! blocks, whitespace-only changes, counters, related repositories, the project-profile file and agent
//! instruction files; every fact carries the lines or paths it was computed from.

use std::collections::{BTreeMap, BTreeSet};

use contracts::api::tour::{
    IdenticalMove, NotAnalysedItem, NotAnalysedSection, ProfileSource, ProjectProfileStatus,
    SourceCheck,
};
use contracts::common::{FileRanges, LineSide, NotAnalysedCategory, ProvenanceBadge, RepoRef};
use contracts::ids::HunkId;
use serde::{Deserialize, Serialize};

use crate::counters::{self, CounterFacts};
use crate::diff::FileDiff;
use crate::lines::ranges;
use crate::moves::{self, MovedBlock, MovesReport};
use crate::related;
use crate::tech::{
    self, Classified, FileSource, Generic, OtherSide, PROFILE_PATH, TechError, TechPlugin,
};

pub mod batches;

/// Result of the pre-pass; stored as the analysis's `prepass` document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prepass {
    /// Disposition of every hunk, in diff order.
    pub hunks: Vec<HunkPrepass>,
    /// Changed lines left for the agent (both sides); every other non-blank changed line is quiet.
    pub agent_lines: Vec<FileRanges>,
    /// Not analysed sections (Tests, Docs & comments, Mechanical & generated), empty ones left out.
    pub not_analysed: Vec<NotAnalysedSection>,
    /// Moved blocks and whitespace-only changes.
    pub moves: MovesReport,
    /// Quiet moves (see [`Prepass::identical_moves`]).
    pub quiet_moves: Vec<IdenticalMove>,
    /// Changed lines whose only change is whitespace, per file: shown as unchanged code, context for the agent
    /// without coverage duty.
    pub whitespace: Vec<WhitespacePairs>,
    /// Counters with their evidence.
    pub counters: CounterFacts,
    /// Same-organisation repositories referenced by the base Cargo files.
    pub related_repos: Vec<RepoRef>,
    /// Text of the project profile file on the base ref, if it exists and is not blank.
    pub profile_file: Option<String>,
    /// Paths in the diff that look like agent instruction files (threat model T14).
    pub agent_instruction_files: Vec<String>,
    /// File sides never rendered and never given to the agent.
    pub not_rendered: Vec<NotRendered>,
}

/// The whitespace-only changed lines of one file (see [`tech::whitespace_pairs`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhitespacePairs {
    /// Path at the base commit.
    pub base: String,
    /// Path at the head commit.
    pub head: String,
    /// (base line, head line), in diff order.
    pub pairs: Vec<(u32, u32)>,
}

/// What happens to a hunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "category")]
pub enum Disposition {
    /// Given to the agent (also when some of its lines are context without coverage duty).
    Agent,
    /// Recognised by a path or line rule.
    NotAnalysed(NotAnalysedCategory),
}

/// The disposition of one hunk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HunkPrepass {
    /// Hunk id.
    pub id: HunkId,
    /// What happens to it.
    pub disposition: Disposition,
}

/// A file side that is never rendered nor given to the agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotRendered {
    /// Repository-relative path on that side.
    pub file: String,
    /// The side.
    pub side: LineSide,
    /// Why.
    pub reason: NotRenderedReason,
}

/// Why a file side is not rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotRenderedReason {
    /// File exceeds the code reader's per-file byte limit.
    TooLarge,
    /// Generated file (brief §7).
    Generated,
    /// Binary file.
    Binary,
}

/// The pre-pass failed.
#[derive(Debug, thiserror::Error)]
pub enum PrepassError {
    /// The tech plugin failed.
    #[error("the not-analysed rules could not be applied")]
    Tech(#[from] TechError),
}

impl Prepass {
    /// Profile sources: the base-ref file and the MemCan memory; `memcan` is `None` when MemCan was not searched,
    /// else whether a profile memory was found.
    pub fn profile_status(&self, memcan: Option<bool>) -> ProjectProfileStatus {
        let file = self
            .profile_file
            .is_some()
            .then_some(ProfileSource::RepositoryFile);
        let memory = (memcan == Some(true)).then_some(ProfileSource::Memcan);
        ProjectProfileStatus {
            sources: file.into_iter().chain(memory).collect(),
            memcan: match memcan {
                Some(_) => SourceCheck::Checked,
                None => SourceCheck::NotChecked,
            },
        }
    }

    /// Quiet rows of moves with identical text: into a file other than a source that still exists, or a pure rename.
    /// Their lines are context for the agent without coverage duty; a reorder within one file is not quiet.
    pub fn identical_moves(&self) -> Vec<IdenticalMove> {
        self.quiet_moves.clone()
    }

    /// Whether `block` is a quiet move; the one decision for the pre-pass, the prompts and the Tour.
    pub fn is_quiet_move(&self, block: &MovedBlock) -> bool {
        self.quiet_moves
            .iter()
            .any(|quiet| quiet.from == block.from && quiet.to == block.to)
    }

    /// Lines that are context for the agent without coverage duty, per side and path, sorted: line-rule lines,
    /// whitespace-only lines and the ranges of quiet moves.
    pub fn context(&self) -> BTreeMap<(LineSide, String), Vec<u32>> {
        let mut out: BTreeMap<(LineSide, String), Vec<u32>> = BTreeMap::new();
        for file in &self.whitespace {
            for (side, path) in [(LineSide::Base, &file.base), (LineSide::Head, &file.head)] {
                out.entry((side, path.clone())).or_default().extend(
                    file.pairs
                        .iter()
                        .map(|&(base, head)| if side == LineSide::Base { base } else { head }),
                );
            }
        }
        for item in self.not_analysed.iter().flat_map(|s| &s.items) {
            if let (true, Some(ranges)) = (is_line_rule_item(item), &item.ranges) {
                out.entry((item.side, item.file.clone()))
                    .or_default()
                    .extend(ranges.iter().flat_map(|r| r.start_line..=r.end_line));
            }
        }
        for quiet in &self.quiet_moves {
            for end in [&quiet.from, &quiet.to] {
                out.entry((end.side, end.file.clone()))
                    .or_default()
                    .extend(end.range.start_line..=end.range.end_line);
            }
        }
        for lines in out.values_mut() {
            lines.sort_unstable();
            lines.dedup();
        }
        out
    }

    /// Whitespace-only changed lines of `path` on `side`, sorted: the code view shows them as unchanged lines.
    pub fn whitespace_lines(&self, side: LineSide, path: &str) -> Vec<u32> {
        let mut lines: Vec<u32> = self
            .whitespace
            .iter()
            .filter(|file| match side {
                LineSide::Base => file.base == path,
                LineSide::Head => file.head == path,
            })
            .flat_map(|file| &file.pairs)
            .map(|&(base, head)| if side == LineSide::Base { base } else { head })
            .collect();
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    /// The whitespace-only pairs (base line, head line) of `file`, for [`crate::lines::without_whitespace`].
    pub fn whitespace_pairs(&self, file: &FileDiff) -> Vec<(u32, u32)> {
        self.whitespace
            .iter()
            .filter(|pairs| {
                file.path_on(LineSide::Base) == Some(pairs.base.as_str())
                    && file.path_on(LineSide::Head) == Some(pairs.head.as_str())
            })
            .flat_map(|pairs| pairs.pairs.iter().copied())
            .collect()
    }

    /// Changed lines of `path` on `side` a comment or import rule covers: the main code view may hide them (test code
    /// stays shown).
    pub fn view_context(&self, side: LineSide, path: &str) -> Vec<u32> {
        let mut lines: Vec<u32> = self
            .not_analysed
            .iter()
            .flat_map(|s| &s.items)
            .filter(|item| {
                item.side == side
                    && item.file == path
                    && is_line_rule_item(item)
                    && item.rule != tech::TEST_ITEM_RULE
            })
            .flat_map(|item| item.ranges.iter().flatten())
            .flat_map(|r| r.start_line..=r.end_line)
            .collect();
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    /// `files` with only the hunks given to the agent, without the lines that are context (see
    /// [`Prepass::context`]); a hunk the pre-pass does not know stays (only a deterministic rule takes material out).
    /// Files left without hunks are dropped.
    pub fn agent_files(&self, files: &[FileDiff]) -> Vec<FileDiff> {
        let quiet: BTreeSet<&HunkId> = self
            .hunks
            .iter()
            .filter(|hunk| hunk.disposition != Disposition::Agent)
            .map(|hunk| &hunk.id)
            .collect();
        let context = self.context();
        let covered = |side: LineSide, path: Option<&str>, line: u32| {
            path.and_then(|path| context.get(&(side, path.to_owned())))
                .is_some_and(|lines| lines.binary_search(&line).is_ok())
        };
        files
            .iter()
            .filter_map(|file| {
                let hunks: Vec<_> = file
                    .hunks
                    .iter()
                    .filter(|hunk| !quiet.contains(&hunk.id))
                    .map(|hunk| {
                        let mut hunk = hunk.clone();
                        let (head, base) =
                            (file.path_on(LineSide::Head), file.path_on(LineSide::Base));
                        hunk.added
                            .retain(|&line| !covered(LineSide::Head, head, line));
                        hunk.removed
                            .retain(|&line| !covered(LineSide::Base, base, line));
                        hunk
                    })
                    .collect();
                (!hunks.is_empty()).then(|| FileDiff {
                    hunks,
                    ..file.clone()
                })
            })
            .collect()
    }

    /// Hunks of `files` the pre-pass knows where a context rule covers some changed lines and none is left for the
    /// agent (the rest are blank): comments, imports, test code, whitespace-only lines, identical moves. Prompts do
    /// not list them.
    pub fn context_only_hunks(&self, files: &[FileDiff]) -> BTreeSet<HunkId> {
        let known: BTreeSet<&HunkId> = self.hunks.iter().map(|hunk| &hunk.id).collect();
        let context = self.context();
        let agent: BTreeMap<(LineSide, &str), Vec<u32>> = self
            .agent_lines
            .iter()
            .map(|lines| {
                let numbers = lines
                    .ranges
                    .iter()
                    .flat_map(|r| r.start_line..=r.end_line)
                    .collect();
                ((lines.side, lines.file.as_str()), numbers)
            })
            .collect();
        let mut out = BTreeSet::new();
        for file in files {
            let sides = [LineSide::Head, LineSide::Base].map(|side| {
                let path = file.path_on(side);
                let context = path.and_then(|path| context.get(&(side, path.to_owned())));
                let agent = path.and_then(|path| agent.get(&(side, path)));
                (context, agent)
            });
            let has = |lines: Option<&Vec<u32>>, line: &u32| {
                lines.is_some_and(|lines| lines.binary_search(line).is_ok())
            };
            for hunk in file.hunks.iter().filter(|hunk| known.contains(&hunk.id)) {
                let changed = [&hunk.added, &hunk.removed];
                let some_context =
                    (0..2).any(|at| changed[at].iter().any(|line| has(sides[at].0, line)));
                let some_agent =
                    (0..2).any(|at| changed[at].iter().any(|line| has(sides[at].1, line)));
                if some_context && !some_agent {
                    out.insert(hunk.id.clone());
                }
            }
        }
        out
    }

    /// Changed lines without coverage duty, the coverage check's quiet input: the hunks the pre-pass took from the
    /// agent (Not analysed files) and the context lines.
    pub fn quiet_lines(&self, files: &[FileDiff]) -> Vec<FileRanges> {
        let quiet: BTreeSet<&HunkId> = self
            .hunks
            .iter()
            .filter(|hunk| hunk.disposition != Disposition::Agent)
            .map(|hunk| &hunk.id)
            .collect();
        let mut merged = self.context();
        for file in files {
            for hunk in file.hunks.iter().filter(|hunk| quiet.contains(&hunk.id)) {
                for (side, lines) in [
                    (LineSide::Head, &hunk.added),
                    (LineSide::Base, &hunk.removed),
                ] {
                    if let Some(path) = file.path_on(side) {
                        merged
                            .entry((side, path.to_owned()))
                            .or_default()
                            .extend(lines.iter().copied());
                    }
                }
            }
        }
        merged
            .into_iter()
            .filter(|(_, lines)| !lines.is_empty())
            .map(|((side, file), mut lines)| {
                lines.sort_unstable();
                lines.dedup();
                FileRanges {
                    side,
                    file,
                    ranges: ranges(&lines),
                }
            })
            .collect()
    }

    /// Whether `path` on `side` is never rendered (generated or binary).
    pub fn is_not_rendered(&self, side: LineSide, path: &str) -> bool {
        self.not_rendered
            .iter()
            .any(|item| item.side == side && item.file == path)
    }

    /// Not analysed sections with every binary file listed (in "Mechanical & generated" unless a path rule
    /// already lists it): a binary file is never silently absent.
    pub fn not_analysed_with_binaries(&self) -> Vec<NotAnalysedSection> {
        let mut sections = self.not_analysed.clone();
        for item in sections.iter_mut().flat_map(|section| &mut section.items) {
            item.renderable = Some(!self.is_not_rendered(item.side, &item.file));
            item.rule_kind = tech::rules()
                .iter()
                .find(|rule| rule.pattern == item.rule)
                .map(|rule| rule.kind);
        }
        let listed = |sections: &[NotAnalysedSection], item: &NotRendered| {
            sections
                .iter()
                .flat_map(|section| &section.items)
                .any(|listed| listed.side == item.side && listed.file == item.file)
        };
        for item in self
            .not_rendered
            .iter()
            .filter(|item| item.reason == NotRenderedReason::Binary)
        {
            if listed(&sections, item) {
                continue;
            }
            let entry = NotAnalysedItem {
                renderable: Some(false),
                rule_kind: Some(contracts::api::settings::RuleKind::Binary),
                file: item.file.clone(),
                side: item.side,
                ranges: None,
                badge: ProvenanceBadge::BinaryContent,
                rule: BINARY_RULE.to_owned(),
            };
            let category = NotAnalysedCategory::GeneratedMechanical;
            match sections
                .iter_mut()
                .find(|section| section.category == category)
            {
                Some(section) => {
                    let new_file = !section.items.iter().any(|other| other.file == entry.file);
                    section.file_count += u32::from(new_file);
                    section.items.push(entry);
                }
                None => sections.push(NotAnalysedSection {
                    category,
                    file_count: 1,
                    items: vec![entry],
                }),
            }
        }
        sections
    }
}

/// Rule text of a binary file in the Not analysed sections.
pub const BINARY_RULE: &str = "binary file (git)";

/// File sides the pre-pass reads: both sides of every changed text file, the Cargo files among `base_paths` (all
/// paths at the base commit) and the project profile at base. Read them, then call [`prepass`].
pub fn files_to_read(files: &[FileDiff], base_paths: &[String]) -> Vec<(LineSide, String)> {
    let mut out = BTreeSet::new();
    for file in files.iter().filter(|file| !file.binary) {
        for side in [LineSide::Base, LineSide::Head] {
            out.extend(file.path_on(side).map(|path| (side, path.to_owned())));
        }
    }
    let cargo = base_paths.iter().filter(|path| {
        matches!(
            tech::dependency_file(path),
            Some(tech::DependencyFile::CargoToml | tech::DependencyFile::CargoLock)
        )
    });
    out.extend(cargo.map(|path| (LineSide::Base, path.clone())));
    out.insert((LineSide::Base, PROFILE_PATH.to_owned()));
    out.into_iter().collect()
}

/// Runs the pre-pass over the diff `files` of `pr_repo`. `base_paths` lists every path at the base commit;
/// `text(side, path)` returns a file's text on that side, `None` when absent or unread (see [`files_to_read`]).
/// No IO happens here.
pub fn prepass<'t>(
    pr_repo: &RepoRef,
    files: &[FileDiff],
    base_paths: &[String],
    text: &dyn Fn(LineSide, &str) -> Option<&'t str>,
) -> Result<Prepass, PrepassError> {
    let mut classified = Vec::new();
    let mut not_rendered = Vec::new();
    for file in files {
        for side in [LineSide::Base, LineSide::Head] {
            let Some(path) = file.path_on(side) else {
                continue;
            };
            let changed = changed_lines(file, side);
            if changed.is_empty() && !file.binary {
                continue;
            }
            let other_side = match side {
                LineSide::Base => LineSide::Head,
                LineSide::Head => LineSide::Base,
            };
            let other = match (file.path_on(other_side), file.binary) {
                (None, _) => OtherSide::Absent,
                (Some(_), true) => OtherSide::Unknown,
                (Some(other), false) => {
                    text(other_side, other).map_or(OtherSide::Unknown, OtherSide::Text)
                }
            };
            let source = FileSource {
                path,
                side,
                text: if file.binary { None } else { text(side, path) },
                other,
                changed_lines: &changed,
                hunks: &file.hunks,
            };
            // The pull request's author controls a header it adds: it marks only files that were generated before.
            // A file the pull request adds is never generated by header (path rules still apply).
            let generated_before = file
                .path_on(LineSide::Base)
                .and_then(|base| text(LineSide::Base, base))
                .is_some_and(tech::has_generated_header);
            let items: Vec<Classified> = Generic
                .classify(&source)?
                .into_iter()
                .filter(|item| item.rule != tech::GENERATED_HEADER_RULE || generated_before)
                .collect();
            for item in items.iter().filter(|item| tech::is_generated(item)) {
                not_rendered.push(not_rendered_item(
                    &item.file,
                    side,
                    NotRenderedReason::Generated,
                ));
            }
            if file.binary {
                not_rendered.push(not_rendered_item(path, side, NotRenderedReason::Binary));
            }
            classified.extend(items);
        }
    }
    let whole_file: BTreeSet<(LineSide, &str)> = classified
        .iter()
        .filter(|item| !tech::is_line_rule(item))
        .map(|item| (item.side, item.file.as_str()))
        .collect();
    let visible = |side: LineSide, path: &str| {
        if whole_file.contains(&(side, path)) {
            None
        } else {
            text(side, path)
        }
    };
    let moves = moves::analyse(files, &visible);
    let quiet_moves: Vec<IdenticalMove> = moves
        .identical()
        .into_iter()
        .filter(|block| quiet_move(files, &block.from.file, &block.to.file))
        .collect();
    let whitespace: Vec<WhitespacePairs> = files
        .iter()
        .filter(|file| !file.binary)
        .filter_map(|file| {
            let (base, head) = (file.path_on(LineSide::Base)?, file.path_on(LineSide::Head)?);
            let (base_text, head_text) = (text(LineSide::Base, base)?, text(LineSide::Head, head)?);
            let pairs = tech::whitespace_pairs(head, base_text, head_text, &file.hunks);
            (!pairs.is_empty()).then(|| WhitespacePairs {
                base: base.to_owned(),
                head: head.to_owned(),
                pairs,
            })
        })
        .collect();
    let lines = Lines::new(files, &classified, &moves, &whitespace, text);
    let not_analysed = sections(files, &lines.classified(&classified));
    let counters = counters::compute(files, &classified, text);
    let related_repos = related::from_cargo(pr_repo, base_paths, text);
    let profile_file = text(LineSide::Base, PROFILE_PATH)
        .filter(|profile| !profile.trim().is_empty())
        .map(str::to_owned);
    let instruction: BTreeSet<&str> = files
        .iter()
        .flat_map(|file| [file.path_on(LineSide::Base), file.path_on(LineSide::Head)])
        .flatten()
        .filter(|path| tech::is_agent_instruction_file(path))
        .collect();
    Ok(Prepass {
        hunks: lines.hunks,
        agent_lines: lines.agent,
        not_analysed,
        moves,
        quiet_moves,
        whitespace,
        counters,
        related_repos,
        profile_file,
        agent_instruction_files: instruction.into_iter().map(str::to_owned).collect(),
        not_rendered,
    })
}

/// Quiet disposition of a changed line; by priority (`rank`): whole-file rule, identical move, whitespace only, line
/// rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quiet {
    LineRule,
    Whitespace,
    Move,
    WholeFile(NotAnalysedCategory),
}

impl Quiet {
    fn rank(self) -> u8 {
        match self {
            Self::LineRule => 0,
            Self::Whitespace => 1,
            Self::Move => 2,
            Self::WholeFile(_) => 3,
        }
    }
}

/// Section order of the Not analysed sections.
const CATEGORIES: [NotAnalysedCategory; 3] = [
    NotAnalysedCategory::Tests,
    NotAnalysedCategory::DocsComments,
    NotAnalysedCategory::GeneratedMechanical,
];

/// Dispositions of the changed lines and hunks.
struct Lines {
    hunks: Vec<HunkPrepass>,
    agent: Vec<FileRanges>,
    /// Line-rule lines that stay quiet (not inside an identical move), per side and path.
    line_rule: BTreeMap<(LineSide, String), BTreeSet<u32>>,
}

impl Lines {
    fn new<'t>(
        files: &[FileDiff],
        classified: &[Classified],
        moves: &MovesReport,
        whitespace: &[WhitespacePairs],
        text: &dyn Fn(LineSide, &str) -> Option<&'t str>,
    ) -> Self {
        let mut quiet: BTreeMap<(LineSide, &str), BTreeMap<u32, Quiet>> = BTreeMap::new();
        let mut mark = |side, path, lines: &mut dyn Iterator<Item = u32>, kind: Quiet| {
            let entry: &mut BTreeMap<u32, Quiet> = quiet.entry((side, path)).or_default();
            for line in lines {
                let slot = entry.entry(line).or_insert(kind);
                if kind.rank() > slot.rank() {
                    *slot = kind;
                }
            }
        };
        for file in files {
            for side in [LineSide::Base, LineSide::Head] {
                let Some(path) = file.path_on(side) else {
                    continue;
                };
                let changed = changed_lines(file, side);
                for item in classified
                    .iter()
                    .filter(|item| item.side == side && item.file == path)
                {
                    if tech::is_line_rule(item) {
                        let mut lines = item
                            .ranges
                            .iter()
                            .flatten()
                            .flat_map(|range| range.start_line..=range.end_line);
                        mark(side, path, &mut lines, Quiet::LineRule);
                    } else {
                        let kind = Quiet::WholeFile(item.category);
                        mark(side, path, &mut changed.iter().copied(), kind);
                    }
                }
            }
        }
        let quiet_moves = moves
            .moved
            .iter()
            .filter(|block| block.identical && quiet_move(files, &block.from.file, &block.to.file));
        for block in quiet_moves {
            for range in [&block.from, &block.to] {
                let mut lines = range.range.start_line..=range.range.end_line;
                mark(range.side, range.file.as_str(), &mut lines, Quiet::Move);
            }
        }
        for file in whitespace {
            let mut base = file.pairs.iter().map(|&(base, _)| base);
            mark(
                LineSide::Base,
                file.base.as_str(),
                &mut base,
                Quiet::Whitespace,
            );
            let mut head = file.pairs.iter().map(|&(_, head)| head);
            mark(
                LineSide::Head,
                file.head.as_str(),
                &mut head,
                Quiet::Whitespace,
            );
        }

        let mut hunks = Vec::new();
        let mut agent: BTreeMap<(LineSide, &str), Vec<u32>> = BTreeMap::new();
        for file in files {
            let lines_of = |side| -> Option<Vec<&str>> {
                Some(text(side, file.path_on(side)?)?.lines().collect())
            };
            let (base_text, head_text) = (lines_of(LineSide::Base), lines_of(LineSide::Head));
            for hunk in &file.hunks {
                let sides = [
                    (LineSide::Base, &hunk.removed),
                    (LineSide::Head, &hunk.added),
                ];
                let mut kinds: Vec<Option<Quiet>> = Vec::new();
                let mut all: Vec<(LineSide, u32, Option<Quiet>)> = Vec::new();
                for (side, numbers) in sides {
                    let Some(path) = file.path_on(side) else {
                        continue;
                    };
                    let lines = if side == LineSide::Base {
                        &base_text
                    } else {
                        &head_text
                    };
                    for &number in numbers {
                        let kind = quiet
                            .get(&(side, path))
                            .and_then(|lines| lines.get(&number))
                            .copied();
                        let blank = lines
                            .as_ref()
                            .and_then(|lines| {
                                lines.get(usize::try_from(number).ok()?.checked_sub(1)?)
                            })
                            .is_some_and(|line| line.trim().is_empty());
                        if !blank {
                            kinds.push(kind);
                        }
                        all.push((side, number, kind));
                    }
                }
                // Blank lines count only when the hunk has nothing else (a blank-only hunk of a test file).
                let quiet_kinds = kinds.iter().flatten().copied();
                let disposition = if kinds.contains(&None) {
                    None
                } else {
                    quiet_kinds.max_by_key(|kind| kind.rank()).or_else(|| {
                        all.iter().find_map(|(_, _, kind)| match kind {
                            Some(whole @ Quiet::WholeFile(_)) => Some(*whole),
                            _ => None,
                        })
                    })
                };
                for (side, number, kind) in all {
                    let agent_line = kind.is_none() && disposition.is_none();
                    if let (true, Some(path)) = (agent_line, file.path_on(side)) {
                        agent.entry((side, path)).or_default().push(number);
                    }
                }
                hunks.push(HunkPrepass {
                    id: hunk.id.clone(),
                    disposition: match disposition {
                        None => Disposition::Agent,
                        Some(Quiet::Move | Quiet::Whitespace | Quiet::LineRule) => {
                            Disposition::Agent
                        }
                        Some(Quiet::WholeFile(category)) => Disposition::NotAnalysed(category),
                    },
                });
            }
        }
        let line_rule = quiet
            .iter()
            .map(|(&(side, path), lines)| {
                let kept = lines
                    .iter()
                    .filter(|(_, kind)| **kind == Quiet::LineRule)
                    .map(|(line, _)| *line)
                    .collect();
                ((side, path.to_owned()), kept)
            })
            .collect();
        let agent = agent
            .into_iter()
            .map(|((side, path), mut lines)| {
                lines.sort_unstable();
                lines.dedup();
                FileRanges {
                    file: path.to_owned(),
                    side,
                    ranges: ranges(&lines),
                }
            })
            .collect();
        Self {
            hunks,
            agent,
            line_rule,
        }
    }

    /// `classified` with line-rule ranges reduced to the lines that stay quiet; emptied items dropped.
    fn classified(&self, classified: &[Classified]) -> Vec<Classified> {
        classified
            .iter()
            .filter_map(|item| {
                if !tech::is_line_rule(item) {
                    return Some(item.clone());
                }
                let kept: Vec<u32> = self
                    .line_rule
                    .get(&(item.side, item.file.clone()))?
                    .iter()
                    .copied()
                    .filter(|line| {
                        item.ranges.as_ref().is_none_or(|ranges| {
                            ranges
                                .iter()
                                .any(|r| r.start_line <= *line && *line <= r.end_line)
                        })
                    })
                    .collect();
                (!kept.is_empty()).then(|| Classified {
                    ranges: Some(ranges(&kept)),
                    ..item.clone()
                })
            })
            .collect()
    }
}

/// Not analysed sections in category order; items per file in diff order, base before head.
fn sections(files: &[FileDiff], classified: &[Classified]) -> Vec<NotAnalysedSection> {
    CATEGORIES
        .iter()
        .filter_map(|&category| {
            let mut items = Vec::new();
            let mut file_count = 0u32;
            for file in files {
                let mut found = false;
                for side in [LineSide::Base, LineSide::Head] {
                    let Some(path) = file.path_on(side) else {
                        continue;
                    };
                    let matching = classified.iter().filter(|item| {
                        item.category == category && item.side == side && item.file == path
                    });
                    for item in matching {
                        found = true;
                        items.push(NotAnalysedItem {
                            renderable: None,
                            rule_kind: None,
                            file: item.file.clone(),
                            side: item.side,
                            ranges: item.ranges.clone(),
                            badge: item.badge,
                            rule: item.rule.clone(),
                        });
                    }
                }
                file_count += u32::from(found);
            }
            (!items.is_empty()).then_some(NotAnalysedSection {
                category,
                file_count,
                items,
            })
        })
        .collect()
}

/// Whether an identical move from base `from` to head `to` is quiet: it leaves a source file that still exists at
/// head for another file, or it is a pure rename. A reorder within one file (renamed or not) and a move out of a
/// deleted file (possibly a rename git did not pair) stay with the agent.
fn quiet_move(files: &[FileDiff], from: &str, to: &str) -> bool {
    let Some(source) = files
        .iter()
        .find(|file| file.path_on(LineSide::Base) == Some(from))
    else {
        return false;
    };
    match source.path_on(LineSide::Head) {
        Some(head) if head == to => source.hunks.is_empty(),
        Some(_) => true,
        None => false,
    }
}

/// Whether a Not analysed item comes from a line rule (context, not a whole file).
fn is_line_rule_item(item: &NotAnalysedItem) -> bool {
    item.ranges.is_some() && tech::is_line_rule_name(&item.rule)
}

fn changed_lines(file: &FileDiff, side: LineSide) -> Vec<u32> {
    let mut lines: Vec<u32> = file
        .hunks
        .iter()
        .flat_map(|hunk| match side {
            LineSide::Base => &hunk.removed,
            LineSide::Head => &hunk.added,
        })
        .copied()
        .collect();
    lines.sort_unstable();
    lines
}

fn not_rendered_item(file: &str, side: LineSide, reason: NotRenderedReason) -> NotRendered {
    NotRendered {
        file: file.to_owned(),
        side,
        reason,
    }
}

#[cfg(test)]
mod tests;
