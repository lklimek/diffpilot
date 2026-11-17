//! Related-repository detection from Cargo files, limited to the same organisation (brief §5).

use std::collections::BTreeSet;
use std::path::Path;

use contracts::common::{LineSide, RepoRef};
use contracts::ids::CommitSha;

use crate::counters::cargo;
use crate::tech::{DependencyFile, dependency_file};

/// Detection failed.
#[derive(Debug, thiserror::Error)]
pub enum RelatedError {
    /// `git` failed.
    #[error("git failed while reading Cargo files: {message}")]
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

/// Repositories of `pr_repo`'s organisation referenced by git or path dependencies in `Cargo.toml` /
/// `Cargo.lock` at `base`.
pub fn detect(
    checkout: &Path,
    base: &CommitSha,
    pr_repo: &RepoRef,
) -> Result<Vec<RepoRef>, RelatedError> {
    let _ = (checkout, base, pr_repo);
    Err(RelatedError::NotImplemented {
        operation: "related::detect",
    })
}

/// Repositories of `pr_repo`'s organisation referenced by git dependencies (`Cargo.toml` `git = …`, `Cargo.lock`
/// `source = "git+…"`) or by path dependencies one level outside the repository (`path = "../<repo>/…"`), read
/// from the Cargo files among `base_paths` through `text` (base side). Sorted, without the PR repository.
pub fn from_cargo<'t>(
    pr_repo: &RepoRef,
    base_paths: &[String],
    text: &dyn Fn(LineSide, &str) -> Option<&'t str>,
) -> Vec<RepoRef> {
    let mut names = BTreeSet::new();
    for path in base_paths {
        let (Some(kind), Some(text)) = (dependency_file(path), text(LineSide::Base, path)) else {
            continue;
        };
        match kind {
            DependencyFile::CargoToml => {
                for line in cargo::toml_lines(text) {
                    let git = cargo::string_value(line.text, "git").and_then(github_repo);
                    names.extend(
                        git.filter(|(owner, _)| owner.eq_ignore_ascii_case(&pr_repo.owner))
                            .map(|(_, name)| name),
                    );
                    let local = cargo::dependency_name(&line)
                        .and(cargo::string_value(line.text, "path"))
                        .and_then(|dependency| sibling(path, dependency));
                    names.extend(local);
                }
            }
            DependencyFile::CargoLock => {
                let sources = text
                    .lines()
                    .filter_map(|line| cargo::string_value(line, "source"));
                for source in sources.filter_map(|source| source.strip_prefix("git+")) {
                    let repo = github_repo(source)
                        .filter(|(owner, _)| owner.eq_ignore_ascii_case(&pr_repo.owner));
                    names.extend(repo.map(|(_, name)| name));
                }
            }
            DependencyFile::Other => {}
        }
    }
    let mut seen = BTreeSet::new();
    names
        .into_iter()
        .filter(|name| {
            !name.eq_ignore_ascii_case(&pr_repo.name) && seen.insert(name.to_lowercase())
        })
        .map(|name| RepoRef {
            owner: pr_repo.owner.clone(),
            name,
        })
        .collect()
}

/// `(owner, repository)` of a GitHub git URL (`https://`, `ssh://git@`, `git://`, `git@github.com:`).
fn github_repo(url: &str) -> Option<(String, String)> {
    let path = match url.split_once("://") {
        Some((_, rest)) => {
            let (authority, path) = rest.split_once('/')?;
            let host = authority.rsplit('@').next().unwrap_or(authority);
            let host = host.split(':').next().unwrap_or(host);
            host.eq_ignore_ascii_case("github.com").then_some(path)?
        }
        None => url.strip_prefix("git@github.com:")?,
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let owner = segments.next()?;
    let name = segments.next()?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    (valid_owner(owner) && valid_name(name)).then(|| (owner.to_owned(), name.to_owned()))
}

/// The sibling repository a path dependency of the manifest at `manifest` points into (exactly one level above
/// the repository root), if any.
fn sibling(manifest: &str, dependency: &str) -> Option<String> {
    if dependency.starts_with('/') {
        return None;
    }
    let mut depth = manifest.split('/').count().saturating_sub(1);
    let mut escaped = 0usize;
    for segment in dependency.split('/') {
        match segment {
            "" | "." => {}
            ".." if depth > 0 => depth -= 1,
            ".." => escaped += 1,
            name if escaped == 1 => return valid_name(name).then(|| name.to_owned()),
            _ if escaped > 1 => return None,
            _ => depth += 1,
        }
    }
    None
}

fn valid_owner(owner: &str) -> bool {
    (1..=39).contains(&owner.len()) && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn valid_name(name: &str) -> bool {
    (1..=100).contains(&name.len())
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn repo(owner: &str, name: &str) -> RepoRef {
        RepoRef {
            owner: owner.to_owned(),
            name: name.to_owned(),
        }
    }

    #[test]
    fn related_repos_same_org_only() {
        let root = r#"[workspace]
members = ["crates/*"]

[workspace.dependencies]
dashcore = { git = "https://github.com/dashpay/rust-dashcore", tag = "v0.40" }
grovedb = { git = "ssh://git@github.com/DashPay/grovedb.git", branch = "develop" }
serde = { git = "https://github.com/serde-rs/serde" }
self-ref = { git = "https://github.com/dashpay/platform" }
lookalike = { git = "https://github.com.evil.example/dashpay/fake" }

[package]
documentation = "https://github.com/dashpay/not-a-dependency"
"#;
        let member = r#"[package]
name = "a"

[lib]
path = "../../../outside/lib.rs"

[dependencies]
tenderdash-abci = { path = "../../../tenderdash-abci/abci" }
inner = { path = "../b" }
far = { path = "../../../../far/x" }
"#;
        let lock = r#"[[package]]
name = "bls"
source = "git+https://github.com/dashpay/bls-signatures?rev=1#abc"

[[package]]
name = "x"
source = "git+https://github.com/other/x#1"

[[package]]
name = "y"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
        let texts: HashMap<&str, &str> = [
            ("Cargo.toml", root),
            ("crates/a/Cargo.toml", member),
            ("Cargo.lock", lock),
        ]
        .into_iter()
        .collect();
        let paths: Vec<String> = [
            "Cargo.toml",
            "crates/a/Cargo.toml",
            "Cargo.lock",
            "src/lib.rs",
        ]
        .map(str::to_owned)
        .to_vec();
        let found = from_cargo(&repo("dashpay", "platform"), &paths, &|side, path| {
            assert_eq!(side, LineSide::Base, "Cargo files are read on the base ref");
            texts.get(path).copied()
        });
        assert_eq!(
            found,
            [
                repo("dashpay", "bls-signatures"),
                repo("dashpay", "grovedb"),
                repo("dashpay", "rust-dashcore"),
                repo("dashpay", "tenderdash-abci"),
            ]
        );
    }

    #[test]
    fn github_urls_and_sibling_paths() {
        let pair = |o: &str, n: &str| Some((o.to_owned(), n.to_owned()));
        assert_eq!(
            github_repo("https://github.com/acme/x.git"),
            pair("acme", "x")
        );
        assert_eq!(github_repo("git@github.com:acme/x"), pair("acme", "x"));
        assert_eq!(
            github_repo("https://user@GitHub.com:443/acme/x/"),
            pair("acme", "x")
        );
        assert_eq!(github_repo("https://gitlab.com/acme/x"), None);
        assert_eq!(github_repo("https://github.com/acme"), None);
        assert_eq!(github_repo("https://github.com/ac me/x"), None);
        assert_eq!(
            sibling("Cargo.toml", "../dashcore/dash"),
            Some("dashcore".to_owned())
        );
        assert_eq!(
            sibling("crates/a/Cargo.toml", "../../../core"),
            Some("core".to_owned())
        );
        assert_eq!(sibling("crates/a/Cargo.toml", "../b"), None);
        assert_eq!(sibling("Cargo.toml", "../../x/y"), None);
        assert_eq!(sibling("Cargo.toml", "/abs/x"), None);
        assert_eq!(sibling("Cargo.toml", "../.."), None);
    }
}
