//! Owner-specific skill rules as per-user settings with code-constant fallbacks (plan §C.2).

use std::collections::HashMap;

use contracts::api::dashboard::DashboardSettings;
use contracts::common::RepoRef;
use uuid::Uuid;

/// Built-in release and long-lived branch patterns (K9), used when a repository row sets none: `*` is any text,
/// `X.Y` a version of two or more numeric parts.
pub const BUILTIN_BRANCH_PATTERNS: [&str; 6] = [
    "main",
    "master",
    "release/*",
    "hotfix/*",
    "vX.Y",
    "vX.Y-dev",
];

/// Rules of one user's ranking, from the dashboard settings and the signed-in login.
#[derive(Clone, Debug)]
pub struct ScoreRules {
    scanned: Vec<String>,
    ready_label: String,
    skip_authors: Vec<String>,
    known_bots: Vec<String>,
    repos: Vec<RepoRule>,
    builtin_branches: Vec<BranchPattern>,
}

#[derive(Clone, Debug)]
struct RepoRule {
    repo: RepoRef,
    alias: Option<String>,
    branches: Vec<BranchPattern>,
}

impl ScoreRules {
    /// Rules of `own_login` with `settings` (the defaults when the user never saved any).
    pub fn new(own_login: &str, settings: &DashboardSettings) -> Self {
        let lower = |s: &String| s.to_lowercase();
        Self {
            scanned: std::iter::once(own_login.to_lowercase())
                .chain(settings.linked_logins.iter().map(lower))
                .collect(),
            ready_label: settings.ready_label.to_lowercase(),
            skip_authors: settings.skip_authors.iter().map(lower).collect(),
            known_bots: settings.known_bots.iter().map(lower).collect(),
            repos: settings
                .repositories
                .iter()
                .map(|row| RepoRule {
                    repo: row.repo.clone(),
                    alias: row.alias.clone(),
                    branches: row
                        .branch_patterns
                        .iter()
                        .map(|p| BranchPattern::new(p))
                        .collect(),
                })
                .collect(),
            builtin_branches: BUILTIN_BRANCH_PATTERNS
                .iter()
                .map(|p| BranchPattern::new(p))
                .collect(),
        }
    }

    /// The login is the user's own or a linked one (K1).
    pub fn is_scanned(&self, login: &str) -> bool {
        self.scanned.contains(&login.to_lowercase())
    }

    /// The label marks a pull request ready for a human (K6).
    pub fn is_ready_label(&self, label: &str) -> bool {
        label.to_lowercase() == self.ready_label
    }

    /// Everything this author wrote is left out of the dashboard (K7).
    pub fn is_skipped_author(&self, login: &str) -> bool {
        self.skip_authors.contains(&login.to_lowercase())
    }

    /// Not a human: GitHub `Bot`, a `[bot]` login or one of the user's known bots (K11).
    pub fn is_bot(&self, login: &str, typename_bot: bool) -> bool {
        let low = login.to_lowercase();
        typename_bot || low.ends_with("[bot]") || self.known_bots.contains(&low)
    }

    fn row(&self, repo: &RepoRef) -> Option<&RepoRule> {
        self.repos.iter().find(|row| {
            row.repo.owner.eq_ignore_ascii_case(&repo.owner)
                && row.repo.name.eq_ignore_ascii_case(&repo.name)
        })
    }

    /// Display id (K8): `<alias>-<n>` with the row's alias, else `<repository name>#<n>`.
    pub fn display_id(&self, repo: &RepoRef, number: u32) -> String {
        match self.row(repo).and_then(|row| row.alias.as_deref()) {
            Some(alias) => format!("{alias}-{number}"),
            None => format!("{}#{number}", repo.name),
        }
    }

    fn branch_patterns(&self, repo: &RepoRef) -> &[BranchPattern] {
        match self.row(repo) {
            Some(row) if !row.branches.is_empty() => &row.branches,
            _ => &self.builtin_branches,
        }
    }

    /// A pull request based on this branch is not stacked on another (K9): the default branch or a pattern match.
    pub fn is_long_lived(
        &self,
        repo: &RepoRef,
        branch: &str,
        default_branch: Option<&str>,
    ) -> bool {
        default_branch == Some(branch)
            || self.branch_patterns(repo).iter().any(|p| p.matches(branch))
    }

    /// The pull request targets a release line (K9): a pattern match other than the default branch.
    pub fn targets_release_branch(
        &self,
        repo: &RepoRef,
        base: &str,
        default_branch: Option<&str>,
    ) -> bool {
        default_branch != Some(base) && self.branch_patterns(repo).iter().any(|p| p.matches(base))
    }
}

/// Display ids `TODO-<prefix>` of MemCan TODOs: the shortest unique prefix of at least 8 characters.
pub fn todo_display_ids(ids: &[Uuid]) -> HashMap<Uuid, String> {
    let texts: Vec<String> = ids.iter().map(|id| id.hyphenated().to_string()).collect();
    ids.iter()
        .zip(&texts)
        .map(|(id, full)| {
            let mut n = 8.min(full.len());
            while n < full.len()
                && texts
                    .iter()
                    .any(|other| other != full && other.starts_with(&full[..n]))
            {
                n += 1;
            }
            (*id, format!("TODO-{}", &full[..n]))
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Char(char),
    Any,
    Version,
}

/// A branch pattern of the settings (plan §F.6 G7): literal characters, `*` and `X.Y`.
#[derive(Clone, Debug)]
pub(super) struct BranchPattern(Vec<Token>);

impl BranchPattern {
    pub(super) fn new(pattern: &str) -> Self {
        let mut tokens = Vec::new();
        let mut rest = pattern;
        while let Some(c) = rest.chars().next() {
            if let Some(after) = rest.strip_prefix("X.Y") {
                tokens.push(Token::Version);
                rest = after;
            } else {
                tokens.push(if c == '*' { Token::Any } else { Token::Char(c) });
                rest = &rest[c.len_utf8()..];
            }
        }
        Self(tokens)
    }

    /// Whole-name match, memoised (no exponential backtracking on many `*`).
    pub(super) fn matches(&self, branch: &str) -> bool {
        let text: Vec<char> = branch.chars().collect();
        let mut memo = vec![None; (self.0.len() + 1) * (text.len() + 1)];
        self.match_at(&text, 0, 0, &mut memo)
    }

    fn match_at(&self, text: &[char], t: usize, s: usize, memo: &mut [Option<bool>]) -> bool {
        let slot = t * (text.len() + 1) + s;
        if let Some(known) = memo.get(slot).copied().flatten() {
            return known;
        }
        let result = match self.0.get(t) {
            None => s == text.len(),
            Some(Token::Char(c)) => {
                text.get(s) == Some(c) && self.match_at(text, t + 1, s + 1, memo)
            }
            Some(Token::Any) => (s..=text.len()).any(|end| self.match_at(text, t + 1, end, memo)),
            Some(Token::Version) => version_ends(&text[s..])
                .into_iter()
                .any(|len| self.match_at(text, t + 1, s + len, memo)),
        };
        if let Some(cell) = memo.get_mut(slot) {
            *cell = Some(result);
        }
        result
    }
}

/// Lengths of every prefix of `text` of the form `\d+(\.\d+)+`.
fn version_ends(text: &[char]) -> Vec<usize> {
    let digits = |from: usize| {
        text[from..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count()
    };
    let mut ends = Vec::new();
    let mut at = digits(0);
    if at == 0 {
        return ends;
    }
    while text.get(at) == Some(&'.') {
        let run = digits(at + 1);
        if run == 0 {
            break;
        }
        for len in 1..=run {
            ends.push(at + 1 + len);
        }
        at += 1 + run;
    }
    ends
}
