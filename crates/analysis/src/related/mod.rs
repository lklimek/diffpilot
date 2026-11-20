//! Related-repository detection from Cargo files, limited to the same organisation (brief §5).

use std::collections::BTreeSet;

use contracts::common::{LineSide, RepoRef};

use crate::counters::cargo;
use crate::tech::{DependencyFile, dependency_file};

/// Repositories of `pr_repo`'s organisation referenced by git dependencies (`Cargo.toml` `git = …`, `Cargo.lock`
/// `source = "git+…"`) or by path dependencies one level outside the repository (`path = "../<repo>/…"`), read
/// from the dependency declarations and package records of the Cargo files among `base_paths` through `text`
/// (base side; a file that does not parse adds nothing). Sorted, without the PR repository.
pub fn from_cargo<'t>(
    pr_repo: &RepoRef,
    base_paths: &[String],
    text: &dyn Fn(LineSide, &str) -> Option<&'t str>,
) -> Vec<RepoRef> {
    let mut local_names = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut add = |url: &str| {
        let repo = github_repo(url).filter(|(owner, _)| owner.eq_ignore_ascii_case(&pr_repo.owner));
        names.extend(repo.map(|(_, name)| name));
    };
    for path in base_paths {
        let (Some(kind), Some(text)) = (dependency_file(path), text(LineSide::Base, path)) else {
            continue;
        };
        match kind {
            DependencyFile::CargoToml => {
                for declaration in cargo::manifest(text).unwrap_or_default() {
                    if let Some(git) = &declaration.git {
                        add(git);
                    }
                    let local = declaration
                        .path
                        .as_deref()
                        .and_then(|dependency| sibling(path, dependency));
                    if let Some(name) = local {
                        local_names.insert(name);
                    }
                }
            }
            DependencyFile::CargoLock => {
                let sources = cargo::lock(text)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|package| package.source);
                for source in sources {
                    if let Some(url) = source.strip_prefix("git+") {
                        add(url);
                    }
                }
            }
            DependencyFile::Other => {}
        }
    }
    names.append(&mut local_names);
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
pub(crate) fn github_repo(url: &str) -> Option<(String, String)> {
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

/// The sibling repository a path dependency of the manifest at `manifest` points into: the path is normalised
/// relative to the repository root, and only a target exactly one level above the root names a sibling.
fn sibling(manifest: &str, dependency: &str) -> Option<String> {
    if dependency.starts_with('/') || dependency.contains('\\') {
        return None;
    }
    let directory = manifest
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);
    let mut stack: Vec<&str> = Vec::new();
    for segment in directory.split('/').chain(dependency.split('/')) {
        match segment {
            "" | "." => {}
            ".." if stack.last().is_some_and(|top| *top != "..") => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    match stack.as_slice() {
        ["..", name, ..] if *name != ".." && valid_name(name) => Some((*name).to_owned()),
        _ => None,
    }
}

fn valid_owner(owner: &str) -> bool {
    (1..=39).contains(&owner.len()) && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub(crate) fn valid_name(name: &str) -> bool {
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

[package.metadata.release]
git = "https://github.com/dashpay/release-metadata"
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
        let lock = r#"# source = "git+https://github.com/dashpay/in-a-comment"
[[package]]
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
        assert_eq!(
            sibling("Cargo.toml", "../foo/../bar"),
            Some("bar".to_owned())
        );
        assert_eq!(sibling("Cargo.toml", "../foo/../../outside"), None);
        assert_eq!(
            sibling("Cargo.toml", "../foo/.."),
            None,
            "the parent of the root itself"
        );
        assert_eq!(
            sibling("crates/a/Cargo.toml", "../../x/../../core/y"),
            Some("core".to_owned())
        );
        assert_eq!(
            sibling("Cargo.toml", "inner/../../sib"),
            Some("sib".to_owned())
        );
    }
}
