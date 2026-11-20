//! The Cargo map of the head commit (ADR-021): the repository's crates, their direct dependencies and where each
//! dependency comes from, read from `Cargo.toml` files and `Cargo.lock` with `toml_edit`. Nothing is run and no IO
//! happens here: what cannot be read is a [`MapLimit`], never an absence.

use std::collections::{BTreeMap, HashMap};

use contracts::common::RepoRef;
use serde::{Deserialize, Serialize};
use toml_edit::{Document, Item, TableLike};

use crate::counters::cargo::{self, LineIndex};
use crate::related;
use crate::tech::{self, CandidateKind, TypeOrigin};

#[cfg(test)]
mod tests;

/// Most manifests read; the rest are cut ([`MapLimit::ManifestsCut`]).
pub const MAX_MANIFESTS: usize = 400;
/// Most bytes of Cargo text read over all files; a file beyond it is unreadable.
pub const MAX_CARGO_TEXT_BYTES: usize = 16 * 1024 * 1024;
/// Most conversion facts kept; the rest are cut ([`MapLimit::FactsCut`]).
pub const MAX_FACTS: usize = 2000;
/// Most type names one definition search looks for.
pub const MAX_DEFINITION_NAMES: usize = 64;
/// Registry name of crates.io.
pub const CRATES_IO: &str = "crates.io";

const MANIFEST: &str = "Cargo.toml";
const LOCK: &str = "Cargo.lock";

/// The Cargo map of one commit.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyMap {
    /// Whether the repository has Cargo manifests.
    pub status: MapStatus,
    /// What was cut or could not be read.
    pub limits: Vec<MapLimit>,
    /// The crates of the repository (manifests with `[package]`), by directory.
    pub crates: Vec<RepoCrate>,
    /// Crates from outside the repository that its crates depend on directly.
    pub external: Vec<ExternalCrate>,
    /// Conversions added for types of other crates, in file and line order.
    pub facts: Vec<ConversionFact>,
}

/// Whether the map exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapStatus {
    /// At least one `Cargo.toml` at the commit.
    Available,
    /// No `Cargo.toml` at the commit: nothing was checked.
    #[default]
    NotRust,
}

/// Something the map does not cover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MapLimit {
    /// More manifests than [`MAX_MANIFESTS`]; the first `read` in path order were read.
    ManifestsCut {
        /// Manifests at the commit.
        total: u32,
        /// Manifests read.
        read: u32,
    },
    /// A manifest that was not read, is too large or does not parse.
    ManifestUnreadable {
        /// Its path.
        path: String,
    },
    /// A `Cargo.lock` that was not read, is too large or does not parse.
    LockUnreadable {
        /// Its path.
        path: String,
    },
    /// A workspace (or standalone crate) without `Cargo.lock`: versions are not resolved, sources come from the
    /// manifests.
    NoLockFile {
        /// Directory of its root manifest (`""` = repository root).
        workspace_dir: String,
    },
    /// A changed Rust file that was not read: its conversions are not checked.
    SourceUnread {
        /// Its path.
        path: String,
    },
    /// More conversion facts than [`MAX_FACTS`]; the first ones in file order are kept.
    FactsCut {
        /// Facts kept.
        kept: u32,
    },
    /// Files of a crate that may hold a searched definition were not read: definitions not found there are not
    /// searched.
    DefinitionsUnread {
        /// The crate's directory.
        dir: String,
        /// Files not read.
        files: u32,
    },
}

/// A conversion added on a changed line of a crate for a type of another crate (ADR-021).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionFact {
    /// What the line adds.
    pub kind: ConversionKind,
    /// The `impl` or `fn` line.
    pub at: FileLine,
    /// The crate holding the line (index into [`DependencyMap::crates`]).
    pub crate_index: u32,
    /// The type's name.
    pub type_name: String,
    /// The type's path as written.
    pub written: String,
    /// The crate the type comes from.
    pub target: Target,
    /// The `use` line that imports it, if it came through one.
    pub imported_at: Option<FileLine>,
    /// Where the type is defined.
    pub definition: Definition,
}

/// What a conversion fact's line adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionKind {
    /// `impl From<A> for B`.
    FromImpl,
    /// `impl TryFrom<A> for B`.
    TryFromImpl,
    /// `impl Trait for B`: a trait implemented on the other crate's type.
    ImplOnForeignType,
    /// A function named like a conversion whose signature names the type.
    ConversionFn,
}

/// Where a fact's type is defined.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Definition {
    /// Found in the defining crate of this repository.
    Located {
        /// The definition line.
        at: FileLine,
    },
    /// Searched in the defining crate of this repository and not found (generated, a macro, a re-export).
    NotLocated,
    /// Not searched: the search limit was reached or the search failed.
    NotSearched,
    /// The type is in another repository; not looked up.
    OtherRepository,
}

/// One definition search: type names in one crate directory of this repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefinitionQuery {
    /// The crate's directory (`""` = repository root).
    pub dir: String,
    /// The type names, sorted and unique.
    pub names: Vec<String>,
}

/// A crate of the repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoCrate {
    /// Package name.
    pub name: String,
    /// Package version, if declared (inherited versions resolved).
    pub version: Option<String>,
    /// Directory of the manifest (`""` = repository root).
    pub dir: String,
    /// Path of the manifest.
    pub manifest: String,
    /// The name code uses for its library (`[lib] name`, else the package name with `-` as `_`).
    pub code_name: String,
    /// Its direct dependencies in manifest order.
    pub dependencies: Vec<Dependency>,
}

/// One direct dependency of a crate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// The name code uses for it.
    pub code_name: String,
    /// The package depended on.
    pub package: String,
    /// Dependency table kind.
    pub kind: DependencyKind,
    /// Line of its key in the crate's manifest.
    pub line: u32,
    /// What it resolves to.
    pub target: Target,
}

/// Dependency table kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyKind {
    /// `[dependencies]`.
    Normal,
    /// `[dev-dependencies]`.
    Dev,
    /// `[build-dependencies]`.
    Build,
}

/// What a dependency resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// A crate of this repository (index into [`DependencyMap::crates`]).
    Repo {
        /// The index.
        index: u32,
    },
    /// A crate from outside (index into [`DependencyMap::external`]).
    External {
        /// The index.
        index: u32,
    },
}

/// A crate from outside the repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalCrate {
    /// Package name.
    pub package: String,
    /// Resolved version (from `Cargo.lock`), if known.
    pub version: Option<String>,
    /// Where it comes from.
    pub source: CrateSource,
    /// The `Cargo.lock` record or the manifest declaration that says so.
    pub evidence: Option<FileLine>,
}

/// A line of a file at the commit.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileLine {
    /// Path.
    pub file: String,
    /// 1-based line.
    pub line: u32,
}

/// Where an external crate comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CrateSource {
    /// A registry ([`CRATES_IO`] or the registry's index URL).
    Registry {
        /// The registry.
        registry: String,
    },
    /// A git repository.
    Git {
        /// The URL without the revision fragment.
        url: String,
        /// The GitHub repository, when the URL names one.
        repo: Option<RepoRef>,
        /// The locked commit, if known.
        rev: Option<String>,
    },
    /// A path dependency whose target is not a read crate of this repository.
    Path {
        /// The path, relative to the repository root (`../<repo>/…` for a sibling repository).
        path: String,
        /// The sibling repository (same owner), for a path one level above the root.
        repo: Option<RepoRef>,
    },
    /// Not determined (a `workspace = true` dependency without a workspace declaration, an unknown source).
    Unknown,
}

impl DependencyMap {
    /// Index of the crate whose directory holds `path` most deeply.
    pub fn crate_of(&self, path: &str) -> Option<usize> {
        self.crates
            .iter()
            .enumerate()
            .filter(|(_, krate)| {
                krate.dir.is_empty()
                    || path
                        .strip_prefix(krate.dir.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|(_, krate)| krate.dir.len())
            .map(|(index, _)| index)
    }
}

impl DependencyMap {
    /// Adds the conversion facts of the changed Rust file `path`, whose head text is `text` (`None`: not read) and
    /// whose added head lines are the sorted `added`: conversions for types of crates the file's crate depends on.
    pub fn add_facts(&mut self, path: &str, text: Option<&str>, added: &[u32]) {
        let Some(from) = self.crate_of(path) else {
            return;
        };
        let Some(text) = text else {
            self.limits.push(MapLimit::SourceUnread {
                path: path.to_owned(),
            });
            return;
        };
        for candidate in tech::conversion_candidates(text, added) {
            let TypeOrigin::Crate {
                code_name,
                import_line,
            } = &candidate.origin
            else {
                continue;
            };
            let Some(dependency) = self.crates.get(from).and_then(|krate| {
                krate
                    .dependencies
                    .iter()
                    .find(|d| d.code_name == *code_name)
            }) else {
                continue;
            };
            let target = dependency.target;
            if target
                == (Target::Repo {
                    index: to_u32(from),
                })
                || self.facts.iter().any(|fact| {
                    fact.at.file == path
                        && fact.at.line == candidate.line
                        && fact.type_name == candidate.type_name
                })
            {
                continue;
            }
            if self.facts.len() >= MAX_FACTS {
                if !self
                    .limits
                    .iter()
                    .any(|l| matches!(l, MapLimit::FactsCut { .. }))
                {
                    self.limits.push(MapLimit::FactsCut {
                        kept: to_u32(MAX_FACTS),
                    });
                }
                return;
            }
            let definition = match target {
                Target::Repo { .. } => Definition::NotSearched,
                Target::External { index } => {
                    match self.external.get(index as usize).map(|e| &e.source) {
                        Some(CrateSource::Path { repo: None, .. }) => Definition::NotLocated,
                        _ => Definition::OtherRepository,
                    }
                }
            };
            self.facts.push(ConversionFact {
                kind: match candidate.kind {
                    CandidateKind::FromImpl => ConversionKind::FromImpl,
                    CandidateKind::TryFromImpl => ConversionKind::TryFromImpl,
                    CandidateKind::ImplOnType => ConversionKind::ImplOnForeignType,
                    CandidateKind::ConversionFn => ConversionKind::ConversionFn,
                },
                at: FileLine {
                    file: path.to_owned(),
                    line: candidate.line,
                },
                crate_index: to_u32(from),
                type_name: candidate.type_name,
                written: candidate.written,
                target,
                imported_at: import_line.map(|line| FileLine {
                    file: path.to_owned(),
                    line,
                }),
                definition,
            });
        }
    }

    /// The definition searches the facts need: per crate of this repository, the names of its types the facts name,
    /// at most [`MAX_DEFINITION_NAMES`] over all searches.
    pub fn definition_queries(&self) -> Vec<DefinitionQuery> {
        let mut by_crate: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        for fact in &self.facts {
            if let (Target::Repo { index }, Definition::NotSearched) =
                (fact.target, &fact.definition)
            {
                by_crate
                    .entry(index)
                    .or_default()
                    .push(fact.type_name.clone());
            }
        }
        let mut budget = MAX_DEFINITION_NAMES;
        let mut out = Vec::new();
        for (index, mut names) in by_crate {
            names.sort();
            names.dedup();
            names.truncate(budget);
            budget -= names.len();
            if let (Some(krate), false) = (self.crates.get(index as usize), names.is_empty()) {
                out.push(DefinitionQuery {
                    dir: krate.dir.clone(),
                    names,
                });
            }
        }
        out
    }

    /// Settles the definitions of the facts whose names `queries` searched, from the head `files` (path, text) the
    /// search found: the first definition in path order inside the defining crate, else [`Definition::NotLocated`];
    /// in a crate holding one of the found files that were `unread`, else [`Definition::NotSearched`] and a
    /// [`MapLimit::DefinitionsUnread`].
    pub fn locate(
        &mut self,
        queries: &[DefinitionQuery],
        files: &[(String, String)],
        unread: &[String],
    ) {
        let mut incomplete: BTreeMap<usize, u32> = BTreeMap::new();
        for path in unread {
            if let Some(index) = self.crate_of(path) {
                *incomplete.entry(index).or_default() += 1;
            }
        }
        for (&index, &count) in &incomplete {
            if let Some(krate) = self.crates.get(index) {
                self.limits.push(MapLimit::DefinitionsUnread {
                    dir: krate.dir.clone(),
                    files: count,
                });
            }
        }
        let mut files: Vec<&(String, String)> = files.iter().collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let owners: Vec<Option<usize>> =
            files.iter().map(|(path, _)| self.crate_of(path)).collect();
        for fact in &mut self.facts {
            let (Target::Repo { index }, Definition::NotSearched) = (fact.target, &fact.definition)
            else {
                continue;
            };
            let Some(dir) = self.crates.get(index as usize).map(|k| &k.dir) else {
                continue;
            };
            let searched = queries
                .iter()
                .any(|q| q.dir == *dir && q.names.binary_search(&fact.type_name).is_ok());
            if !searched {
                continue;
            }
            fact.definition = files
                .iter()
                .zip(&owners)
                .filter(|(_, owner)| **owner == Some(index as usize))
                .find_map(|((path, text), _)| {
                    tech::definition_line(text, &fact.type_name).map(|line| FileLine {
                        file: path.clone(),
                        line,
                    })
                })
                .map_or_else(
                    || {
                        if incomplete.contains_key(&(index as usize)) {
                            Definition::NotSearched
                        } else {
                            Definition::NotLocated
                        }
                    },
                    |at| Definition::Located { at },
                );
        }
    }
}

/// The Cargo files among `head_paths` the map reads: every `Cargo.lock` and the first [`MAX_MANIFESTS`]
/// `Cargo.toml` files in path order.
pub fn cargo_files_to_read(head_paths: &[String]) -> Vec<String> {
    let mut manifests: Vec<&String> = head_paths
        .iter()
        .filter(|p| file_name(p) == MANIFEST)
        .collect();
    manifests.sort();
    manifests.truncate(MAX_MANIFESTS);
    let mut out: Vec<String> = head_paths
        .iter()
        .filter(|p| file_name(p) == LOCK)
        .chain(manifests)
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Builds the map of the commit whose paths are `head_paths`; `text(path)` gives a Cargo file's text (`None` when
/// absent or unread). `pr_repo` names sibling repositories of path dependencies.
pub fn map<'t>(
    pr_repo: &RepoRef,
    head_paths: &[String],
    text: &dyn Fn(&str) -> Option<&'t str>,
) -> DependencyMap {
    let total = head_paths
        .iter()
        .filter(|p| file_name(p) == MANIFEST)
        .count();
    if total == 0 {
        return DependencyMap::default();
    }
    let mut limits = Vec::new();
    if total > MAX_MANIFESTS {
        limits.push(MapLimit::ManifestsCut {
            total: to_u32(total),
            read: to_u32(MAX_MANIFESTS),
        });
    }
    let mut budget = MAX_CARGO_TEXT_BYTES;
    let mut read = |path: &str| -> Option<&'t str> {
        let text = text(path)?;
        budget = budget.checked_sub(text.len())?;
        Some(text)
    };
    let mut manifests: Vec<Manifest<'t>> = Vec::new();
    for path in cargo_files_to_read(head_paths)
        .iter()
        .filter(|p| file_name(p) == MANIFEST)
    {
        match read(path).and_then(|text| Some((text, cargo::parse(text)?))) {
            Some((text, document)) => manifests.push(Manifest {
                dir: dir_of(path).to_owned(),
                path: path.clone(),
                lines: LineIndex::new(text),
                document,
            }),
            None => limits.push(MapLimit::ManifestUnreadable { path: path.clone() }),
        }
    }
    let mut builder = Builder {
        pr_repo,
        manifests: &manifests,
        crates: Vec::new(),
        external: Vec::new(),
        external_index: HashMap::new(),
        locks: BTreeMap::new(),
    };
    for root in builder.lock_roots() {
        let path = join(&root, LOCK);
        let lock = if head_paths.contains(&path) {
            match read(&path).and_then(Lock::parse) {
                Some(lock) => Some(lock),
                None => {
                    limits.push(MapLimit::LockUnreadable { path });
                    None
                }
            }
        } else {
            limits.push(MapLimit::NoLockFile {
                workspace_dir: root.clone(),
            });
            None
        };
        builder.locks.insert(root, lock);
    }
    builder.build();
    DependencyMap {
        status: MapStatus::Available,
        limits,
        crates: builder.crates,
        external: builder.external,
        facts: Vec::new(),
    }
}

/// One parsed manifest.
struct Manifest<'t> {
    dir: String,
    path: String,
    lines: LineIndex,
    document: Document<&'t str>,
}

impl Manifest<'_> {
    fn package(&self) -> Option<&dyn TableLike> {
        self.document.get("package").and_then(Item::as_table_like)
    }

    fn workspace(&self) -> Option<&dyn TableLike> {
        self.document.get("workspace").and_then(Item::as_table_like)
    }

    /// Whether `dir` is listed in this workspace root's `exclude`.
    fn excludes(&self, dir: &str) -> bool {
        let excluded = self
            .workspace()
            .and_then(|w| w.get("exclude"))
            .and_then(Item::as_array);
        excluded.is_some_and(|list| {
            list.iter().filter_map(|v| v.as_str()).any(|entry| {
                normalise(&self.dir, entry).is_some_and(|Inside(excluded)| {
                    dir == excluded
                        || dir
                            .strip_prefix(excluded.as_str())
                            .is_some_and(|rest| rest.starts_with('/'))
                })
            })
        })
    }
}

/// One parsed `Cargo.lock`.
struct Lock {
    packages: Vec<LockPackage>,
}

struct LockPackage {
    name: String,
    version: Option<String>,
    source: Option<String>,
    line: u32,
    dependencies: Vec<String>,
}

impl Lock {
    fn parse(text: &str) -> Option<Self> {
        let document = cargo::parse(text)?;
        let lines = LineIndex::new(text);
        let mut packages = Vec::new();
        if let Some(records) = document.get("package").and_then(Item::as_array_of_tables) {
            for record in records.iter() {
                let Some(name) = record.get("name").and_then(Item::as_str) else {
                    continue;
                };
                let field = |key: &str| record.get(key).and_then(Item::as_str).map(str::to_owned);
                packages.push(LockPackage {
                    name: name.to_owned(),
                    version: field("version"),
                    source: field("source"),
                    line: record.span().map_or(0, |span| lines.line(span.start)),
                    dependencies: record
                        .get("dependencies")
                        .and_then(Item::as_array)
                        .map(|list| {
                            list.iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                });
            }
        }
        Some(Self { packages })
    }
}

/// One dependency declaration as read from a manifest (after `workspace = true` inheritance).
struct Declared {
    package: String,
    git: Option<String>,
    path: Option<String>,
    /// Directory paths in the declaration are relative to.
    base_dir: String,
    /// Where the declaration that gives the source is.
    evidence: Option<FileLine>,
    /// `workspace = true` without a workspace declaration.
    unresolved: bool,
}

struct Builder<'b, 't> {
    pr_repo: &'b RepoRef,
    manifests: &'b [Manifest<'t>],
    crates: Vec<RepoCrate>,
    external: Vec<ExternalCrate>,
    external_index: HashMap<(String, Option<String>, String), u32>,
    /// Lock file per lock root directory (`None` = missing or unreadable).
    locks: BTreeMap<String, Option<Lock>>,
}

impl<'b, 't> Builder<'b, 't> {
    /// The workspace root of the manifest at `dir`: the nearest manifest at or above it with `[workspace]` that
    /// does not exclude it.
    fn root_of(&self, dir: &str) -> Option<&'b Manifest<'t>> {
        let manifests: &'b [Manifest<'t>] = self.manifests;
        manifests
            .iter()
            .filter(|m| m.workspace().is_some())
            .filter(|m| m.dir.is_empty() || m.dir == dir || dir.starts_with(&format!("{}/", m.dir)))
            .max_by_key(|m| m.dir.len())
            .filter(|root| !root.excludes(dir))
    }

    /// Directories whose `Cargo.lock` applies: workspace roots and crates without a workspace.
    fn lock_roots(&self) -> Vec<String> {
        let mut roots: Vec<String> = self
            .manifests
            .iter()
            .filter(|m| m.package().is_some())
            .map(|m| {
                self.root_of(&m.dir)
                    .map_or(m.dir.clone(), |r| r.dir.clone())
            })
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }

    fn build(&mut self) {
        let manifests: &'b [Manifest<'t>] = self.manifests;
        let packages: Vec<&'b Manifest<'t>> = manifests
            .iter()
            .filter(|m| {
                m.package()
                    .and_then(|p| p.get("name"))
                    .and_then(Item::as_str)
                    .is_some()
            })
            .collect();
        for manifest in &packages {
            let root = self.root_of(&manifest.dir);
            let Some(package) = manifest.package() else {
                continue;
            };
            let Some(name) = package.get("name").and_then(Item::as_str) else {
                continue;
            };
            let version = match package.get("version") {
                Some(item) if inherits(item) => root
                    .and_then(|r| r.workspace())
                    .and_then(|w| w.get("package"))
                    .and_then(Item::as_table_like)
                    .and_then(|p| p.get("version"))
                    .and_then(Item::as_str)
                    .map(str::to_owned),
                Some(item) => item.as_str().map(str::to_owned),
                None => None,
            };
            let lib = manifest
                .document
                .get("lib")
                .and_then(Item::as_table_like)
                .and_then(|lib| lib.get("name"))
                .and_then(Item::as_str);
            self.crates.push(RepoCrate {
                name: name.to_owned(),
                version,
                dir: manifest.dir.clone(),
                manifest: manifest.path.clone(),
                code_name: lib.map_or_else(|| code(name), str::to_owned),
                dependencies: Vec::new(),
            });
        }
        for (index, manifest) in packages.iter().enumerate() {
            let dependencies = self.dependencies(manifest, index);
            if let Some(krate) = self.crates.get_mut(index) {
                krate.dependencies = dependencies;
            }
        }
    }

    fn dependencies(&mut self, manifest: &'b Manifest<'t>, index: usize) -> Vec<Dependency> {
        let root = self.root_of(&manifest.dir);
        let lock_root = root.map_or(manifest.dir.clone(), |r| r.dir.clone());
        let mut tables: Vec<(DependencyKind, &Item)> = kinds(manifest.document.as_table());
        if let Some(targets) = manifest
            .document
            .get("target")
            .and_then(Item::as_table_like)
        {
            for (_, target) in targets.iter() {
                tables.extend(target.as_table_like().map(kinds).unwrap_or_default());
            }
        }
        let mut out = Vec::new();
        for (kind, item) in tables {
            let Some(table) = item.as_table_like() else {
                continue;
            };
            for (key, value) in table.iter() {
                let line = table
                    .get_key_value(key)
                    .and_then(|(key, _)| key.span())
                    .map_or(0, |span| manifest.lines.line(span.start));
                let declared = declared(manifest, root, key, value, line);
                let target = self.target(&declared, &lock_root, index);
                let code_name = match target {
                    Target::Repo { index } if declared.package == key => self
                        .crates
                        .get(index as usize)
                        .map_or_else(|| code(key), |c| c.code_name.clone()),
                    _ => code(key),
                };
                out.push(Dependency {
                    code_name,
                    package: declared.package,
                    kind,
                    line,
                    target,
                });
            }
        }
        out
    }

    fn target(&mut self, declared: &Declared, lock_root: &str, from: usize) -> Target {
        if let Some(path) = &declared.path {
            match normalise(&declared.base_dir, path) {
                Some(Inside(dir)) => {
                    if let Some(index) = self.crates.iter().position(|c| c.dir == dir) {
                        return Target::Repo {
                            index: to_u32(index),
                        };
                    }
                    let source = CrateSource::Path {
                        path: dir,
                        repo: None,
                    };
                    return self.add(&declared.package, None, source, declared.evidence.clone());
                }
                None => {
                    let source = sibling(self.pr_repo, &declared.base_dir, path);
                    return self.add(&declared.package, None, source, declared.evidence.clone());
                }
            }
        }
        if declared.unresolved {
            return self.add(&declared.package, None, CrateSource::Unknown, None);
        }
        if let Some(Some(lock)) = self.locks.get(lock_root)
            && let Some((version, source, line)) =
                locked(lock, self.crates.get(from), &declared.package)
        {
            let evidence = Some(FileLine {
                file: join(lock_root, LOCK),
                line,
            });
            return self.add(&declared.package, version, source, evidence);
        }
        let source = match &declared.git {
            Some(url) => CrateSource::Git {
                url: url.clone(),
                repo: github(url),
                rev: None,
            },
            None => CrateSource::Registry {
                registry: CRATES_IO.to_owned(),
            },
        };
        self.add(&declared.package, None, source, declared.evidence.clone())
    }

    fn add(
        &mut self,
        package: &str,
        version: Option<String>,
        source: CrateSource,
        evidence: Option<FileLine>,
    ) -> Target {
        let key = (package.to_owned(), version.clone(), format!("{source:?}"));
        if let Some(index) = self.external_index.get(&key) {
            return Target::External { index: *index };
        }
        let index = to_u32(self.external.len());
        self.external.push(ExternalCrate {
            package: package.to_owned(),
            version,
            source,
            evidence,
        });
        self.external_index.insert(key, index);
        Target::External { index }
    }
}

/// The declaration of dependency `key` (`value`) of `manifest`, with `workspace = true` resolved through `root`.
fn declared(
    manifest: &Manifest<'_>,
    root: Option<&Manifest<'_>>,
    key: &str,
    value: &Item,
    line: u32,
) -> Declared {
    let field = |item: &Item, name: &str| {
        item.as_table_like()
            .and_then(|t| t.get(name))
            .and_then(Item::as_str)
            .map(str::to_owned)
    };
    let inherited = value
        .as_table_like()
        .and_then(|t| t.get("workspace"))
        .and_then(Item::as_bool)
        .unwrap_or(false);
    if !inherited {
        return Declared {
            package: field(value, "package").unwrap_or_else(|| key.to_owned()),
            git: field(value, "git"),
            path: field(value, "path"),
            base_dir: manifest.dir.clone(),
            evidence: Some(FileLine {
                file: manifest.path.clone(),
                line,
            }),
            unresolved: false,
        };
    }
    let found = root.and_then(|root| {
        let table = root.workspace()?.get("dependencies")?.as_table_like()?;
        let (declared_key, item) = table.get_key_value(key)?;
        let line = declared_key
            .span()
            .map_or(0, |span| root.lines.line(span.start));
        Some((root, item, line))
    });
    match found {
        Some((root, item, root_line)) => Declared {
            package: field(item, "package").unwrap_or_else(|| key.to_owned()),
            git: field(item, "git"),
            path: field(item, "path"),
            base_dir: root.dir.clone(),
            evidence: Some(FileLine {
                file: root.path.clone(),
                line: root_line,
            }),
            unresolved: false,
        },
        None => Declared {
            package: key.to_owned(),
            git: None,
            path: None,
            base_dir: manifest.dir.clone(),
            evidence: None,
            unresolved: true,
        },
    }
}

/// Version, source and record line of `package` in `lock`, as the lock record of `from` names it.
fn locked(
    lock: &Lock,
    from: Option<&RepoCrate>,
    package: &str,
) -> Option<(Option<String>, CrateSource, u32)> {
    let candidates: Vec<&LockPackage> =
        lock.packages.iter().filter(|p| p.name == package).collect();
    let chosen = match candidates.as_slice() {
        [] => return None,
        [only] => *only,
        _ => {
            let entry = from
                .and_then(|krate| {
                    lock.packages.iter().find(|p| {
                        p.name == krate.name && p.source.is_none() && p.version == krate.version
                    })
                })
                .and_then(|record| {
                    record
                        .dependencies
                        .iter()
                        .map(|d| lock_entry(d))
                        .find(|(name, _, _)| *name == package)
                })?;
            let (_, version, source) = entry;
            *candidates.iter().find(|p| {
                version.is_none_or(|v| p.version.as_deref() == Some(v))
                    && source.is_none_or(|s| p.source.as_deref() == Some(s))
            })?
        }
    };
    let source = chosen
        .source
        .as_deref()
        .map_or(CrateSource::Unknown, source_of);
    Some((chosen.version.clone(), source, chosen.line))
}

/// `name [version [(source)]]` of a `Cargo.lock` dependency entry.
fn lock_entry(entry: &str) -> (&str, Option<&str>, Option<&str>) {
    let mut parts = entry.splitn(3, ' ');
    let name = parts.next().unwrap_or_default();
    let version = parts.next();
    let source = parts
        .next()
        .and_then(|s| s.strip_prefix('('))
        .and_then(|s| s.strip_suffix(')'));
    (name, version, source)
}

/// A `Cargo.lock` source string.
fn source_of(source: &str) -> CrateSource {
    if let Some(index) = source
        .strip_prefix("registry+")
        .or_else(|| source.strip_prefix("sparse+"))
    {
        let crates_io = [
            "https://github.com/rust-lang/crates.io-index",
            "https://index.crates.io/",
        ];
        let registry = if crates_io.contains(&index) {
            CRATES_IO.to_owned()
        } else {
            index.to_owned()
        };
        return CrateSource::Registry { registry };
    }
    if let Some(git) = source.strip_prefix("git+") {
        let (url, rev) = match git.split_once('#') {
            Some((url, rev)) => (url, Some(rev.to_owned())),
            None => (git, None),
        };
        return CrateSource::Git {
            url: url.to_owned(),
            repo: github(url),
            rev,
        };
    }
    CrateSource::Unknown
}

fn github(url: &str) -> Option<RepoRef> {
    related::github_repo(url).map(|(owner, name)| RepoRef { owner, name })
}

/// A path dependency outside the repository: a sibling repository of the same owner when it lies one level above
/// the root.
fn sibling(pr_repo: &RepoRef, base_dir: &str, path: &str) -> CrateSource {
    let joined = if base_dir.is_empty() {
        path.to_owned()
    } else {
        format!("{base_dir}/{path}")
    };
    let mut stack: Vec<&str> = Vec::new();
    for segment in joined.split('/') {
        match segment {
            "" | "." => {}
            ".." if stack.last().is_some_and(|top| *top != "..") => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    let repo = match stack.as_slice() {
        ["..", name, ..] if *name != ".." && related::valid_name(name) => Some(RepoRef {
            owner: pr_repo.owner.clone(),
            name: (*name).to_owned(),
        }),
        _ => None,
    };
    CrateSource::Path {
        path: stack.join("/"),
        repo,
    }
}

/// A normalised directory inside the repository.
struct Inside(String);

/// `path` relative to `base_dir`, normalised; `None` when it leaves the repository or is absolute.
fn normalise(base_dir: &str, path: &str) -> Option<Inside> {
    if path.starts_with('/') || path.contains('\\') {
        return None;
    }
    let mut stack: Vec<&str> = Vec::new();
    for segment in base_dir.split('/').chain(path.split('/')) {
        match segment {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            other => stack.push(other),
        }
    }
    Some(Inside(stack.join("/")))
}

/// Dependency tables of a manifest or `target.<cfg>` table.
fn kinds(table: &dyn TableLike) -> Vec<(DependencyKind, &Item)> {
    [
        ("dependencies", DependencyKind::Normal),
        ("dev-dependencies", DependencyKind::Dev),
        ("dev_dependencies", DependencyKind::Dev),
        ("build-dependencies", DependencyKind::Build),
        ("build_dependencies", DependencyKind::Build),
    ]
    .into_iter()
    .filter_map(|(name, kind)| Some((kind, table.get(name)?)))
    .collect()
}

/// Whether a manifest value is `{ workspace = true }` (or `x.workspace = true`).
fn inherits(item: &Item) -> bool {
    item.as_table_like()
        .and_then(|t| t.get("workspace"))
        .and_then(Item::as_bool)
        .unwrap_or(false)
}

/// The name code uses for a crate or dependency key.
fn code(name: &str) -> String {
    name.replace('-', "_")
}

fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}
