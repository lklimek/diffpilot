//! Exercise the installed checker's entry point with the same context that a runner receives.
use std::path::Path;
use std::process::Command;

#[test]
fn checker_accepts_fixture_file_and_reports_line_numbered_rejections() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482");
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let built = Command::new(fixture.join("repo/build-repo.sh"))
        .arg(&repo)
        .output()
        .unwrap();
    assert!(built.status.success());
    let read = |file: &str| std::fs::read_to_string(fixture.join(file)).unwrap();
    let commits: serde_json::Value = serde_json::from_str(&read("repo/commits.json")).unwrap();
    let base = contracts::ids::CommitSha(commits["base"].as_str().unwrap().into());
    let head = contracts::ids::CommitSha(commits["head"].as_str().unwrap().into());
    let files = analysis::diff::diff(&repo, &base, &head).unwrap();
    let hunks: Vec<serde_json::Value> = serde_json::from_str(&read("repo/hunks.json")).unwrap();
    let mut listed = files.clone();
    for file in &mut listed {
        file.hunks.retain(|hunk| {
            hunks
                .iter()
                .any(|entry| entry["id"] == hunk.id.0 && entry["prepass"] == "agent")
        });
    }
    let context = analysis::tagged::Context {
        version: 1,
        base,
        head,
        files,
        listed,
        plan: serde_json::from_str(&read("agent/plan.json")).unwrap(),
        flow_id: contracts::ids::FlowId("f2".into()),
        moves: Default::default(),
        thread_aliases: vec![],
    };
    std::fs::write(
        root.path().join("flow-context.json"),
        serde_json::to_vec(&context).unwrap(),
    )
    .unwrap();
    std::fs::create_dir(root.path().join("out")).unwrap();
    let path = root.path().join("out/flow.md");
    let text = read("agent/flow-2.md");
    std::fs::write(&path, &text).unwrap();
    let check = || {
        Command::new(env!("CARGO_BIN_EXE_sr-check"))
            .arg("flow")
            .arg(&path)
            .output()
            .unwrap()
    };
    let good = check();
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    assert_eq!(good.stdout, b"ok\n");
    let bad = text.replace("risk: medium", "risk: impossible");
    std::fs::write(&path, &bad).unwrap();
    let rejected = check();
    assert!(!rejected.status.success());
    let errors = context
        .check(&bad, &repo)
        .unwrap_err()
        .into_iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        String::from_utf8(rejected.stderr).unwrap(),
        format!("{errors}\n")
    );
    std::fs::remove_file(&path).unwrap();
    assert!(!check().status.success());
}
