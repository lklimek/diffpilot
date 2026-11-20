use std::collections::HashMap;

use analysis::diff::LineSpan;
use contracts::agent::plan::{HunkSelection, PlannedFlow};
use contracts::common::Classification;
use contracts::ids::FlowId;

use super::*;

fn repo(owner: &str, name: &str) -> RepoRef {
    RepoRef {
        owner: owner.to_owned(),
        name: name.to_owned(),
    }
}

const ROOT: &str = "[workspace]\nmembers = [\"crates/*\"]\n";
const APP: &str = r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
dpp = { git = "https://github.com/dashpay/platform", tag = "v2" }
chain = { path = "../chain" }
serde = "1"
foo = { git = "https://github.com/other/foo" }
"#;
const CORE: &str = "[package]\nname = \"chain\"\nversion = \"0.1.0\"\n";
const LOCK: &str = r#"version = 4

[[package]]
name = "dpp"
version = "2.0.0"
source = "git+https://github.com/dashpay/platform?tag=v2#abc123"

[[package]]
name = "foo"
version = "0.1.0"
source = "git+https://github.com/other/foo#def"
"#;
const APP_SOURCE: &str = "use dpp::Identity;\nuse chain::Height;\nuse foo::X;\nstruct L;\nimpl From<Identity> for L {}\nimpl TryFrom<Height> for L {}\nimpl From<X> for L {}\n";

fn map() -> DependencyMap {
    let files: HashMap<&str, &str> = [
        ("Cargo.toml", ROOT),
        ("Cargo.lock", LOCK),
        ("crates/app/Cargo.toml", APP),
        ("crates/chain/Cargo.toml", CORE),
    ]
    .into_iter()
    .collect();
    let paths: Vec<String> = files.keys().map(|p| (*p).to_owned()).collect();
    let mut map = cargo_map::map(&repo("dashpay", "dash-evo-tool"), &paths, &|p| {
        files.get(p).copied()
    });
    map.add_facts("crates/app/src/lib.rs", Some(APP_SOURCE), &[5, 6, 7]);
    map
}

fn hunk(id: &str, start: u32, added: u32) -> Hunk {
    Hunk {
        id: HunkId(id.to_owned()),
        base: LineSpan { start, len: 0 },
        head: LineSpan { start, len: added },
        added: (start..start + added).collect(),
        removed: Vec::new(),
    }
}

fn file(path: &str, hunks: Vec<Hunk>) -> FileDiff {
    FileDiff {
        path: path.to_owned(),
        old_path: None,
        status: FileStatus::Modified,
        binary: false,
        hunks,
    }
}

fn diff() -> Vec<FileDiff> {
    vec![
        file(
            "crates/app/src/lib.rs",
            vec![hunk("h1", 5, 3), hunk("h4", 20, 10)],
        ),
        file("crates/chain/src/x.rs", vec![hunk("h2", 1, 2)]),
        file("docs/readme.md", vec![hunk("h3", 1, 1)]),
    ]
}

fn flow(id: &str, rank: u32, hunks: &[&str]) -> PlannedFlow {
    PlannedFlow {
        id: FlowId(id.to_owned()),
        title: format!("Flow {id}"),
        classification: Classification::Behavioural,
        importance_rank: rank,
        intent: String::new(),
        hunks: hunks
            .iter()
            .map(|h| HunkSelection {
                hunk_id: HunkId((*h).to_owned()),
                head_start: None,
                head_end: None,
            })
            .collect(),
        depends_on: Vec::new(),
    }
}

fn note(text: &str, hunks: &[&str]) -> PlanCrateNote {
    PlanCrateNote {
        note: text.to_owned(),
        hunks: hunks.iter().map(|h| HunkId((*h).to_owned())).collect(),
    }
}

fn plan() -> PlanOutput {
    PlanOutput {
        flows: vec![flow("f2", 2, &["h2"]), flow("f1", 1, &["h1", "h2"])],
        unassigned: Vec::new(),
        crate_notes: Some(vec![
            note("small one", &["h1"]),
            note("big one", &["h4"]),
            note("both", &["h1", "h4"]),
            note("another small", &["h1"]),
            note("unknown hunk", &["h99"]),
            note("chain change", &["h2"]),
        ]),
    }
}

fn view_with(plan: Option<&PlanOutput>, controlled: &[String]) -> DependencyView {
    let map = map();
    let files = diff();
    let pr_repo = repo("dashpay", "dash-evo-tool");
    project(&ProjectionInput {
        map: &map,
        pr_repo: &pr_repo,
        files: &files,
        plan,
        controlled,
    })
}

fn view() -> DependencyView {
    let plan = plan();
    view_with(Some(&plan), &["Acme".to_owned(), "DASHPAY".to_owned()])
}

fn find<'v>(view: &'v DependencyView, name: &str) -> &'v DependencyCrate {
    view.repositories
        .iter()
        .flat_map(|r| &r.crates)
        .find(|c| c.name == name)
        .expect("crate listed")
}

#[test]
fn deps_view_controlled_owners_include_the_pr_owner_once() {
    assert_eq!(view().controlled_owners, ["Acme", "dashpay"]);
}

#[test]
fn deps_view_this_repository_first_with_changed_crates_by_size() {
    let view = view();
    let this = &view.repositories[0];
    assert_eq!(this.kind, RepositoryKind::This);
    assert!(this.controlled);
    let names: Vec<&str> = this.crates.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["app", "chain"]);
    let app = find(&view, "app");
    let changes = app.changes.as_ref().expect("changed");
    assert_eq!((changes.changed_lines, changes.files), (13, 1));
    let flows: Vec<&str> = changes.flows.iter().map(|f| f.id.0.as_str()).collect();
    assert_eq!(flows, ["f1"]);
    let chain = find(&view, "chain");
    let flows: Vec<&str> = chain
        .changes
        .as_ref()
        .expect("changed")
        .flows
        .iter()
        .map(|f| f.id.0.as_str())
        .collect();
    assert_eq!(flows, ["f1", "f2"], "reading order");
    assert_eq!(chain.used_by, ["app"]);
    assert_eq!(view.files_outside_crates, 1);
}

#[test]
fn deps_view_lists_controlled_repositories_first_then_other_sources() {
    let view = view();
    let groups: Vec<(RepositoryKind, bool, Option<&str>)> = view
        .repositories
        .iter()
        .map(|r| (r.kind, r.controlled, r.location.as_deref()))
        .collect();
    assert_eq!(
        groups,
        [
            (RepositoryKind::This, true, None),
            (
                RepositoryKind::Github,
                true,
                Some("https://github.com/dashpay/platform?tag=v2")
            ),
            (
                RepositoryKind::Github,
                false,
                Some("https://github.com/other/foo")
            ),
            (RepositoryKind::Registry, false, Some("crates.io")),
        ]
    );
    let serde = find(&view, "serde");
    assert_eq!(
        (serde.used_by.as_slice(), serde.facts_about),
        (["app".to_owned()].as_slice(), 0)
    );
    let platform = &view.repositories[1];
    assert_eq!(platform.kind, RepositoryKind::Github);
    assert_eq!(platform.repo, Some(repo("dashpay", "platform")));
    let dpp = &platform.crates[0];
    assert_eq!(dpp.name, "dpp");
    assert_eq!(dpp.used_by, ["app"]);
    assert_eq!(dpp.facts_about, 1);
    assert_eq!(dpp.revision.as_deref(), Some("abc123"));
    assert_eq!(
        dpp.source_at,
        Some(HeadLine {
            file: "Cargo.lock".to_owned(),
            line: 3
        })
    );
    let app = find(&view, "app").changes.as_ref().expect("changed");
    assert_eq!(
        app.other_dependencies, 0,
        "every direct dependency is listed"
    );
}

#[test]
fn deps_view_lists_at_most_the_cap_of_other_crates_and_counts_the_rest() {
    let mut manifest =
        String::from("[package]\nname = \"big\"\nversion = \"0.1.0\"\n\n[dependencies]\n");
    for n in 0..MAX_OTHER_CRATES + 5 {
        manifest.push_str(&format!("dep{n:03} = \"1\"\n"));
    }
    let files: HashMap<&str, &str> = [("Cargo.toml", manifest.as_str())].into_iter().collect();
    let paths: Vec<String> = files.keys().map(|p| (*p).to_owned()).collect();
    let map = cargo_map::map(&repo("acme", "big"), &paths, &|p| files.get(p).copied());
    let diff = vec![file("src/lib.rs", vec![hunk("h1", 1, 1)])];
    let pr_repo = repo("acme", "big");
    let view = project(&ProjectionInput {
        map: &map,
        pr_repo: &pr_repo,
        files: &diff,
        plan: None,
        controlled: &[],
    });
    let registry = &view.repositories[1];
    assert_eq!(registry.kind, RepositoryKind::Registry);
    assert_eq!(registry.crates.len(), MAX_OTHER_CRATES);
    let big = find(&view, "big").changes.as_ref().expect("changed");
    assert_eq!(big.other_dependencies, 5);
}

#[test]
fn deps_view_facts_of_listed_crates_and_a_count_of_the_others() {
    let view = view();
    let app = find(&view, "app").changes.as_ref().expect("changed");
    let facts: Vec<(&str, &str)> = app
        .facts
        .iter()
        .map(|f| (f.type_name.as_str(), f.type_crate.as_str()))
        .collect();
    assert_eq!(facts, [("Identity", "dpp"), ("Height", "chain")]);
    assert_eq!(
        app.other_facts, 1,
        "foo::X of an organisation not controlled"
    );
    assert_eq!(app.facts[0].definition, TypeDefinition::OtherRepository);
    assert_eq!(
        app.facts[0].imported_at,
        Some(HeadLine {
            file: "crates/app/src/lib.rs".to_owned(),
            line: 1
        })
    );
    assert_eq!(find(&view, "chain").facts_about, 1);
}

#[test]
fn deps_view_controlling_another_owner_lists_its_repository() {
    let plan = plan();
    let view = view_with(Some(&plan), &["other".to_owned()]);
    let names: Vec<String> = view
        .repositories
        .iter()
        .filter_map(|r| r.repo.as_ref().map(|r| format!("{}/{}", r.owner, r.name)))
        .collect();
    assert_eq!(
        names,
        ["dashpay/dash-evo-tool", "dashpay/platform", "other/foo"]
    );
    assert!(view.repositories[2].controlled);
    let app = find(&view, "app").changes.as_ref().expect("changed");
    assert_eq!(app.other_facts, 0);
}

#[test]
fn deps_view_notes_ranked_by_changed_lines_with_a_count_of_the_rest() {
    let view = view();
    let app = find(&view, "app").changes.as_ref().expect("changed");
    let notes: Vec<(&str, u32)> = app
        .notes
        .iter()
        .map(|n| (n.text.as_str(), n.changed_lines))
        .collect();
    assert_eq!(notes, [("both", 13), ("big one", 10), ("small one", 3)]);
    assert_eq!(app.notes_hidden, 1);
    assert_eq!(
        app.notes[1].lines,
        [FileRange {
            file: "crates/app/src/lib.rs".to_owned(),
            side: LineSide::Head,
            range: LineRange {
                start_line: 20,
                end_line: 29
            }
        }]
    );
    let chain = find(&view, "chain").changes.as_ref().expect("changed");
    let notes: Vec<&str> = chain.notes.iter().map(|n| n.text.as_str()).collect();
    assert_eq!(notes, ["chain change"]);
    assert_eq!(view.notes, NotesState::Produced);
}

#[test]
fn deps_view_notes_state_follows_the_plan() {
    assert_eq!(view_with(None, &[]).notes, NotesState::Pending);
    let mut plan = plan();
    plan.crate_notes = None;
    let view = view_with(Some(&plan), &[]);
    assert_eq!(view.notes, NotesState::NotProduced);
    assert!(
        find(&view, "app")
            .changes
            .as_ref()
            .expect("changed")
            .notes
            .is_empty()
    );
}

#[test]
fn deps_view_of_a_repository_without_cargo_is_not_available() {
    let map = DependencyMap::default();
    let files = diff();
    let pr_repo = repo("acme", "web");
    let view = project(&ProjectionInput {
        map: &map,
        pr_repo: &pr_repo,
        files: &files,
        plan: None,
        controlled: &[],
    });
    assert_eq!(view.status, DependencyStatus::NotRust);
    assert!(view.repositories.is_empty());
    assert_eq!(
        view.files_outside_crates, 0,
        "nothing was checked, so nothing is counted"
    );
}

/// A git repository in a temp dir with `files` committed; returns it and the commit.
fn repository(files: &[(&str, &str)]) -> (tempfile::TempDir, CommitSha) {
    let dir = tempfile::tempdir().expect("temp dir");
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.test")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.test")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .expect("git");
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    git(&["init", "-q"]);
    for (path, text) in files {
        let file = dir.path().join(path);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        std::fs::write(file, text).expect("write");
    }
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c"]);
    let head = CommitSha(git(&["rev-parse", "HEAD"]));
    (dir, head)
}

#[tokio::test]
async fn deps_compute_reads_the_mirror_and_locates_definitions() {
    let (dir, head) = repository(&[
        ("Cargo.toml", ROOT),
        ("Cargo.lock", LOCK),
        ("crates/app/Cargo.toml", APP),
        ("crates/chain/Cargo.toml", CORE),
        ("crates/app/src/lib.rs", APP_SOURCE),
        ("crates/chain/src/lib.rs", "pub mod height;\n"),
        (
            "crates/chain/src/height.rs",
            "/// A height.\npub struct Height(pub u64);\n",
        ),
        ("crates/app/src/height.rs", "pub struct Height;\n"),
    ]);
    let files = vec![file("crates/app/src/lib.rs", vec![hunk("h1", 5, 3)])];
    let map = compute(
        AnalysisId::new_random(),
        &repo("dashpay", "dash-evo-tool"),
        &head,
        dir.path(),
        &files,
    )
    .await
    .expect("computed");
    assert_eq!(map.status, MapStatus::Available);
    let height = map
        .facts
        .iter()
        .find(|f| f.type_name == "Height")
        .expect("fact");
    assert_eq!(
        height.definition,
        Definition::Located {
            at: FileLine {
                file: "crates/chain/src/height.rs".to_owned(),
                line: 2
            }
        },
        "in the defining crate, not the same name in the changing crate"
    );
    let identity = map
        .facts
        .iter()
        .find(|f| f.type_name == "Identity")
        .expect("fact");
    assert_eq!(identity.definition, Definition::OtherRepository);
}

#[tokio::test]
async fn deps_compute_of_a_repository_without_cargo() {
    let (dir, head) = repository(&[("index.js", "x\n")]);
    let files = vec![file("index.js", vec![hunk("h1", 1, 1)])];
    let map = compute(
        AnalysisId::new_random(),
        &repo("acme", "web"),
        &head,
        dir.path(),
        &files,
    )
    .await
    .expect("computed");
    assert_eq!(map, DependencyMap::default());
}

#[tokio::test]
async fn deps_compute_definition_files_over_the_cap_are_not_searched() {
    let comments: Vec<(String, String)> = (0..MAX_DEFINITION_FILES)
        .map(|n| {
            (
                format!("crates/chain/src/a{n:03}.rs"),
                "// struct Height is defined elsewhere\n".to_owned(),
            )
        })
        .collect();
    let mut files: Vec<(&str, &str)> = vec![
        ("Cargo.toml", ROOT),
        ("Cargo.lock", LOCK),
        ("crates/app/Cargo.toml", APP),
        ("crates/chain/Cargo.toml", CORE),
        ("crates/app/src/lib.rs", APP_SOURCE),
        ("crates/chain/src/zz.rs", "pub struct Height;\n"),
    ];
    files.extend(comments.iter().map(|(p, t)| (p.as_str(), t.as_str())));
    let (dir, head) = repository(&files);
    let diff = vec![file("crates/app/src/lib.rs", vec![hunk("h1", 5, 3)])];
    let map = compute(
        AnalysisId::new_random(),
        &repo("dashpay", "dash-evo-tool"),
        &head,
        dir.path(),
        &diff,
    )
    .await
    .expect("computed");
    let height = map
        .facts
        .iter()
        .find(|f| f.type_name == "Height")
        .expect("fact");
    assert_eq!(
        height.definition,
        Definition::NotSearched,
        "the defining file is past the cap: not read, so not 'not located'"
    );
    assert!(
        map.limits.contains(&MapLimit::DefinitionsUnread {
            dir: "crates/chain".to_owned(),
            files: 1
        }),
        "{:?}",
        map.limits
    );
}
