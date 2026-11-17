//! The deterministic pre-pass (brief §5 Pipeline 2) over plain inputs: classification by the tech plugin, moved
//! blocks, whitespace-only changes, counters, related repositories, the project-profile file and agent
//! instruction files; every fact carries the lines or paths it was computed from.

use std::collections::{BTreeMap, BTreeSet};

use contracts::api::tour::{
    IdenticalMove, NotAnalysedItem, NotAnalysedSection, ProfileSource, ProjectProfileStatus,
};
use contracts::common::{FileRanges, LineSide, NotAnalysedCategory, RepoRef};
use contracts::ids::HunkId;
use serde::{Deserialize, Serialize};

use crate::counters::{self, CounterFacts};
use crate::diff::FileDiff;
use crate::lines::ranges;
use crate::moves::{self, MovesReport};
use crate::related;
use crate::tech::{self, Classified, FileSource, Generic, PROFILE_PATH, TechError, TechPlugin};

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

/// What happens to a hunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "category")]
pub enum Disposition {
    /// Given to the agent (also when some of its lines are quiet).
    Agent,
    /// All its lines lie in moves with identical text.
    IdenticalMove,
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
    /// Profile sources: the base-ref file and, when the caller found one, the MemCan memory.
    pub fn profile_status(&self, memcan: bool) -> ProjectProfileStatus {
        let file = self
            .profile_file
            .is_some()
            .then_some(ProfileSource::RepositoryFile);
        let memory = memcan.then_some(ProfileSource::Memcan);
        ProjectProfileStatus {
            sources: file.into_iter().chain(memory).collect(),
        }
    }

    /// Quiet rows of moves with identical text.
    pub fn identical_moves(&self) -> Vec<IdenticalMove> {
        self.moves.identical()
    }
}

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
            let source = FileSource {
                path,
                side,
                text: if file.binary { None } else { text(side, path) },
                changed_lines: &changed,
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
    let lines = Lines::new(files, &classified, &moves, text);
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
        counters,
        related_repos,
        profile_file,
        agent_instruction_files: instruction.into_iter().map(str::to_owned).collect(),
        not_rendered,
    })
}

/// Quiet disposition of a changed line; by priority (`rank`): whole-file rule, identical move, line rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quiet {
    LineRule,
    Move,
    WholeFile(NotAnalysedCategory),
}

impl Quiet {
    fn rank(self) -> u8 {
        match self {
            Self::LineRule => 0,
            Self::Move => 1,
            Self::WholeFile(_) => 2,
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
        for block in moves.moved.iter().filter(|block| block.identical) {
            for range in [&block.from, &block.to] {
                let mut lines = range.range.start_line..=range.range.end_line;
                mark(range.side, range.file.as_str(), &mut lines, Quiet::Move);
            }
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
                        Some(Quiet::Move) => Disposition::IdenticalMove,
                        Some(Quiet::LineRule) => {
                            Disposition::NotAnalysed(NotAnalysedCategory::DocsComments)
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
