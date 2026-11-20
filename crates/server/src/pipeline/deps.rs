//! The Dependencies view (ADR-021): the Cargo map of the head commit with its conversion facts, computed from the
//! mirror once per analysis and stored as the `dependencies` document, and its projection with the plan's crate notes
//! and the controlled owners read at request time.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use analysis::blob::{Blob, Limits};
use analysis::deps::{
    self as cargo_map, ConversionKind as MapConversionKind, CrateSource, Definition,
    DefinitionQuery, DependencyMap, FileLine, MapLimit, MapStatus, Target,
};
use analysis::diff::{FileDiff, FileStatus, Hunk};
use contracts::agent::plan::{PlanCrateNote, PlanOutput};
use contracts::api::analyses::AnalysisSummary;
use contracts::api::deps::{
    ConversionFact, ConversionKind, CrateChanges, CrateFlow, CrateNote, DependencyCrate,
    DependencyLimit, DependencyRepository, DependencyStatus, DependencyView, HeadLine, NotesState,
    RepositoryKind, TypeDefinition,
};
use contracts::common::{FileRange, LineRange, LineSide, RepoRef};
use contracts::ids::{AnalysisId, CommitSha, HunkId};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::documents::DocumentKind;

use crate::github::mirror::{self, MAX_BLOB_BYTES, MirrorError, READ_TIMEOUT};
use crate::state::AppState;

/// Most bytes of changed Rust files one computation reads for conversion facts.
pub const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
/// Most bytes of files read to confirm definitions.
pub const MAX_DEFINITION_BYTES: u64 = 32 * 1024 * 1024;
/// Most facts listed per crate; the rest are counted.
pub const MAX_FACTS_PER_CRATE: usize = 50;
/// Most notes listed per crate; the rest are counted.
pub const MAX_NOTES_PER_CRATE: usize = 3;

/// The map could not be computed or stored.
#[derive(Debug, thiserror::Error)]
pub enum DepsRunError {
    /// The mirror could not be read.
    #[error("the repository files could not be read")]
    Mirror(#[from] MirrorError),
    /// The database failed.
    #[error("the dependency map could not be stored")]
    Db(#[from] DbError),
}

/// The stored map of `analysis`; a document of an older shape counts as missing (the map is deterministic).
pub async fn stored(
    state: &AppState,
    analysis: AnalysisId,
) -> Result<Option<DependencyMap>, DbError> {
    match state
        .db
        .documents()
        .get(analysis, DocumentKind::Dependencies, "")
        .await
    {
        Err(DbError::Json(error)) => {
            tracing::info!(%analysis, %error, "stored dependency map has an older shape; it is computed again");
            Ok(None)
        }
        stored => stored,
    }
}

/// The stored map, computed and stored first when missing.
pub async fn ensure(
    state: &AppState,
    summary: &AnalysisSummary,
    mirror: &Path,
    files: &[FileDiff],
) -> Result<DependencyMap, DepsRunError> {
    if let Some(map) = stored(state, summary.id).await? {
        return Ok(map);
    }
    let map = compute(
        summary.id,
        &summary.pr.repo,
        &summary.pr.head_sha,
        mirror,
        files,
    )
    .await?;
    store(state, summary.id, &map).await?;
    Ok(map)
}

/// [`ensure`] for a read request: a failed store is logged and the computed map served.
pub async fn ensure_for_read(
    state: &AppState,
    summary: &AnalysisSummary,
    mirror: &Path,
    files: &[FileDiff],
) -> Result<DependencyMap, DepsRunError> {
    if let Some(map) = stored(state, summary.id).await? {
        return Ok(map);
    }
    let map = compute(
        summary.id,
        &summary.pr.repo,
        &summary.pr.head_sha,
        mirror,
        files,
    )
    .await?;
    if let Err(error) = store(state, summary.id, &map).await {
        tracing::warn!(analysis = %summary.id, ?error,
            "the dependency map computed for a read could not be stored; the next read computes it again");
    }
    Ok(map)
}

async fn store(state: &AppState, analysis: AnalysisId, map: &DependencyMap) -> Result<(), DbError> {
    state
        .db
        .documents()
        .put(
            analysis,
            DocumentKind::Dependencies,
            "",
            map,
            Timestamp::now(),
        )
        .await?;
    tracing::debug!(%analysis, crates = map.crates.len(), facts = map.facts.len(), "dependency map stored");
    Ok(())
}

/// Reads the Cargo files and changed Rust files at head and builds the map with its facts and definitions.
async fn compute(
    analysis: AnalysisId,
    pr_repo: &RepoRef,
    head: &CommitSha,
    mirror: &Path,
    files: &[FileDiff],
) -> Result<DependencyMap, MirrorError> {
    let head_paths = mirror::tree_paths(mirror, head).await?;
    let cargo_files = cargo_map::cargo_files_to_read(&head_paths);
    let budget = u64::try_from(cargo_map::MAX_CARGO_TEXT_BYTES).unwrap_or(u64::MAX);
    let texts = read_texts(mirror, head, &cargo_files, budget).await?;
    let mut map = cargo_map::map(pr_repo, &head_paths, &|path| {
        texts.get(path).map(String::as_str)
    });
    if map.status == MapStatus::NotRust {
        return Ok(map);
    }
    let sources: Vec<(&str, Vec<u32>)> = files
        .iter()
        .filter(|file| {
            !file.binary && file.status != FileStatus::Deleted && file.path.ends_with(".rs")
        })
        .map(|file| {
            let mut added: Vec<u32> = file
                .hunks
                .iter()
                .flat_map(|h| h.added.iter().copied())
                .collect();
            added.sort_unstable();
            (file.path.as_str(), added)
        })
        .filter(|(path, added)| !added.is_empty() && map.crate_of(path).is_some())
        .collect();
    let paths: Vec<String> = sources.iter().map(|(path, _)| (*path).to_owned()).collect();
    let texts = read_texts(mirror, head, &paths, MAX_SOURCE_BYTES).await?;
    for (path, added) in &sources {
        map.add_facts(path, texts.get(*path).map(String::as_str), added);
    }
    let mut searched: Vec<DefinitionQuery> = Vec::new();
    let mut found: BTreeSet<String> = BTreeSet::new();
    for query in map.definition_queries() {
        match mirror::grep_definitions(mirror, head, &query.dir, &query.names).await {
            Ok(paths) => {
                found.extend(paths);
                searched.push(query);
            }
            Err(error) => {
                tracing::warn!(%analysis, dir = %query.dir, ?error,
                    "definition search failed; those definitions are shown as not searched");
            }
        }
    }
    let found: Vec<String> = found.into_iter().collect();
    let texts = read_texts(mirror, head, &found, MAX_DEFINITION_BYTES).await?;
    let candidates: Vec<(String, String)> = texts.into_iter().collect();
    map.locate(&searched, &candidates);
    Ok(map)
}

/// File texts by path.
type Texts = BTreeMap<String, String>;

/// UTF-8 texts of `paths` at `commit`, bounded by `total` bytes; unread, too large or non-UTF-8 files are left out.
async fn read_texts(
    mirror: &Path,
    commit: &CommitSha,
    paths: &[String],
    total: u64,
) -> Result<Texts, MirrorError> {
    if paths.is_empty() {
        return Ok(BTreeMap::new());
    }
    let requests = paths
        .iter()
        .map(|path| (commit.clone(), path.clone()))
        .collect();
    let limits = Limits {
        max_blob_bytes: MAX_BLOB_BYTES,
        total_bytes: total,
        timeout: READ_TIMEOUT,
    };
    let blobs = mirror::read_blobs(mirror, requests, limits).await?;
    Ok(paths
        .iter()
        .zip(blobs)
        .filter_map(|(path, blob)| match blob {
            Blob::Content(bytes) => String::from_utf8(bytes.to_vec())
                .ok()
                .map(|text| (path.clone(), text)),
            _ => None,
        })
        .collect())
}

/// What the view is projected from.
#[derive(Clone, Copy, Debug)]
pub struct ProjectionInput<'a> {
    /// The map of the head commit.
    pub map: &'a DependencyMap,
    /// The pull request's repository.
    pub pr_repo: &'a RepoRef,
    /// The diff.
    pub files: &'a [FileDiff],
    /// The stored plan, if any.
    pub plan: Option<&'a PlanOutput>,
    /// The Settings list of controlled owners.
    pub controlled: &'a [String],
}

/// The Dependencies view of `input`.
pub fn project(input: &ProjectionInput<'_>) -> DependencyView {
    let mut owners: Vec<String> = Vec::new();
    for owner in std::iter::once(&input.pr_repo.owner).chain(input.controlled) {
        if !owners.iter().any(|known| known.eq_ignore_ascii_case(owner)) {
            owners.push(owner.clone());
        }
    }
    owners.sort_by_key(|owner| owner.to_lowercase());
    let controlled = |repo: &RepoRef| owners.iter().any(|o| o.eq_ignore_ascii_case(&repo.owner));
    let notes = match input.plan {
        None => NotesState::Pending,
        Some(plan) if plan.crate_notes.is_none() => NotesState::NotProduced,
        Some(_) => NotesState::Produced,
    };
    let map = input.map;
    let mut view = DependencyView {
        status: match map.status {
            MapStatus::Available => DependencyStatus::Available,
            MapStatus::NotRust => DependencyStatus::NotRust,
        },
        controlled_owners: owners.clone(),
        limits: map.limits.iter().map(limit).collect(),
        files_outside_crates: 0,
        notes,
        repositories: Vec::new(),
    };
    if map.status == MapStatus::NotRust {
        return view;
    }

    // Changed lines and files per crate.
    let mut changes: BTreeMap<usize, CrateChanges> = BTreeMap::new();
    let mut hunk_crate: HashMap<&HunkId, (usize, &FileDiff, &Hunk)> = HashMap::new();
    for file in input.files {
        let Some(index) = map.crate_of(&file.path) else {
            view.files_outside_crates += 1;
            continue;
        };
        let entry = changes.entry(index).or_insert_with(empty_changes);
        entry.files += 1;
        for hunk in &file.hunks {
            entry.changed_lines += to_u32(hunk.added.len() + hunk.removed.len());
            hunk_crate.insert(&hunk.id, (index, file, hunk));
        }
    }

    // Flows per crate, in reading order.
    if let Some(plan) = input.plan {
        let mut flows: Vec<_> = plan.flows.iter().collect();
        flows.sort_by_key(|flow| flow.importance_rank);
        for flow in flows {
            let crates: BTreeSet<usize> = flow
                .hunks
                .iter()
                .filter_map(|selection| {
                    hunk_crate.get(&selection.hunk_id).map(|(index, ..)| *index)
                })
                .collect();
            for index in crates {
                if let Some(entry) = changes.get_mut(&index) {
                    entry.flows.push(CrateFlow {
                        id: flow.id.clone(),
                        title: flow.title.clone(),
                    });
                }
            }
        }
        for note in plan.crate_notes.iter().flatten() {
            add_note(note, &hunk_crate, &mut changes);
        }
    }
    for entry in changes.values_mut() {
        entry
            .notes
            .sort_by_key(|note| std::cmp::Reverse(note.changed_lines));
        let hidden = entry.notes.len().saturating_sub(MAX_NOTES_PER_CRATE);
        entry.notes.truncate(MAX_NOTES_PER_CRATE);
        entry.notes_hidden = to_u32(hidden);
    }

    // Which external crates are "yours".
    let external_repo = |index: u32| -> Option<RepoRef> {
        match &map.external.get(index as usize)?.source {
            CrateSource::Git { repo, .. } | CrateSource::Path { repo, .. } => repo.clone(),
            CrateSource::Registry { .. } | CrateSource::Unknown => None,
        }
    };
    let listed_target = |target: Target| match target {
        Target::Repo { .. } => true,
        Target::External { index } => external_repo(index).is_some_and(|repo| controlled(&repo)),
    };

    // Facts per changed crate.
    let mut facts_about: HashMap<Target, u32> = HashMap::new();
    for fact in &map.facts {
        let Some(entry) = changes.get_mut(&(fact.crate_index as usize)) else {
            continue;
        };
        if !listed_target(fact.target) {
            entry.other_facts += 1;
            continue;
        }
        *facts_about.entry(fact.target).or_default() += 1;
        if entry.facts.len() >= MAX_FACTS_PER_CRATE {
            entry.facts_hidden += 1;
            continue;
        }
        entry.facts.push(ConversionFact {
            kind: match fact.kind {
                MapConversionKind::FromImpl => ConversionKind::FromImpl,
                MapConversionKind::TryFromImpl => ConversionKind::TryFromImpl,
                MapConversionKind::ImplOnForeignType => ConversionKind::ImplOnForeignType,
                MapConversionKind::ConversionFn => ConversionKind::ConversionFn,
            },
            at: head_line(&fact.at),
            type_name: fact.type_name.clone(),
            written: fact.written.clone(),
            type_crate: target_name(map, fact.target),
            imported_at: fact.imported_at.as_ref().map(head_line),
            definition: match &fact.definition {
                Definition::Located { at } => TypeDefinition::Located { at: head_line(at) },
                Definition::NotLocated => TypeDefinition::NotLocated,
                Definition::NotSearched => TypeDefinition::NotSearched,
                Definition::OtherRepository => TypeDefinition::OtherRepository,
            },
        });
    }

    // Direct dependencies of the changed crates: listed when in this repository or controlled, counted otherwise.
    let mut used_by: BTreeMap<Target, BTreeSet<String>> = BTreeMap::new();
    for (&index, entry) in &mut changes {
        let Some(krate) = map.crates.get(index) else {
            continue;
        };
        let mut others: BTreeSet<&str> = BTreeSet::new();
        for dependency in &krate.dependencies {
            if dependency.target
                == (Target::Repo {
                    index: to_u32(index),
                })
            {
                continue;
            }
            if listed_target(dependency.target) {
                used_by
                    .entry(dependency.target)
                    .or_default()
                    .insert(krate.name.clone());
            } else {
                others.insert(dependency.package.as_str());
            }
        }
        entry.other_dependencies = to_u32(others.len());
    }

    // This repository: changed crates and the crates they use.
    let mut this: BTreeSet<usize> = changes.keys().copied().collect();
    for target in used_by.keys() {
        if let Target::Repo { index } = target {
            this.insert(*index as usize);
        }
    }
    let mut crates: Vec<DependencyCrate> = this
        .into_iter()
        .filter_map(|index| {
            let krate = map.crates.get(index)?;
            let target = Target::Repo {
                index: to_u32(index),
            };
            Some(DependencyCrate {
                name: krate.name.clone(),
                version: krate.version.clone(),
                manifest: Some(krate.manifest.clone()),
                source_at: None,
                revision: None,
                used_by: used_by
                    .get(&target)
                    .map(|s| s.iter().cloned().collect())
                    .unwrap_or_default(),
                facts_about: facts_about.get(&target).copied().unwrap_or(0),
                changes: changes.remove(&index),
            })
        })
        .collect();
    sort_crates(&mut crates);
    view.repositories.push(DependencyRepository {
        kind: RepositoryKind::This,
        repo: Some(input.pr_repo.clone()),
        location: None,
        controlled: true,
        crates,
    });

    // Controlled repositories of external crates.
    let mut others: BTreeMap<(String, String), DependencyRepository> = BTreeMap::new();
    for (target, users) in &used_by {
        let Target::External { index } = *target else {
            continue;
        };
        let (Some(external), Some(repo)) = (map.external.get(index as usize), external_repo(index))
        else {
            continue;
        };
        let (location, revision) = match &external.source {
            CrateSource::Git { url, rev, .. } => (Some(url.clone()), rev.clone()),
            CrateSource::Path { path, .. } => (Some(path.clone()), None),
            CrateSource::Registry { .. } | CrateSource::Unknown => (None, None),
        };
        let key = (repo.owner.to_lowercase(), repo.name.to_lowercase());
        let entry = others.entry(key).or_insert_with(|| DependencyRepository {
            kind: RepositoryKind::Github,
            repo: Some(repo.clone()),
            location: location.clone(),
            controlled: true,
            crates: Vec::new(),
        });
        entry.crates.push(DependencyCrate {
            name: external.package.clone(),
            version: external.version.clone(),
            manifest: None,
            source_at: external.evidence.as_ref().map(head_line),
            revision,
            used_by: users.iter().cloned().collect(),
            facts_about: facts_about.get(target).copied().unwrap_or(0),
            changes: None,
        });
    }
    for mut repository in others.into_values() {
        sort_crates(&mut repository.crates);
        view.repositories.push(repository);
    }
    view
}

/// Adds `note` to every changed crate holding one of its hunks, with that crate's lines of those hunks.
fn add_note(
    note: &PlanCrateNote,
    hunk_crate: &HashMap<&HunkId, (usize, &FileDiff, &Hunk)>,
    changes: &mut BTreeMap<usize, CrateChanges>,
) {
    let text = note.note.trim();
    if text.is_empty() {
        return;
    }
    let mut per_crate: BTreeMap<usize, (Vec<FileRange>, u32)> = BTreeMap::new();
    for id in &note.hunks {
        let Some((index, file, hunk)) = hunk_crate.get(id) else {
            continue;
        };
        let (side, span) = if hunk.head.len > 0 {
            (LineSide::Head, &hunk.head)
        } else {
            (LineSide::Base, &hunk.base)
        };
        let (file_path, start) = match side {
            LineSide::Head => (file.path.clone(), span.start),
            LineSide::Base => (
                file.old_path.clone().unwrap_or_else(|| file.path.clone()),
                span.start,
            ),
        };
        let entry = per_crate.entry(*index).or_default();
        entry.0.push(FileRange {
            file: file_path,
            side,
            range: LineRange {
                start_line: start.max(1),
                end_line: (start + span.len.saturating_sub(1)).max(start.max(1)),
            },
        });
        entry.1 += to_u32(hunk.added.len() + hunk.removed.len());
    }
    for (index, (lines, changed_lines)) in per_crate {
        if let Some(entry) = changes.get_mut(&index) {
            entry.notes.push(CrateNote {
                text: text.to_owned(),
                lines,
                changed_lines,
            });
        }
    }
}

fn empty_changes() -> CrateChanges {
    CrateChanges {
        changed_lines: 0,
        files: 0,
        flows: Vec::new(),
        facts: Vec::new(),
        facts_hidden: 0,
        other_facts: 0,
        notes: Vec::new(),
        notes_hidden: 0,
        other_dependencies: 0,
    }
}

/// Changed crates first (most changed lines first), then by name.
fn sort_crates(crates: &mut [DependencyCrate]) {
    crates.sort_by(|a, b| {
        let lines = |c: &DependencyCrate| c.changes.as_ref().map(|ch| ch.changed_lines);
        lines(b).cmp(&lines(a)).then_with(|| a.name.cmp(&b.name))
    });
}

fn target_name(map: &DependencyMap, target: Target) -> String {
    match target {
        Target::Repo { index } => map.crates.get(index as usize).map(|c| c.name.clone()),
        Target::External { index } => map.external.get(index as usize).map(|e| e.package.clone()),
    }
    .unwrap_or_default()
}

fn head_line(line: &FileLine) -> HeadLine {
    HeadLine {
        file: line.file.clone(),
        line: line.line,
    }
}

fn limit(limit: &MapLimit) -> DependencyLimit {
    match limit {
        MapLimit::ManifestsCut { total, read } => DependencyLimit::ManifestsCut {
            total: *total,
            read: *read,
        },
        MapLimit::ManifestUnreadable { path } => {
            DependencyLimit::ManifestUnreadable { path: path.clone() }
        }
        MapLimit::LockUnreadable { path } => DependencyLimit::LockUnreadable { path: path.clone() },
        MapLimit::NoLockFile { workspace_dir } => DependencyLimit::NoLockFile {
            workspace_dir: workspace_dir.clone(),
        },
        MapLimit::SourceUnread { path } => DependencyLimit::SourceUnread { path: path.clone() },
        MapLimit::FactsCut { kept } => DependencyLimit::FactsCut { kept: *kept },
    }
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
