//! Tech plugins: path and line classification rules and call hierarchy (rule A6; `generic` in v1).
//! `Generic::classify` is filled in by WP3-PREPASS.

use contracts::api::tour::CallHierarchy;
use contracts::common::{LineRange, LineSide, NotAnalysedCategory, ProvenanceBadge};

/// A language plugin.
pub trait TechPlugin: Send + Sync {
    /// Plugin name.
    fn name(&self) -> &'static str;
    /// Whether the plugin handles this file.
    fn detect(&self, path: &str) -> bool;
    /// Material of one side of a file that is not analysed (tests, docs and comments, generated), by path or
    /// line rule; called once per side with changed lines (removed lines on base, e.g. a deleted comment).
    fn classify(&self, file: &FileSource<'_>) -> Result<Vec<Classified>, TechError>;
    /// Callers and callees of a symbol; `None` when the plugin cannot tell.
    fn call_hierarchy(&self, symbol: &SymbolAt) -> Option<CallHierarchy>;
}

/// One side of a changed file as a plugin sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileSource<'a> {
    /// Repository-relative path on that side.
    pub path: &'a str,
    /// `head` (added lines) or `base` (removed lines).
    pub side: LineSide,
    /// Text on that side; `None` for binary files.
    pub text: Option<&'a str>,
    /// Changed lines on that side: added head lines or removed base lines.
    pub changed_lines: &'a [u32],
}

/// Material recognised by a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classified {
    /// Repository-relative path on that side.
    pub file: String,
    /// Side of `ranges`; `base` for a deleted file or removed lines.
    pub side: LineSide,
    /// Matched changed lines; `None` for the whole file.
    pub ranges: Option<Vec<LineRange>>,
    /// Section.
    pub category: NotAnalysedCategory,
    /// How it was recognised.
    pub badge: ProvenanceBadge,
    /// The rule (path glob or line regex).
    pub rule: String,
}

/// A symbol at a head line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolAt {
    /// Repository-relative path.
    pub file: String,
    /// Head line.
    pub line: u32,
    /// Symbol name.
    pub symbol: String,
}

/// A plugin failed.
#[derive(Debug, thiserror::Error)]
pub enum TechError {
    /// Declared but not implemented yet.
    #[error("the analysis operation {operation} is not implemented yet")]
    NotImplemented {
        /// The operation.
        operation: &'static str,
    },
}

/// The built-in language-agnostic plugin: path globs and line regexes; no call hierarchy in v1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Generic;

impl TechPlugin for Generic {
    fn name(&self) -> &'static str {
        "generic"
    }

    fn detect(&self, _path: &str) -> bool {
        true
    }

    fn classify(&self, _file: &FileSource<'_>) -> Result<Vec<Classified>, TechError> {
        Err(TechError::NotImplemented {
            operation: "tech::Generic::classify",
        })
    }

    fn call_hierarchy(&self, _symbol: &SymbolAt) -> Option<CallHierarchy> {
        // TODO: static call hierarchy (tree-sitter tags) belongs to the language plugins (brief §5, v2).
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_call_hierarchy_none() {
        let symbol = SymbolAt {
            file: "src/rpc/client.rs".to_owned(),
            line: 99,
            symbol: "RpcClient::send".to_owned(),
        };
        assert_eq!(Generic.call_hierarchy(&symbol), None);
        assert!(Generic.detect("any/file.ext"));
        assert_eq!(Generic.name(), "generic");
    }
}
