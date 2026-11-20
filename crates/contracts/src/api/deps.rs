//! The Dependencies view (ADR-021): the Cargo map of the head commit grouped by repository, with what each changed
//! crate adds for other crates' types (deterministic facts) and the LLM's keyword notes (separate type).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{FileRange, RepoRef};
use crate::ids::FlowId;

/// Response of `GET /api/v1/analyses/{id}/dependencies`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DependencyView {
    /// Whether the head commit has Cargo manifests.
    pub status: DependencyStatus,
    /// Owners whose repositories are "yours": the pull request's owner and the Settings list, sorted.
    pub controlled_owners: Vec<String>,
    /// What the map does not cover.
    pub limits: Vec<DependencyLimit>,
    /// Changed files in no crate (not checked).
    pub files_outside_crates: u32,
    /// Whether the LLM's notes exist for this analysis.
    pub notes: NotesState,
    /// This repository first, then other repositories (controlled first), registries and other sources.
    pub repositories: Vec<DependencyRepository>,
}

/// Whether the map exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum DependencyStatus {
    /// The head commit has at least one `Cargo.toml`.
    Available,
    /// No `Cargo.toml` at the head commit: nothing was checked ("not available", never "no dependencies").
    NotRust,
}

/// Something the map does not cover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DependencyLimit {
    /// More manifests than the server reads; the first `read` in path order were read.
    ManifestsCut {
        /// Manifests at the head commit.
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
    /// A workspace without `Cargo.lock`: versions are not resolved, sources come from the manifests.
    NoLockFile {
        /// Directory of its root manifest (`""` = repository root).
        workspace_dir: String,
    },
    /// A changed Rust file that was not read: its conversions are not checked.
    SourceUnread {
        /// Its path.
        path: String,
    },
    /// More conversion facts than the server keeps; the first ones in file order are kept.
    FactsCut {
        /// Facts kept.
        kept: u32,
    },
}

/// Whether the LLM's crate notes exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum NotesState {
    /// The plan is stored with notes (possibly none for a crate).
    Produced,
    /// The plan is not stored yet.
    Pending,
    /// The plan was stored without notes (analysed before notes existed).
    NotProduced,
}

/// A repository or other source of crates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DependencyRepository {
    /// What kind of source it is.
    pub kind: RepositoryKind,
    /// The GitHub repository, when known.
    pub repo: Option<RepoRef>,
    /// The registry name, git URL or path as Cargo names it; null for this repository.
    pub location: Option<String>,
    /// The repository's owner is controlled ("yours").
    pub controlled: bool,
    /// Its crates: changed crates first (by changed lines), then by name.
    pub crates: Vec<DependencyCrate>,
}

/// Kind of a crate source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryKind {
    /// The pull request's repository.
    This,
    /// A GitHub repository (git dependency or sibling path).
    Github,
    /// Another git URL.
    OtherGit,
    /// A registry (crates.io or another); the repository is not looked up.
    Registry,
    /// A path the server did not read.
    Path,
    /// Not determined (an inherited dependency without a workspace declaration, an unknown source).
    Unknown,
}

/// A crate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DependencyCrate {
    /// Package name.
    pub name: String,
    /// Version, if known.
    pub version: Option<String>,
    /// Manifest path of a crate of this repository.
    pub manifest: Option<String>,
    /// The `Cargo.lock` record or manifest declaration naming its source (head commit).
    pub source_at: Option<HeadLine>,
    /// Locked git commit, if any.
    pub revision: Option<String>,
    /// Changed crates depending on it directly.
    pub used_by: Vec<String>,
    /// Conversion facts naming its types.
    pub facts_about: u32,
    /// What the pull request changes in it; null for an unchanged crate.
    pub changes: Option<CrateChanges>,
}

/// A line of a file at the head commit.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
pub struct HeadLine {
    /// Repository-relative path.
    pub file: String,
    /// 1-based line.
    pub line: u32,
}

/// What the pull request changes in a crate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CrateChanges {
    /// Changed lines (added and removed) in its files.
    pub changed_lines: u32,
    /// Changed files.
    pub files: u32,
    /// Flows with lines in it, in Tour order.
    pub flows: Vec<CrateFlow>,
    /// Conversions added for types of workspace or controlled crates (deterministic), at most 50.
    pub facts: Vec<ConversionFact>,
    /// Facts beyond those listed.
    pub facts_hidden: u32,
    /// Facts about types of other crates (counted, not listed).
    pub other_facts: u32,
    /// The LLM's notes, at most 3, most changed lines first.
    pub notes: Vec<CrateNote>,
    /// Notes beyond those listed.
    pub notes_hidden: u32,
    /// Direct dependencies not listed (neither in this repository nor controlled).
    pub other_dependencies: u32,
}

/// A flow with lines in a crate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CrateFlow {
    /// Flow id.
    pub id: FlowId,
    /// Its title (LLM).
    pub title: String,
}

/// A conversion added for a type of another crate: a deterministic fact (Rust tokens), never a verdict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ConversionFact {
    /// What the line adds.
    pub kind: ConversionKind,
    /// The `impl` or `fn` line.
    pub at: HeadLine,
    /// The type's name.
    pub type_name: String,
    /// The type's path as written.
    pub written: String,
    /// The crate the type comes from.
    pub type_crate: String,
    /// The `use` line importing it, if any.
    pub imported_at: Option<HeadLine>,
    /// Where the type is defined.
    pub definition: TypeDefinition,
}

/// What a conversion fact's line adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ConversionKind {
    /// `impl From<A> for B`.
    FromImpl,
    /// `impl TryFrom<A> for B`.
    TryFromImpl,
    /// A trait implemented on the other crate's type.
    ImplOnForeignType,
    /// A function named like a conversion (`parse_`, `validate_`, …) whose signature names the type.
    ConversionFn,
}

/// Where a fact's type is defined.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeDefinition {
    /// Found in the defining crate of this repository.
    Located {
        /// The definition line.
        at: HeadLine,
    },
    /// Searched in the defining crate of this repository, not found (generated, a macro, a re-export).
    NotLocated,
    /// Not searched (search limit reached or the search failed).
    NotSearched,
    /// In another repository; not looked up.
    OtherRepository,
}

/// A keyword note of the LLM on what changed in a crate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CrateNote {
    /// The note (LLM).
    pub text: String,
    /// The changed lines of the hunks it describes, in this crate (head side when present).
    pub lines: Vec<FileRange>,
    /// Changed lines of those hunks: the ranking key.
    pub changed_lines: u32,
}
