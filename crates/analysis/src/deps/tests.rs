use std::collections::HashMap;

use proptest::prelude::*;

use super::*;

fn repo(owner: &str, name: &str) -> RepoRef {
    RepoRef {
        owner: owner.to_owned(),
        name: name.to_owned(),
    }
}

fn pr_repo() -> RepoRef {
    repo("dashpay", "dash-evo-tool")
}

/// Builds the map of `files` (path → text); every path is in the head tree.
fn map_of(files: &[(&str, &str)]) -> DependencyMap {
    let texts: HashMap<&str, &str> = files.iter().copied().collect();
    let mut paths: Vec<String> = files.iter().map(|(path, _)| (*path).to_owned()).collect();
    paths.push("README.md".to_owned());
    paths.push("crates/a/src/lib.rs".to_owned());
    map(&pr_repo(), &paths, &|path| texts.get(path).copied())
}

fn krate<'m>(map: &'m DependencyMap, name: &str) -> &'m RepoCrate {
    map.crates
        .iter()
        .find(|c| c.name == name)
        .expect("crate listed")
}

fn dependency<'m>(krate: &'m RepoCrate, package: &str) -> &'m Dependency {
    krate
        .dependencies
        .iter()
        .find(|d| d.package == package)
        .expect("dependency listed")
}

fn external<'m>(map: &'m DependencyMap, dependency: &Dependency) -> &'m ExternalCrate {
    match dependency.target {
        Target::External { index } => &map.external[index as usize],
        Target::Repo { .. } => panic!("expected an external crate"),
    }
}

const ROOT: &str = r#"[workspace]
members = ["crates/*"]
exclude = ["crates/excluded"]

[workspace.package]
version = "1.4.0"

[workspace.dependencies]
dpp = { git = "https://github.com/dashpay/platform", tag = "v2.0.0" }
serde = "1"
"#;

const CRATE_A: &str = r#"[package]
name = "a-app"
version = "0.1.0"

[dependencies]
dpp = { workspace = true }
b = { path = "../b" }
serde.workspace = true
my-json = { package = "serde_json", version = "1" }
dashcore = { path = "../../../rust-dashcore/dash" }

[dev-dependencies]
tokio = "1"

[target.'cfg(unix)'.build-dependencies]
cc = "1"
"#;

const CRATE_B: &str = r#"[package]
name = "b"
version.workspace = true

[lib]
name = "bee"
"#;

const LOCK: &str = r#"version = 4

[[package]]
name = "a-app"
version = "0.1.0"
dependencies = [
 "b",
 "dpp",
 "serde 1.0.200",
 "serde_json",
]

[[package]]
name = "b"
version = "1.4.0"

[[package]]
name = "dpp"
version = "2.0.0"
source = "git+https://github.com/dashpay/platform?tag=v2.0.0#0123abcd"

[[package]]
name = "serde"
version = "1.0.200"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "serde"
version = "0.9.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "serde_json"
version = "1.0.1"
source = "sparse+https://index.crates.io/"
"#;

fn workspace() -> DependencyMap {
    map_of(&[
        ("Cargo.toml", ROOT),
        ("Cargo.lock", LOCK),
        ("crates/a/Cargo.toml", CRATE_A),
        ("crates/b/Cargo.toml", CRATE_B),
    ])
}

#[test]
fn deps_repository_without_cargo_is_not_rust() {
    let map = map_of(&[]);
    assert_eq!(map.status, MapStatus::NotRust);
    assert!(map.crates.is_empty());
    assert!(map.limits.is_empty());
}

#[test]
fn deps_workspace_crates_with_names_versions_and_code_names() {
    let map = workspace();
    assert_eq!(map.status, MapStatus::Available);
    let names: Vec<&str> = map.crates.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["a-app", "b"], "the virtual root is no crate");
    let a = krate(&map, "a-app");
    assert_eq!(a.dir, "crates/a");
    assert_eq!(a.manifest, "crates/a/Cargo.toml");
    assert_eq!(a.code_name, "a_app");
    let b = krate(&map, "b");
    assert_eq!(b.code_name, "bee", "`[lib] name`");
    assert_eq!(b.version.as_deref(), Some("1.4.0"), "inherited version");
    assert!(map.limits.is_empty(), "{:?}", map.limits);
}

#[test]
fn deps_sources_come_from_the_lock_file() {
    let map = workspace();
    let a = krate(&map, "a-app");

    let dpp = dependency(a, "dpp");
    assert_eq!(dpp.code_name, "dpp");
    assert_eq!(dpp.kind, DependencyKind::Normal);
    assert_eq!(dpp.line, 6);
    let dpp = external(&map, dpp);
    assert_eq!(dpp.version.as_deref(), Some("2.0.0"));
    assert_eq!(
        dpp.source,
        CrateSource::Git {
            url: "https://github.com/dashpay/platform?tag=v2.0.0".to_owned(),
            repo: Some(repo("dashpay", "platform")),
            rev: Some("0123abcd".to_owned()),
        }
    );
    assert_eq!(
        dpp.evidence,
        Some(FileLine {
            file: "Cargo.lock".to_owned(),
            line: 17
        })
    );

    let serde = external(&map, dependency(a, "serde"));
    assert_eq!(
        serde.version.as_deref(),
        Some("1.0.200"),
        "the version the crate's lock record names, not the other one"
    );
    assert_eq!(
        serde.source,
        CrateSource::Registry {
            registry: CRATES_IO.to_owned()
        }
    );

    let json = dependency(a, "serde_json");
    assert_eq!(
        json.code_name, "my_json",
        "a renamed dependency is used by its key"
    );
    let json = external(&map, json);
    assert_eq!(json.version.as_deref(), Some("1.0.1"));
    assert_eq!(
        json.source,
        CrateSource::Registry {
            registry: CRATES_IO.to_owned()
        },
        "the sparse index is crates.io"
    );
}

#[test]
fn deps_path_dependencies_inside_and_outside_the_repository() {
    let map = workspace();
    let a = krate(&map, "a-app");
    let b = dependency(a, "b");
    assert_eq!(
        b.code_name, "bee",
        "a workspace crate is used by its lib name"
    );
    let Target::Repo { index } = b.target else {
        panic!("a workspace crate");
    };
    assert_eq!(map.crates[index as usize].name, "b");

    let dashcore = external(&map, dependency(a, "dashcore"));
    assert_eq!(
        dashcore.source,
        CrateSource::Path {
            path: "../rust-dashcore/dash".to_owned(),
            repo: Some(repo("dashpay", "rust-dashcore")),
        },
        "a sibling repository of the same owner"
    );
}

#[test]
fn deps_kinds_and_crates_missing_from_the_lock_file() {
    let map = workspace();
    let a = krate(&map, "a-app");
    let tokio = dependency(a, "tokio");
    assert_eq!(tokio.kind, DependencyKind::Dev);
    let tokio = external(&map, tokio);
    assert_eq!(
        tokio.version, None,
        "not resolved: absent from the lock file"
    );
    assert_eq!(
        tokio.evidence,
        Some(FileLine {
            file: "crates/a/Cargo.toml".to_owned(),
            line: 13
        }),
        "the manifest declaration says where it comes from"
    );
    assert_eq!(dependency(a, "cc").kind, DependencyKind::Build);
}

#[test]
fn deps_without_lock_file_take_sources_from_manifests() {
    let map = map_of(&[("Cargo.toml", ROOT), ("crates/a/Cargo.toml", CRATE_A)]);
    assert!(
        map.limits.contains(&MapLimit::NoLockFile {
            workspace_dir: String::new()
        }),
        "{:?}",
        map.limits
    );
    let a = krate(&map, "a-app");
    let dpp = external(&map, dependency(a, "dpp"));
    assert_eq!(dpp.version, None);
    assert_eq!(
        dpp.source,
        CrateSource::Git {
            url: "https://github.com/dashpay/platform".to_owned(),
            repo: Some(repo("dashpay", "platform")),
            rev: None,
        },
        "the workspace declaration"
    );
    assert_eq!(
        dpp.evidence,
        Some(FileLine {
            file: "Cargo.toml".to_owned(),
            line: 9
        }),
        "the declaration in the workspace manifest"
    );
}

#[test]
fn deps_unreadable_manifest_is_a_limit_not_an_absence() {
    let map = map_of(&[
        ("Cargo.toml", ROOT),
        ("Cargo.lock", LOCK),
        ("crates/a/Cargo.toml", CRATE_A),
        ("crates/b/Cargo.toml", "[package\nname = "),
    ]);
    assert!(map.limits.contains(&MapLimit::ManifestUnreadable {
        path: "crates/b/Cargo.toml".to_owned()
    }));
    assert_eq!(map.crates.len(), 1);
    let b = dependency(krate(&map, "a-app"), "b");
    let b = external(&map, b);
    assert_eq!(
        b.source,
        CrateSource::Path {
            path: "crates/b".to_owned(),
            repo: None
        },
        "a path inside the repository whose manifest was not read"
    );
}

#[test]
fn deps_unparseable_lock_is_a_limit() {
    let map = map_of(&[
        ("Cargo.toml", ROOT),
        ("Cargo.lock", "[[package"),
        ("crates/a/Cargo.toml", CRATE_A),
    ]);
    assert!(map.limits.contains(&MapLimit::LockUnreadable {
        path: "Cargo.lock".to_owned()
    }));
}

#[test]
fn deps_excluded_crate_inherits_nothing() {
    let excluded = r#"[package]
name = "x"

[dependencies]
dpp = { workspace = true }
"#;
    let map = map_of(&[
        ("Cargo.toml", ROOT),
        ("crates/excluded/Cargo.toml", excluded),
    ]);
    let x = krate(&map, "x");
    let dpp = external(&map, dependency(x, "dpp"));
    assert_eq!(dpp.source, CrateSource::Unknown);
}

#[test]
fn deps_nested_workspaces_use_their_own_root() {
    let nested_root = r#"[workspace]
members = ["inner"]

[workspace.dependencies]
dpp = { git = "https://github.com/acme/other" }
"#;
    let inner = r#"[package]
name = "inner"

[dependencies]
dpp.workspace = true
"#;
    let map = map_of(&[
        ("Cargo.toml", ROOT),
        ("tools/Cargo.toml", nested_root),
        ("tools/inner/Cargo.toml", inner),
    ]);
    let inner = krate(&map, "inner");
    let dpp = external(&map, dependency(inner, "dpp"));
    assert_eq!(
        dpp.source,
        CrateSource::Git {
            url: "https://github.com/acme/other".to_owned(),
            repo: Some(repo("acme", "other")),
            rev: None
        }
    );
    assert!(map.limits.contains(&MapLimit::NoLockFile {
        workspace_dir: "tools".to_owned()
    }));
}

#[test]
fn deps_crate_of_a_path_is_the_deepest_crate() {
    let root_package = r#"[package]
name = "root"

[workspace]
members = ["crates/*"]
"#;
    let map = map_of(&[
        ("Cargo.toml", root_package),
        ("crates/a/Cargo.toml", CRATE_A),
        ("crates/ab/Cargo.toml", "[package]\nname = \"ab\"\n"),
    ]);
    let name = |path: &str| map.crate_of(path).map(|i| map.crates[i].name.as_str());
    assert_eq!(name("crates/a/src/lib.rs"), Some("a-app"));
    assert_eq!(
        name("crates/ab/src/lib.rs"),
        Some("ab"),
        "not a prefix match"
    );
    assert_eq!(name("src/main.rs"), Some("root"));
    assert_eq!(name("crates/c/src/lib.rs"), Some("root"));
}

#[test]
fn deps_crate_of_without_root_crate_is_none() {
    let map = workspace();
    assert_eq!(map.crate_of("docs/x.md"), None);
}

#[test]
fn deps_manifests_beyond_the_cap_are_a_limit() {
    let paths: Vec<String> = (0..MAX_MANIFESTS + 3)
        .map(|i| format!("c{i:04}/Cargo.toml"))
        .collect();
    let wanted = cargo_files_to_read(&paths);
    assert_eq!(wanted.len(), MAX_MANIFESTS);
    let text = "[package]\nname = \"c\"\n";
    let built = map(&pr_repo(), &paths, &|_| Some(text));
    assert_eq!(built.crates.len(), MAX_MANIFESTS);
    assert!(built.limits.contains(&MapLimit::ManifestsCut {
        total: u32::try_from(MAX_MANIFESTS + 3).unwrap_or(u32::MAX),
        read: u32::try_from(MAX_MANIFESTS).unwrap_or(u32::MAX),
    }));
}

#[test]
fn deps_files_to_read_are_manifests_and_lock_files() {
    let paths: Vec<String> = [
        "Cargo.toml",
        "Cargo.lock",
        "src/lib.rs",
        "crates/a/Cargo.toml",
        "crates/a/Cargo.toml.orig",
        "docs/Cargo.lock.md",
    ]
    .iter()
    .map(|p| (*p).to_owned())
    .collect();
    assert_eq!(
        cargo_files_to_read(&paths),
        ["Cargo.lock", "Cargo.toml", "crates/a/Cargo.toml"]
    );
}

#[test]
fn deps_text_beyond_the_total_budget_is_a_limit() {
    let big = format!(
        "[package]\nname = \"big\"\ndescription = \"{}\"\n",
        "x".repeat(MAX_CARGO_TEXT_BYTES / 5)
    );
    let names = ["a", "b", "c", "d", "e", "f"];
    let paths: Vec<String> = names.iter().map(|n| format!("{n}/Cargo.toml")).collect();
    let files: Vec<(&str, &str)> = paths.iter().map(|p| (p.as_str(), big.as_str())).collect();
    let built = map_of(&files);
    assert_eq!(built.crates.len(), 4, "{:?}", built.limits);
    for path in ["e/Cargo.toml", "f/Cargo.toml"] {
        assert!(
            built.limits.contains(&MapLimit::ManifestUnreadable {
                path: path.to_owned()
            }),
            "{path}: {:?}",
            built.limits
        );
    }
}

#[test]
fn deps_lock_dependency_entries_parse() {
    assert_eq!(lock_entry("serde"), ("serde", None, None));
    assert_eq!(lock_entry("serde 1.0.1"), ("serde", Some("1.0.1"), None));
    assert_eq!(
        lock_entry("dpp 2.0.0 (git+https://x#ab)"),
        ("dpp", Some("2.0.0"), Some("git+https://x#ab"))
    );
}

#[test]
fn deps_source_strings_classify() {
    assert_eq!(
        source_of("registry+https://example.com/index"),
        CrateSource::Registry {
            registry: "https://example.com/index".to_owned()
        }
    );
    assert_eq!(
        source_of("git+ssh://git@github.com/DashPay/grovedb.git?branch=develop#ff"),
        CrateSource::Git {
            url: "ssh://git@github.com/DashPay/grovedb.git?branch=develop".to_owned(),
            repo: Some(repo("DashPay", "grovedb")),
            rev: Some("ff".to_owned()),
        }
    );
    assert_eq!(source_of("path+file:///x"), CrateSource::Unknown);
}

proptest! {
    #[test]
    fn deps_never_panic_on_arbitrary_manifests(manifest in ".{0,400}", lock in ".{0,400}") {
        let _ = map_of(&[
            ("Cargo.toml", manifest.as_str()),
            ("Cargo.lock", lock.as_str()),
            ("x/Cargo.toml", manifest.as_str()),
        ]);
    }
}

const SOURCE: &str = "use dpp::identity::Identity;\nuse bee::Height;\nuse crate::model::Local;\nimpl From<Identity> for Local {}\nimpl TryFrom<Height> for Local {}\npub fn parse_height(s: &str) -> Height { todo() }\nimpl From<std::fmt::Error> for Local {}\nimpl From<a_app::X> for Local {}\n";

fn with_facts() -> DependencyMap {
    let mut map = workspace();
    map.add_facts(
        "crates/a/src/lib.rs",
        Some(SOURCE),
        &[1, 2, 3, 4, 5, 6, 7, 8],
    );
    map
}

#[test]
fn deps_facts_name_the_crate_of_each_type() {
    let map = with_facts();
    let facts: Vec<(ConversionKind, u32, &str)> = map
        .facts
        .iter()
        .map(|f| (f.kind, f.at.line, f.type_name.as_str()))
        .collect();
    assert_eq!(
        facts,
        [
            (ConversionKind::FromImpl, 4, "Identity"),
            (ConversionKind::TryFromImpl, 5, "Height"),
            (ConversionKind::ConversionFn, 6, "Height"),
        ],
        "local, std and own-crate types are no facts"
    );
    let identity = &map.facts[0];
    assert_eq!(identity.crate_index, 0);
    assert_eq!(
        external(
            &map,
            &Dependency {
                code_name: String::new(),
                package: String::new(),
                kind: DependencyKind::Normal,
                line: 0,
                target: identity.target,
            }
        )
        .package,
        "dpp"
    );
    assert_eq!(
        identity.imported_at,
        Some(FileLine {
            file: "crates/a/src/lib.rs".to_owned(),
            line: 1
        })
    );
    assert_eq!(identity.definition, Definition::OtherRepository);
    let height = &map.facts[1];
    assert_eq!(height.target, Target::Repo { index: 1 });
    assert_eq!(height.definition, Definition::NotSearched);
}

#[test]
fn deps_unread_source_is_a_limit() {
    let mut map = workspace();
    map.add_facts("crates/a/src/big.rs", None, &[1]);
    assert!(map.limits.contains(&MapLimit::SourceUnread {
        path: "crates/a/src/big.rs".to_owned()
    }));
    map.add_facts("docs/x.rs", None, &[1]);
    assert_eq!(
        map.limits.len(),
        1,
        "a file outside every crate is not checked here"
    );
}

#[test]
fn deps_definitions_are_searched_in_the_defining_crate() {
    let mut map = with_facts();
    let queries = map.definition_queries();
    assert_eq!(
        queries,
        [DefinitionQuery {
            dir: "crates/b".to_owned(),
            names: vec!["Height".to_owned()]
        }]
    );
    let files = vec![
        (
            "crates/a/src/h.rs".to_owned(),
            "pub struct Height;\n".to_owned(),
        ),
        (
            "crates/b/src/z.rs".to_owned(),
            "\npub struct Height(u64);\n".to_owned(),
        ),
        (
            "crates/b/src/m.rs".to_owned(),
            "// struct Height\n".to_owned(),
        ),
    ];
    map.locate(&queries, &files, &[]);
    let expected = Definition::Located {
        at: FileLine {
            file: "crates/b/src/z.rs".to_owned(),
            line: 2,
        },
    };
    assert_eq!(
        map.facts[1].definition, expected,
        "not in another crate, not in a comment"
    );
    assert_eq!(map.facts[2].definition, expected);
}

#[test]
fn deps_definition_not_found_is_not_located() {
    let mut map = with_facts();
    let queries = map.definition_queries();
    map.locate(&queries, &[], &[]);
    assert_eq!(map.facts[1].definition, Definition::NotLocated);
    assert_eq!(map.facts[0].definition, Definition::OtherRepository);
}

#[test]
fn deps_unsearched_names_stay_not_searched() {
    let mut map = with_facts();
    map.locate(&[], &[], &[]);
    assert_eq!(map.facts[1].definition, Definition::NotSearched);
}

#[test]
fn deps_definition_files_not_read_leave_definitions_not_searched() {
    let mut map = with_facts();
    let queries = map.definition_queries();
    let unread = [
        "crates/b/src/huge.rs".to_owned(),
        "crates/b/src/other.rs".to_owned(),
    ];
    map.locate(&queries, &[], &unread);
    assert_eq!(
        map.facts[1].definition,
        Definition::NotSearched,
        "a file found but not read may hold the definition"
    );
    assert_eq!(
        map.limits,
        [MapLimit::DefinitionsUnread {
            dir: "crates/b".to_owned(),
            files: 2
        }]
    );
    let mut located = with_facts();
    let files = [(
        "crates/b/src/z.rs".to_owned(),
        "pub struct Height;\n".to_owned(),
    )];
    located.locate(&queries, &files, &unread);
    assert!(
        matches!(located.facts[1].definition, Definition::Located { .. }),
        "a definition found in a read file is located"
    );
}

// Independent review (GO9 part A): contract tests written from ADR-021 and Cargo semantics.

#[test]
fn review_conversion_candidates_stay_linear_in_the_tokens() {
    // A hostile changed file: conversion-named functions without a body. Each signature must not run to the end
    // of the file and name every later token (quadratic time and memory in the trusted server).
    let n = 1000;
    let text = format!("{}\n", "fn to_a ".repeat(n));
    let got = tech::conversion_candidates(&text, &[1]);
    assert!(
        got.len() <= 2 * n,
        "{} candidates for {n} functions",
        got.len()
    );
    let text = format!("{}\n", "]impl a for b ".repeat(n));
    let got = tech::conversion_candidates(&text, &[1]);
    assert!(got.len() <= 2 * n, "{} candidates for {n} impls", got.len());
}

#[test]
fn review_explicit_member_wins_over_exclude() {
    // Cargo: a path in `members` stays a member even when an `exclude` entry covers it.
    let root = "[workspace]\nmembers = [\"crates/a\"]\nexclude = [\"crates\"]\n\n[workspace.dependencies]\ndpp = { git = \"https://github.com/dashpay/platform\" }\n";
    let a =
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[dependencies]\ndpp.workspace = true\n";
    let map = map_of(&[("Cargo.toml", root), ("crates/a/Cargo.toml", a)]);
    let dpp = external(&map, dependency(krate(&map, "a"), "dpp"));
    assert_ne!(dpp.source, CrateSource::Unknown, "{:?}", map.limits);
}

#[test]
fn review_package_workspace_pointer_is_not_a_missing_lock() {
    // ADR-021 §1: `package.workspace` pointers are a reported gap, never guessed. The pointed-to workspace has a
    // Cargo.lock, so "no Cargo.lock" is a false limit.
    let ws = "[workspace]\nmembers = [\"../shared\"]\n";
    let shared = "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nworkspace = \"../ws\"\n";
    let map = map_of(&[
        ("ws/Cargo.toml", ws),
        ("ws/Cargo.lock", "version = 4\n"),
        ("shared/Cargo.toml", shared),
    ]);
    assert!(
        !map.limits.contains(&MapLimit::NoLockFile {
            workspace_dir: "shared".to_owned()
        }),
        "{:?}",
        map.limits
    );
}

#[test]
fn review_unsearched_path_crate_is_not_claimed_not_located() {
    // `NotLocated` means "searched in the defining crate and not found"; a crate whose manifest was not read was
    // never searched.
    let mut map = map_of(&[
        ("Cargo.toml", ROOT),
        ("Cargo.lock", LOCK),
        ("crates/a/Cargo.toml", CRATE_A),
        ("crates/b/Cargo.toml", "[package\nname = "),
    ]);
    map.add_facts(
        "crates/a/src/lib.rs",
        Some("use b::Height;\nstruct L;\nimpl From<Height> for L {}\n"),
        &[1, 2, 3],
    );
    let fact = map.facts.first().expect("a fact for b::Height");
    assert_ne!(fact.definition, Definition::NotLocated);
}

#[test]
fn review_absolute_path_dependency_is_not_a_repository_path() {
    let a = "[package]\nname = \"a\"\n\n[dependencies]\nx = { path = \"/opt/x\" }\n";
    let map = map_of(&[("crates/a/Cargo.toml", a)]);
    let x = external(&map, dependency(krate(&map, "a"), "x"));
    assert_ne!(
        x.source,
        CrateSource::Path {
            path: "crates/a/opt/x".to_owned(),
            repo: None
        },
        "an absolute path shown as a path inside the repository"
    );
}

#[test]
fn review_lock_version_of_a_crate_without_version() {
    // Cargo >= 1.75: a package without `version` is 0.0.0 in Cargo.lock.
    let a = "[package]\nname = \"a\"\n\n[dependencies]\nserde = \"1\"\n";
    let lock = "version = 4\n\n[[package]]\nname = \"a\"\nversion = \"0.0.0\"\ndependencies = [\n \"serde 1.0.200\",\n]\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n[[package]]\nname = \"serde\"\nversion = \"0.9.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
    let map = map_of(&[("Cargo.toml", a), ("Cargo.lock", lock)]);
    let serde = external(&map, dependency(krate(&map, "a"), "serde"));
    assert_eq!(serde.version.as_deref(), Some("1.0.200"));
}

#[test]
fn review_import_in_another_module_does_not_name_a_local_type() {
    let text =
        "mod tests {\n    use dpp::Foo;\n}\nstruct Foo;\nstruct L;\nimpl From<Foo> for L {}\n";
    let got = tech::conversion_candidates(text, &[6]);
    assert!(
        got.iter().all(
            |c| !matches!(&c.origin, TypeOrigin::Crate { code_name, .. } if code_name == "dpp")
        ),
        "{got:?}"
    );
}

#[test]
fn review_macro_call_on_the_impl_line_does_not_hide_the_impl() {
    let text = "struct L;\nimpl From<dpp::X> for L { fn from(_: dpp::X) -> Self { todo!() } }\n";
    let got = tech::conversion_candidates(text, &[2]);
    assert!(
        got.iter()
            .any(|c| c.type_name == "X" && c.kind == tech::CandidateKind::FromImpl),
        "{got:?}"
    );
}

#[test]
fn review_deep_toml_does_not_overflow_a_worker_stack() {
    let deep_array = format!(
        "[package]\nname = \"a\"\nx = {}1{}\n",
        "[".repeat(100_000),
        "]".repeat(100_000)
    );
    let deep_inline = format!(
        "[package]\nname = \"a\"\nx = {}1{}\n",
        "{a = ".repeat(50_000),
        "}".repeat(50_000)
    );
    let dotted = format!(
        "[package]\nname = \"a\"\n{} = 1\n",
        vec!["a"; 200_000].join(".")
    );
    let header = format!("[{}]\n", vec!["a"; 200_000].join("."));
    let handle = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            for text in [deep_array, deep_inline, dotted, header] {
                let _ = map_of(&[("Cargo.toml", text.as_str())]);
            }
        })
        .expect("thread");
    assert!(handle.join().is_ok());
}
