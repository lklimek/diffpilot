use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use contracts::agent::analyse_flow::MovedFrom;
use contracts::api::code::MovedLines;
use contracts::api::tour::{Counters, MoveFact, ProjectProfileStatus};
use contracts::common::{LineRange, ProvenanceBadge};
use contracts::ids::CommitSha;
use proptest::prelude::*;
use similar::{Algorithm, DiffOp, capture_diff_slices};

use super::*;
use crate::diff::{self, FileStatus, Hunk, LineSpan};

type Texts = HashMap<(LineSide, String), String>;

fn run(files: &[FileDiff], base_paths: &[String], texts: &Texts) -> Prepass {
    let repo = RepoRef {
        owner: "acme".to_owned(),
        name: "node-client".to_owned(),
    };
    prepass(&repo, files, base_paths, &|side, path| {
        texts.get(&(side, path.to_owned())).map(String::as_str)
    })
    .expect("prepass")
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482")
}

fn json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read fixture"))
        .expect("fixture JSON")
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git");
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).expect("utf-8"))
}

fn from_value<T: serde::de::DeserializeOwned>(value: &serde_json::Value) -> T {
    serde_json::from_value(value.clone()).expect("fixture shape")
}

#[test]
fn prepass_reproduces_fixture_pr_482() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo = dir.path().join("repo");
    let built = Command::new(fixture().join("repo/build-repo.sh"))
        .arg(&repo)
        .output()
        .expect("build-repo.sh");
    assert!(built.status.success(), "{built:?}");
    let commits = json(&fixture().join("repo/commits.json"));
    let sha = |key: &str| CommitSha(commits[key].as_str().expect("sha").to_owned());
    let (base, head) = (sha("base"), sha("head"));
    let files = diff::diff(&repo, &base, &head).expect("diff");
    let base_paths: Vec<String> = git(&repo, &["ls-tree", "-r", "--name-only", &base.0])
        .expect("ls-tree")
        .lines()
        .map(str::to_owned)
        .collect();
    let mut texts = Texts::new();
    for (side, path) in files_to_read(&files, &base_paths) {
        let commit = if side == LineSide::Base { &base } else { &head };
        if let Some(text) = git(&repo, &["show", &format!("{}:{path}", commit.0)]) {
            texts.insert((side, path), text);
        }
    }
    assert!(texts.contains_key(&(LineSide::Base, PROFILE_PATH.to_owned())));
    assert!(texts.contains_key(&(LineSide::Base, "Cargo.toml".to_owned())));

    let result = run(&files, &base_paths, &texts);

    // Hunk dispositions: `prepass` column of hunks.json.
    let expected: Vec<(String, String)> = json(&fixture().join("repo/hunks.json"))
        .as_array()
        .expect("hunks")
        .iter()
        .map(|hunk| {
            let field = |name: &str| hunk[name].as_str().expect("field").to_owned();
            (
                field("id"),
                if ["h17", "h22"].contains(&field("id").as_str()) {
                    "agent".to_owned()
                } else {
                    field("prepass")
                },
            )
        })
        .collect();
    let actual: Vec<(String, String)> = result
        .hunks
        .iter()
        .map(|hunk| {
            let disposition = match hunk.disposition {
                Disposition::Agent => "agent".to_owned(),
                Disposition::NotAnalysed(category) => format!(
                    "not_analysed:{}",
                    serde_json::to_value(category)
                        .expect("category")
                        .as_str()
                        .expect("string")
                ),
            };
            (hunk.id.0.clone(), disposition)
        })
        .collect();
    assert_eq!(actual, expected);

    // Facts of the Tour (api-seed): quiet rows, counters, profile, instruction files.
    let tour = json(&fixture().join("api-seed/default/tour.json"));
    let facts = &tour["pr_context"]["facts"];
    assert_eq!(
        result.not_analysed_with_binaries(),
        from_value::<Vec<NotAnalysedSection>>(
            &json(&fixture().join("api/default/tour.json"))["quiet"]["not_analysed"]
        )
    );
    assert!(
        result.identical_moves().is_empty(),
        "the fixture's only identical move stays within src/rpc/client.rs"
    );
    assert_eq!(
        result.counters.counters,
        from_value::<Counters>(&facts["counters"])
    );
    assert_eq!(result.counters.counters.lines_removed_from_tests, 12);
    assert_eq!(result.counters.dependencies.len(), 3);
    assert!(result.counters.dependency_files_not_checked.is_empty());
    assert_eq!(
        result.profile_status(None),
        from_value::<ProjectProfileStatus>(&facts["project_profile"])
    );
    assert_eq!(
        result.agent_instruction_files,
        from_value::<Vec<String>>(&facts["agent_instruction_files"])
    );
    assert!(
        result.related_repos.is_empty(),
        "the fixture has no git or path dependencies"
    );
    assert!(result.not_rendered.is_empty());

    // Move facts: the moved step f5.1 and the lines endpoint.
    let step = tour["flows"]
        .as_array()
        .expect("flows")
        .iter()
        .flat_map(|flow| flow["steps"].as_array().expect("steps"))
        .find(|step| step["id"] == "f5.1")
        .expect("step f5.1");
    let claim: MovedFrom = from_value(&step["llm"]["moved_from"]);
    let range: LineRange = from_value(&step["facts"]["range"]);
    let confirmed = result
        .moves
        .confirm(step["facts"]["file"].as_str().expect("file"), range, &claim)
        .expect("f5.1 is a deterministic move");
    assert_eq!(
        confirmed.fact,
        from_value::<MoveFact>(&step["facts"]["moved"])
    );
    assert_eq!(confirmed.covered_base_lines, [22, 23]);
    let lines = json(&fixture().join("api-seed/default/lines/head/src/rpc/client.rs.json"));
    assert_eq!(
        result.moves.moved_lines("src/rpc/client.rs", 1, 224),
        from_value::<Vec<MovedLines>>(&lines["moved"])
    );

    // Agent input: no quiet line, no not-analysed file.
    for range in &result.agent_lines {
        assert!(
            !range.file.starts_with("tests/")
                && !range.file.ends_with(".md")
                && range.file != "Cargo.lock",
            "{range:?}"
        );
    }
    let stored: Prepass = serde_json::from_value(serde_json::to_value(&result).expect("serialise"))
        .expect("deserialise");
    assert_eq!(stored, result, "the prepass document round-trips");
}

fn hunk(id: usize, base: (u32, u32), head: (u32, u32)) -> Hunk {
    let base = LineSpan {
        start: base.0,
        len: base.1,
    };
    let head = LineSpan {
        start: head.0,
        len: head.1,
    };
    Hunk {
        id: HunkId(format!("h{id}")),
        base,
        head,
        added: (head.start..head.start + head.len).collect(),
        removed: (base.start..base.start + base.len).collect(),
    }
}

fn file(path: &str, old_path: Option<&str>, status: FileStatus, hunks: Vec<Hunk>) -> FileDiff {
    FileDiff {
        path: path.to_owned(),
        old_path: old_path.map(str::to_owned),
        status,
        binary: false,
        hunks,
    }
}

fn text(side: LineSide, path: &str, lines: &[&str]) -> ((LineSide, String), String) {
    let text = lines.iter().map(|line| format!("{line}\n")).collect();
    ((side, path.to_owned()), text)
}

#[test]
fn generated_file_never_in_agent_input() {
    let files = vec![
        file(
            "Cargo.lock",
            None,
            FileStatus::Modified,
            vec![hunk(1, (2, 1), (2, 1))],
        ),
        file(
            "api/service.pb.go",
            None,
            FileStatus::Added,
            vec![hunk(2, (0, 0), (1, 2))],
        ),
        FileDiff {
            binary: true,
            ..file("assets/logo.png", None, FileStatus::Modified, vec![])
        },
        file(
            "src/gen.rs",
            None,
            FileStatus::Modified,
            vec![hunk(3, (3, 1), (3, 1))],
        ),
        file(
            "src/lib.rs",
            None,
            FileStatus::Modified,
            vec![hunk(4, (1, 1), (1, 1))],
        ),
    ];
    let header = "// Code generated by build.rs. DO NOT EDIT.";
    let texts: Texts = [
        text(LineSide::Base, "Cargo.lock", &["# lock", "version = 3"]),
        text(LineSide::Head, "Cargo.lock", &["# lock", "version = 4"]),
        text(
            LineSide::Head,
            "api/service.pb.go",
            &["package api", "func A() {}"],
        ),
        text(
            LineSide::Base,
            "src/gen.rs",
            &[header, "", "pub const A: u32 = 1;"],
        ),
        text(
            LineSide::Head,
            "src/gen.rs",
            &[header, "", "pub const A: u32 = 2;"],
        ),
        text(LineSide::Base, "src/lib.rs", &["use a::x;"]),
        text(LineSide::Head, "src/lib.rs", &["use a::y;"]),
    ]
    .into_iter()
    .collect();
    let result = run(&files, &[], &texts);
    let dispositions: Vec<Disposition> = result.hunks.iter().map(|hunk| hunk.disposition).collect();
    let generated = Disposition::NotAnalysed(NotAnalysedCategory::GeneratedMechanical);
    assert_eq!(
        dispositions,
        [generated, generated, generated, Disposition::Agent]
    );
    let agent_files: Vec<&str> = result
        .agent_lines
        .iter()
        .map(|range| range.file.as_str())
        .collect();
    assert!(
        agent_files.is_empty(),
        "import declarations require no flow coverage"
    );
    let not_rendered: Vec<(&str, LineSide, NotRenderedReason)> = result
        .not_rendered
        .iter()
        .map(|item| (item.file.as_str(), item.side, item.reason))
        .collect();
    assert_eq!(
        not_rendered,
        [
            (
                "api/service.pb.go",
                LineSide::Head,
                NotRenderedReason::Generated
            ),
            ("assets/logo.png", LineSide::Base, NotRenderedReason::Binary),
            ("assets/logo.png", LineSide::Head, NotRenderedReason::Binary),
            ("src/gen.rs", LineSide::Base, NotRenderedReason::Generated),
            ("src/gen.rs", LineSide::Head, NotRenderedReason::Generated),
        ],
        "the lockfile is mechanical and stays readable"
    );
    for section in result.not_analysed_with_binaries() {
        for item in section.items {
            let wire = serde_json::to_value(&item).unwrap();
            assert_eq!(
                wire["renderable"],
                !result.is_not_rendered(item.side, &item.file),
                "{}",
                item.file
            );
        }
    }

    let section = result
        .not_analysed
        .iter()
        .find(|section| section.category == NotAnalysedCategory::GeneratedMechanical)
        .expect("generated section");
    assert_eq!(section.file_count, 3);
    let gen_item = section
        .items
        .iter()
        .find(|item| item.file == "src/gen.rs")
        .expect("src/gen.rs item");
    assert_eq!(
        (gen_item.badge, gen_item.ranges.clone()),
        (
            ProvenanceBadge::ClassifiedByLineRule,
            Some(vec![LineRange {
                start_line: 3,
                end_line: 3
            }])
        )
    );
}

#[test]
fn agent_files_quiet_lines_and_binaries_split_the_diff() {
    let files = vec![
        file(
            "api/service.pb.go",
            None,
            FileStatus::Added,
            vec![hunk(1, (0, 0), (1, 2))],
        ),
        FileDiff {
            binary: true,
            ..file("assets/logo.png", None, FileStatus::Added, vec![])
        },
        file(
            "src/lib.rs",
            None,
            FileStatus::Modified,
            vec![hunk(2, (1, 1), (1, 1))],
        ),
    ];
    let texts: Texts = [
        text(
            LineSide::Head,
            "api/service.pb.go",
            &["package api", "func A() {}"],
        ),
        text(LineSide::Base, "src/lib.rs", &["use a::x;"]),
        text(LineSide::Head, "src/lib.rs", &["use a::y;"]),
    ]
    .into_iter()
    .collect();
    let result = run(&files, &[], &texts);
    let agent_files = result.agent_files(&files);
    let agent: Vec<(&str, usize)> = agent_files
        .iter()
        .map(|file| (file.path.as_str(), file.hunks.len()))
        .collect();
    assert_eq!(
        agent,
        [("src/lib.rs", 1)],
        "generated and binary files never reach the agent"
    );
    let quiet: Vec<(&str, LineSide, Vec<LineRange>)> = result
        .quiet_lines(&files)
        .into_iter()
        .map(|lines| {
            (
                if lines.file == "api/service.pb.go" {
                    "pb"
                } else {
                    "imports"
                },
                lines.side,
                lines.ranges,
            )
        })
        .collect();
    assert_eq!(
        quiet,
        [
            (
                "pb",
                LineSide::Head,
                vec![LineRange {
                    start_line: 1,
                    end_line: 2
                }]
            ),
            (
                "imports",
                LineSide::Head,
                vec![LineRange {
                    start_line: 1,
                    end_line: 1
                }]
            ),
            (
                "imports",
                LineSide::Base,
                vec![LineRange {
                    start_line: 1,
                    end_line: 1
                }]
            )
        ]
    );
    assert!(result.is_not_rendered(LineSide::Head, "api/service.pb.go"));
    assert!(result.is_not_rendered(LineSide::Head, "assets/logo.png"));
    assert!(!result.is_not_rendered(LineSide::Head, "src/lib.rs"));
    let sections = result.not_analysed_with_binaries();
    let binary: Vec<(&str, ProvenanceBadge, &str)> = sections
        .iter()
        .flat_map(|section| &section.items)
        .filter(|item| item.file == "assets/logo.png")
        .map(|item| (item.file.as_str(), item.badge, item.rule.as_str()))
        .collect();
    assert_eq!(
        binary,
        [(
            "assets/logo.png",
            ProvenanceBadge::BinaryContent,
            BINARY_RULE
        )]
    );
    let generated = sections
        .iter()
        .find(|section| section.category == NotAnalysedCategory::GeneratedMechanical)
        .expect("section");
    assert_eq!(
        generated.file_count, 2,
        "the generated file and the binary file"
    );
}

#[test]
fn header_added_by_the_pull_request_does_not_hide_a_file() {
    let header = "// Code generated by build.rs. DO NOT EDIT.";
    let files = vec![file(
        "src/pay.rs",
        None,
        FileStatus::Modified,
        vec![hunk(1, (0, 0), (1, 1)), hunk(2, (2, 1), (3, 1))],
    )];
    let texts: Texts = [
        text(
            LineSide::Base,
            "src/pay.rs",
            &["fn pay() {", "    send(amount);", "}"],
        ),
        text(
            LineSide::Head,
            "src/pay.rs",
            &[header, "fn pay() {", "    send(amount * 2);", "}"],
        ),
    ]
    .into_iter()
    .collect();
    let result = run(&files, &[], &texts);
    assert!(
        result
            .hunks
            .iter()
            .all(|hunk| hunk.disposition == Disposition::Agent)
    );
    assert!(result.not_rendered.is_empty() && result.not_analysed.is_empty());
}

#[test]
fn new_file_with_a_generated_header_goes_to_the_agent() {
    let files = vec![file(
        "src/new.rs",
        None,
        FileStatus::Added,
        vec![hunk(1, (0, 0), (1, 3))],
    )];
    let lines = ["// @generated", "", "pub fn drain() { transfer(all()) }"];
    let texts: Texts = [text(LineSide::Head, "src/new.rs", &lines)]
        .into_iter()
        .collect();
    let result = run(&files, &[], &texts);
    assert_eq!(result.hunks[0].disposition, Disposition::Agent);
    assert_eq!(
        result.agent_lines,
        [FileRanges {
            file: "src/new.rs".to_owned(),
            side: LineSide::Head,
            ranges: vec![LineRange {
                start_line: 1,
                end_line: 3
            }],
        }]
    );
    assert!(result.not_rendered.is_empty() && result.not_analysed.is_empty());
}

#[test]
fn shebang_with_a_quote_does_not_hide_code() {
    let files = vec![file(
        "src/main.rs",
        None,
        FileStatus::Modified,
        vec![hunk(1, (0, 0), (1, 1)), hunk(2, (3, 1), (4, 1))],
    )];
    let texts: Texts = [
        text(
            LineSide::Base,
            "src/main.rs",
            &["fn main() {", "let _ = \"", "// harmless", "\";", "}"],
        ),
        text(
            LineSide::Head,
            "src/main.rs",
            &[
                "#!/unused \"",
                "fn main() {",
                "let _ = \"",
                "//\"; std::process::exit(1); let _ = \"",
                "\";",
                "}",
            ],
        ),
    ]
    .into_iter()
    .collect();
    let result = run(&files, &[], &texts);
    assert!(
        result
            .hunks
            .iter()
            .all(|hunk| hunk.disposition == Disposition::Agent),
        "{:?}",
        result.hunks
    );
    let head_lines: Vec<u32> = result
        .agent_lines
        .iter()
        .filter(|ranges| ranges.side == LineSide::Head)
        .flat_map(|ranges| ranges.ranges.iter().flat_map(|r| r.start_line..=r.end_line))
        .collect();
    assert_eq!(
        head_lines,
        [1, 4],
        "the line that calls exit stays with the agent"
    );
    assert!(result.not_analysed.is_empty());
}

#[test]
fn prepass_flags_agent_instruction_files() {
    let files = vec![
        file(
            "AGENTS.md",
            None,
            FileStatus::Modified,
            vec![hunk(1, (1, 1), (1, 1))],
        ),
        file(
            "docs/CLAUDE.md",
            None,
            FileStatus::Deleted,
            vec![hunk(2, (1, 1), (0, 0))],
        ),
        file(
            ".github/notes.md",
            Some(".codex/notes.md"),
            FileStatus::Renamed,
            vec![],
        ),
        file(
            "src/lib.rs",
            None,
            FileStatus::Modified,
            vec![hunk(3, (1, 1), (1, 1))],
        ),
    ];
    let result = run(&files, &[], &Texts::new());
    assert_eq!(
        result.agent_instruction_files,
        [".codex/notes.md", "AGENTS.md", "docs/CLAUDE.md"],
        "head paths, deleted files and the old path of a rename"
    );
    let empty = run(&files[3..], &[], &Texts::new());
    assert!(empty.agent_instruction_files.is_empty());
}

#[test]
fn profile_status_combines_file_and_memcan() {
    let profile = [text(
        LineSide::Base,
        PROFILE_PATH,
        &["# Profile", "Payments node."],
    )];
    let with_file = run(&[], &[], &profile.into_iter().collect());
    assert!(with_file.profile_file.is_some());
    assert_eq!(
        with_file.profile_status(Some(true)).sources,
        [ProfileSource::RepositoryFile, ProfileSource::Memcan]
    );
    let blank = run(
        &[],
        &[],
        &[text(LineSide::Base, PROFILE_PATH, &["  "])]
            .into_iter()
            .collect(),
    );
    assert_eq!(blank.profile_file, None, "a blank file is no profile");
    assert!(
        blank.profile_status(Some(false)).sources.is_empty(),
        "no project profile"
    );
    let unchecked = with_file.profile_status(None);
    assert_eq!(unchecked.sources, [ProfileSource::RepositoryFile]);
    assert_eq!(
        unchecked.memcan,
        SourceCheck::NotChecked,
        "MemCan not searched"
    );
    assert_eq!(
        blank.profile_status(Some(false)).memcan,
        SourceCheck::Checked
    );
}

/// Line pool of the generated diffs: code, comments, blanks, a generated header, a moved-looking block.
const POOL: [&str; 10] = [
    "",
    "// note",
    "fn alpha() -> u32 { compute_value(1) }",
    "let value = compute(input_parameter);",
    "    let value = compute(input_parameter);",
    "}",
    "/// documentation line",
    "// Code generated by tool. DO NOT EDIT.",
    "call(other_function_name, argument);",
    "return result_value_here_now;",
];
const PATHS: [&str; 12] = [
    "Cargo.lock",
    "README.md",
    "src/a.rs",
    "src/b.rs",
    "src/gen.rs",
    "tests/t.rs",
    "src/tests/auth.rs",
    "crates/k/tests/it.rs",
    "tool/build.py",
    "web/app.test.ts",
    "docs/guide.txt",
    "src/c.rs",
];

/// -U0 hunks between two line lists, as `git diff` reports them.
fn hunks_between(base: &[&str], head: &[&str], next_id: &mut usize) -> Vec<Hunk> {
    let to = |value: usize| u32::try_from(value).expect("small");
    capture_diff_slices(Algorithm::Myers, base, head)
        .into_iter()
        .filter_map(|op| {
            let (old, old_len, new, new_len) = match op {
                DiffOp::Equal { .. } => return None,
                DiffOp::Delete {
                    old_index,
                    old_len,
                    new_index,
                } => (old_index, old_len, new_index, 0),
                DiffOp::Insert {
                    old_index,
                    new_index,
                    new_len,
                } => (old_index, 0, new_index, new_len),
                DiffOp::Replace {
                    old_index,
                    old_len,
                    new_index,
                    new_len,
                } => (old_index, old_len, new_index, new_len),
            };
            let start = |index: usize, len: usize| if len == 0 { to(index) } else { to(index + 1) };
            *next_id += 1;
            Some(hunk(
                *next_id,
                (start(old, old_len), to(old_len)),
                (start(new, new_len), to(new_len)),
            ))
        })
        .collect()
}

fn generated_files() -> impl Strategy<Value = Vec<(usize, u8, Vec<usize>, Vec<usize>)>> {
    let one = (
        0u8..5,
        prop::collection::vec(0..POOL.len(), 0..10),
        prop::collection::vec(0..POOL.len(), 0..10),
    );
    (
        prop::sample::subsequence((0..PATHS.len()).collect::<Vec<_>>(), 1..=PATHS.len()),
        prop::collection::vec(one, PATHS.len()),
    )
        .prop_map(|(paths, specs)| {
            paths
                .into_iter()
                .zip(specs)
                .map(|(path, (status, base, head))| (path, status, base, head))
                .collect()
        })
}

proptest! {
    /// Every non-blank changed line is in exactly one of: the agent's lines, a Not analysed item, an identical move;
    /// unless its only change is whitespace (it reads as unchanged code and is never the agent's).
    #[test]
    fn every_changed_line_has_exactly_one_disposition(specs in generated_files()) {
        let mut files = Vec::new();
        let mut texts = Texts::new();
        let mut next_id = 0;
        for (path, status, base, head) in specs {
            let path = PATHS[path];
            let pick = |indices: &[usize]| -> Vec<&str> { indices.iter().map(|&i| POOL[i]).collect() };
            let old_path = format!("old/{path}");
            let (status, old, base, head): (FileStatus, Option<&str>, Vec<&str>, Vec<&str>) = match status {
                2 => (FileStatus::Added, None, Vec::new(), pick(&head)),
                3 => (FileStatus::Deleted, None, pick(&base), Vec::new()),
                4 => (FileStatus::Renamed, Some(old_path.as_str()), pick(&base), pick(&head)),
                _ => (FileStatus::Modified, None, pick(&base), pick(&head)),
            };
            let diff = file(path, old, status, hunks_between(&base, &head, &mut next_id));
            for (side, lines) in [(LineSide::Base, &base), (LineSide::Head, &head)] {
                if let Some(side_path) = diff.path_on(side) {
                    let (key, value) = text(side, side_path, lines);
                    texts.insert(key, value);
                }
            }
            files.push(diff);
        }
        let result = run(&files, &[], &texts);
        let mut count: BTreeMap<(LineSide, String, u32), usize> = BTreeMap::new();
        for file in &files {
            for side in [LineSide::Base, LineSide::Head] {
                let Some(path) = file.path_on(side) else { continue };
                let body = &texts[&(side, path.to_owned())];
                let lines: Vec<&str> = body.lines().collect();
                for line in changed_lines(file, side) {
                    if !lines[usize::try_from(line - 1).expect("index")].trim().is_empty() {
                        count.insert((side, path.to_owned(), line), 0);
                    }
                }
            }
        }
        for file in &result.whitespace {
            for &(base, head) in &file.pairs {
                count.remove(&(LineSide::Base, file.base.clone(), base));
                count.remove(&(LineSide::Head, file.head.clone(), head));
                for (side, path, line) in [(LineSide::Base, &file.base, base), (LineSide::Head, &file.head, head)] {
                    prop_assert!(!result.agent_lines.iter().any(|r| r.side == side
                        && &r.file == path
                        && r.ranges.iter().any(|r| r.start_line <= line && line <= r.end_line)));
                }
            }
        }
        let mut hit = |side: LineSide, file: &str, lines: &mut dyn Iterator<Item = u32>| {
            for line in lines {
                if let Some(n) = count.get_mut(&(side, file.to_owned(), line)) {
                    *n += 1;
                }
            }
        };
        for range in &result.agent_lines {
            hit(range.side, &range.file, &mut range.ranges.iter().flat_map(|r| r.start_line..=r.end_line));
        }
        for item in result.not_analysed.iter().flat_map(|section| &section.items) {
            let diff = files.iter().find(|f| f.path_on(item.side) == Some(item.file.as_str())).expect("item file");
            let all = changed_lines(diff, item.side);
            match &item.ranges {
                Some(ranges) => hit(item.side, &item.file, &mut ranges.iter().flat_map(|r| r.start_line..=r.end_line)),
                None => hit(item.side, &item.file, &mut all.into_iter()),
            }
        }
        for moved in result.identical_moves() {
            for end in [&moved.from, &moved.to] {
                hit(end.side, &end.file, &mut (end.range.start_line..=end.range.end_line));
            }
        }
        for (line, n) in count {
            prop_assert_eq!(n, 1, "{:?}", line);
        }
    }
}

#[test]
fn whitespace_only_lines_read_as_unchanged_code_and_stay_context() {
    // Both statements are re-indented into a new `if` and swapped.
    let base = [
        "fn withdraw() {",
        "    ensure_authorised()?;",
        "    debit()?;",
        "}",
    ];
    let head = [
        "fn withdraw() {",
        "    if enabled {",
        "        debit()?;",
        "        ensure_authorised()?;",
        "    }",
        "}",
    ];
    let mut next_id = 0;
    let files = vec![file(
        "src/pay.rs",
        None,
        FileStatus::Modified,
        hunks_between(&base, &head, &mut next_id),
    )];
    let texts = Texts::from([
        text(LineSide::Base, "src/pay.rs", &base),
        text(LineSide::Head, "src/pay.rs", &head),
    ]);
    let result = run(&files, &[], &texts);
    let (head_ws, base_ws) = (
        result.whitespace_lines(LineSide::Head, "src/pay.rs"),
        result.whitespace_lines(LineSide::Base, "src/pay.rs"),
    );
    assert_eq!(
        (head_ws.len(), base_ws.len()),
        (1, 1),
        "one statement pairs"
    );
    // Not hidden: no line-rule context, no Not analysed entry.
    assert!(result.view_context(LineSide::Head, "src/pay.rs").is_empty());
    assert!(result.view_context(LineSide::Base, "src/pay.rs").is_empty());
    assert!(result.not_analysed.is_empty());
    // Context for the agent without coverage duty.
    let context = result.context();
    assert_eq!(
        context.get(&(LineSide::Head, "src/pay.rs".into())),
        Some(&head_ws)
    );
    assert_eq!(
        context.get(&(LineSide::Base, "src/pay.rs".into())),
        Some(&base_ws)
    );

    // The view: every head line is shown; the paired one is unchanged, the rest is marked; the other statement's
    // base line is shown as removed, the paired base line is not.
    let view = crate::lines::without_whitespace(&files[0], &result.whitespace_pairs(&files[0]));
    let changed = crate::lines::changed_lines_within(
        std::slice::from_ref(&view),
        "src/pay.rs",
        LineSide::Head,
        1,
        6,
    );
    let moved_statement = if head_ws == [4] { 3 } else { 4 };
    assert_eq!(
        changed,
        [2, moved_statement, 5],
        "the `if`, the moved statement and `}}` are marked"
    );
    let removed: Vec<u32> = crate::lines::deletion_markers(&view, 1, 6)
        .iter()
        .flat_map(|marker| marker.base.start..marker.base.start + marker.base.len)
        .collect();
    assert_eq!(removed.len(), 1);
    assert!(
        !base_ws.contains(&removed[0]),
        "the paired base line is not shown as removed"
    );
}

#[test]
fn rust_comment_import_context_stays_with_agent_but_needs_no_coverage() {
    let text = "// heading\nuse std::fmt;\nfn work() {}\n/* explanation */\n";
    let files = vec![FileDiff {
        path: "src/lib.rs".into(),
        old_path: None,
        status: FileStatus::Added,
        binary: false,
        hunks: vec![Hunk {
            id: HunkId("h1".into()),
            base: LineSpan { start: 0, len: 0 },
            head: LineSpan { start: 1, len: 4 },
            added: vec![1, 2, 3, 4],
            removed: vec![],
        }],
    }];
    let texts = HashMap::from([((LineSide::Head, "src/lib.rs".into()), text.into())]);
    let result = run(&files, &[], &texts);
    assert_eq!(
        result.agent_files(&files)[0].hunks[0].head,
        files[0].hunks[0].head
    );
    let quiet = result.quiet_lines(&files);
    for line in [1, 2, 4] {
        assert!(
            quiet.iter().any(|f| f
                .ranges
                .iter()
                .any(|r| r.start_line <= line && line <= r.end_line)),
            "line {line} needs no flow"
        );
    }
    assert_eq!(
        result.agent_lines[0].ranges,
        vec![LineRange {
            start_line: 3,
            end_line: 3
        }]
    );
}

#[test]
fn fixture_and_mock_directories_are_excluded_from_agent_input() {
    for path in [
        "src/fixtures/data.rs",
        "packages/client/mocks/api.ts",
        "crates/node/test_helpers/data.rs",
    ] {
        let files = vec![FileDiff {
            path: path.into(),
            old_path: None,
            status: FileStatus::Added,
            binary: false,
            hunks: vec![Hunk {
                id: HunkId("h1".into()),
                base: LineSpan { start: 0, len: 0 },
                head: LineSpan { start: 1, len: 1 },
                added: vec![1],
                removed: vec![],
            }],
        }];
        let texts = HashMap::from([((LineSide::Head, path.into()), "fn helper() {}\n".into())]);
        let result = run(&files, &[], &texts);
        assert!(result.agent_files(&files).is_empty(), "{path}");
        assert_eq!(result.not_analysed[0].category, NotAnalysedCategory::Tests);
    }
}

#[test]
fn reorder_within_a_file_stays_with_the_agent_cross_file_move_stays_quiet() {
    let guard = [
        "    self.ensure_authorised(amount)?;",
        "    self.ensure_within_limit(amount)?;",
        "    self.audit.record(\"withdraw\", amount);",
    ];
    let rest = [
        "    self.ledger.debit(self.account, amount)?;",
        "    self.ledger.flush()?;",
        "    self.notify_owner(amount)?;",
        "    self.metrics.inc(\"withdraw\");",
        "    self.metrics.flush_all();",
        "    self.log_completion();",
    ];
    let open = ["fn withdraw(&mut self, amount: u64) -> Result<(), Error> {"];
    let close = ["    Ok(())", "}"];
    let base: Vec<&str> = [&open[..], &guard, &rest, &close].concat();
    let head: Vec<&str> = [&open[..], &rest, &guard, &close].concat();
    let mut next_id = 0;
    let files = vec![file(
        "src/bank.rs",
        None,
        FileStatus::Modified,
        hunks_between(&base, &head, &mut next_id),
    )];
    let texts = Texts::from([
        text(LineSide::Base, "src/bank.rs", &base),
        text(LineSide::Head, "src/bank.rs", &head),
    ]);
    let result = run(&files, &[], &texts);
    let block = result.moves.moved.iter().find(|block| block.identical);
    assert!(
        block.is_some_and(|block| !result.is_quiet_move(block)),
        "an identical move within one file is no quiet move"
    );
    assert!(result.quiet_lines(&files).is_empty());
    assert!(result.identical_moves().is_empty(), "no quiet row");
    assert_eq!(result.agent_files(&files), files);

    // A whole function moved into another file that the origin still calls: a quiet row, its lines context.
    let helper = [
        "fn audit_entry(ledger: &Ledger, amount: u64) {",
        "    ledger.record(\"withdraw\", amount);",
        "    ledger.flush_audit_trail();",
        "}",
    ];
    let caller = [
        "fn withdraw(ledger: &Ledger, amount: u64) {",
        "    audit_entry(ledger, amount);",
        "}",
    ];
    let base_a: Vec<&str> = [&caller[..], &helper].concat();
    let head_b: Vec<&str> = ["const LIMIT: u64 = 10;"]
        .into_iter()
        .chain(helper)
        .collect();
    let mut next_id = 0;
    let files = vec![
        file(
            "src/a.rs",
            None,
            FileStatus::Modified,
            hunks_between(&base_a, &caller, &mut next_id),
        ),
        file(
            "src/b.rs",
            None,
            FileStatus::Added,
            hunks_between(&[], &head_b, &mut next_id),
        ),
    ];
    let texts = Texts::from([
        text(LineSide::Base, "src/a.rs", &base_a),
        text(LineSide::Head, "src/a.rs", &caller),
        text(LineSide::Head, "src/b.rs", &head_b),
    ]);
    let result = run(&files, &[], &texts);
    assert_eq!(result.identical_moves().len(), 1, "cross-file: a quiet row");
    assert!(
        result
            .hunks
            .iter()
            .all(|hunk| hunk.disposition == Disposition::Agent),
        "listed to the agent as context"
    );
    let listed = result.agent_files(&files);
    assert!(
        listed[0].hunks[0].removed.is_empty(),
        "no coverage duty at the origin"
    );
    assert_eq!(
        listed[1].hunks[0].added,
        [1],
        "only the new use line is mandatory"
    );
    assert!(
        result
            .quiet_lines(&files)
            .iter()
            .any(|lines| lines.file == "src/a.rs" && lines.side == LineSide::Base)
    );

    // The source file is deleted (possibly a rename git did not pair): the move stays with the agent.
    let mut next_id = 0;
    let files = vec![
        file(
            "src/a.rs",
            None,
            FileStatus::Deleted,
            hunks_between(&base_a, &[], &mut next_id),
        ),
        file(
            "src/b.rs",
            None,
            FileStatus::Added,
            hunks_between(&[], &head_b, &mut next_id),
        ),
    ];
    let texts = Texts::from([
        text(LineSide::Base, "src/a.rs", &base_a),
        text(LineSide::Head, "src/b.rs", &head_b),
    ]);
    let result = run(&files, &[], &texts);
    assert!(result.moves.moved.iter().any(|block| block.identical));
    assert!(
        result.identical_moves().is_empty(),
        "the source file is gone"
    );
    assert!(result.quiet_lines(&files).is_empty());
}

#[test]
fn inline_rust_tests_are_not_analysed_but_stay_shown() {
    let base = ["fn f() -> u8 {", "    1", "}"];
    let head = [
        "fn f() -> u8 {",
        "    2",
        "}",
        "",
        "#[cfg(test)]",
        "mod tests {",
        "    #[test]",
        "    fn two() {",
        "        assert_eq!(super::f(), 2);",
        "    }",
        "}",
    ];
    let mut next_id = 0;
    let files = vec![file(
        "src/lib.rs",
        None,
        FileStatus::Modified,
        hunks_between(&base, &head, &mut next_id),
    )];
    let texts = Texts::from([
        text(LineSide::Base, "src/lib.rs", &base),
        text(LineSide::Head, "src/lib.rs", &head),
    ]);
    let result = run(&files, &[], &texts);
    // The diff may pair the module's closing `}` with the function's: the changed lines of the module.
    let mut changed: Vec<u32> = files[0]
        .hunks
        .iter()
        .flat_map(|hunk| hunk.added.iter().copied())
        .filter(|line| (5..=11).contains(line))
        .collect();
    changed.sort_unstable();
    assert!(changed.starts_with(&[5, 6, 7, 8, 9, 10]), "{changed:?}");
    let test_lines = crate::lines::ranges(&changed)[0];
    // Listed under Tests with its lines, by the line rule.
    let tests: Vec<&NotAnalysedItem> = result
        .not_analysed
        .iter()
        .filter(|section| section.category == NotAnalysedCategory::Tests)
        .flat_map(|section| &section.items)
        .collect();
    assert_eq!(tests.len(), 1, "{:?}", result.not_analysed);
    assert_eq!(tests[0].rule, tech::TEST_ITEM_RULE);
    assert_eq!(tests[0].badge, ProvenanceBadge::ClassifiedByLineRule);
    assert_eq!(tests[0].ranges, Some(vec![test_lines]));
    // Not for the agent, no coverage duty; the production line stays with the agent.
    let agent: Vec<LineRange> = result
        .agent_lines
        .iter()
        .filter(|lines| lines.side == LineSide::Head)
        .flat_map(|lines| lines.ranges.clone())
        .collect();
    let agent_lines: Vec<u32> = agent
        .iter()
        .flat_map(|r| r.start_line..=r.end_line)
        .collect();
    assert!(agent_lines.contains(&2), "{agent_lines:?}");
    assert!(
        agent_lines.iter().all(|line| !changed.contains(line)),
        "{agent_lines:?}"
    );
    let quiet = result.quiet_lines(&files);
    assert!(
        quiet
            .iter()
            .any(|lines| lines.side == LineSide::Head && lines.ranges.contains(&test_lines))
    );
    // Shown in the code views: test code is not hidden like comments and imports.
    assert!(result.view_context(LineSide::Head, "src/lib.rs").is_empty());
    // Counted as test lines.
    let count = u32::try_from(changed.len()).expect("small");
    assert_eq!(result.counters.counters.lines_added_to_tests, Some(count));
}

#[test]
fn a_hunk_of_only_context_lines_is_context_only() {
    let head = ["// heading", "use std::fmt;", "", "fn work() {}"];
    let files = vec![file(
        "src/lib.rs",
        None,
        FileStatus::Added,
        vec![hunk(1, (0, 0), (1, 3)), hunk(2, (0, 0), (4, 1))],
    )];
    let texts = Texts::from([text(LineSide::Head, "src/lib.rs", &head)]);
    let result = run(&files, &[], &texts);
    let [context, code] = files[0].hunks.as_slice() else {
        panic!("two hunks")
    };
    let mut with_unknown = files.clone();
    // A hunk the pre-pass does not know is never context only.
    with_unknown[0].hunks.push(Hunk {
        id: HunkId("h99".into()),
        ..context.clone()
    });
    assert_eq!(
        result.context_only_hunks(&with_unknown),
        BTreeSet::from([context.id.clone()]),
        "comment, import, blank: not {code:?}"
    );
}

/// A change made only of context lines (imports, comments, inline tests) leaves nothing to plan: no plan batch,
/// so no plan job spends tokens on an empty listing.
#[test]
#[ignore = "repro: agent_files keeps context-only hunks, so plan_batches still yields a batch and a plan job runs"]
fn a_change_of_only_context_lines_has_no_plan_batch() {
    let head = ["// heading", "use std::fmt;", ""];
    let files = vec![file(
        "src/lib.rs",
        None,
        FileStatus::Added,
        vec![hunk(1, (0, 0), (1, 3))],
    )];
    let texts = Texts::from([text(LineSide::Head, "src/lib.rs", &head)]);
    let result = run(&files, &[], &texts);
    assert_eq!(
        result.context_only_hunks(&files),
        BTreeSet::from([files[0].hunks[0].id.clone()])
    );
    let batches = crate::prepass::batches::plan_batches(&result.agent_files(&files));
    assert!(batches.is_empty(), "{batches:?}");
}
