//! Drift guard for the generated artefacts (plan §5.1, ADR-009). `UPDATE_GENERATED=1` rewrites them.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use contracts::generate::{API_SCHEMA_DIR, API_SCHEMAS, SCHEMA_DIR, TS_DIR, generated_files};
use contracts::routes::{GET_ME, Listener, ROUTES, RouteAuth};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files_in(dir: &Path) -> BTreeSet<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn generated_artifacts_are_current() {
    let root = repo_root();
    let update = std::env::var("UPDATE_GENERATED").is_ok_and(|value| value == "1");
    let files = generated_files().expect("contracts generate");
    let expected: BTreeSet<PathBuf> = files.iter().map(|file| root.join(&file.path)).collect();
    let mut stale = Vec::new();
    for file in &files {
        let path = root.join(&file.path);
        if update {
            fs::create_dir_all(path.parent().expect("file has a parent"))
                .expect("create directory");
            fs::write(&path, &file.contents).expect("write generated file");
        } else if fs::read_to_string(&path).ok().as_deref() != Some(file.contents.as_str()) {
            stale.push(file.path.clone());
        }
    }
    for dir in [SCHEMA_DIR, API_SCHEMA_DIR, TS_DIR] {
        for extra in files_in(&root.join(dir)).difference(&expected) {
            if update {
                fs::remove_file(extra).expect("remove orphaned generated file");
            } else {
                stale.push(extra.display().to_string());
            }
        }
    }
    assert!(
        stale.is_empty(),
        "generated artefacts are stale: {stale:?}; run `UPDATE_GENERATED=1 cargo test -p contracts --test generated` \
         and commit the result"
    );
}

#[test]
fn typescript_has_no_bigint_and_unique_declarations() {
    let files = generated_files().expect("contracts generate");
    let types = files
        .iter()
        .find(|file| file.path.ends_with("/types.ts"))
        .expect("types.ts");
    assert!(!types.contents.contains("bigint"));
    for name in [
        "Tour",
        "AnalysisEvent",
        "ApiErrorCode",
        "UserFeedbackValue",
        "QuietRowRef",
        "LlmCallStatus",
    ] {
        let declaration = format!("export type {name} =");
        assert_eq!(types.contents.matches(&declaration).count(), 1, "{name}");
    }
}

#[test]
fn routes_ts_lists_only_public_routes() {
    let files = generated_files().expect("contracts generate");
    let routes = files
        .iter()
        .find(|file| file.path.ends_with("/routes.ts"))
        .expect("routes.ts");
    for route in ROUTES {
        let listed = routes.contents.contains(&format!("  {}: {{", route.name));
        assert_eq!(listed, route.listener == Listener::Public, "{}", route.name);
    }
}

/// Every route a personal access token may read names its response schema in `schemas/api/` exactly once, and
/// no schema names another route (plan v1.1 §E.5 E5-7, `docs/contracts/agent-api.md`).
#[test]
fn every_token_readable_route_has_a_response_schema() {
    let readable: BTreeSet<&str> = ROUTES
        .iter()
        .filter(|route| matches!(route.auth, RouteAuth::SessionOrToken(_)) || **route == GET_ME)
        .map(|route| route.name)
        .collect();
    let mut described = Vec::new();
    for api in API_SCHEMAS {
        described.extend(api.responses.iter().map(|route| route.name));
    }
    let unique: BTreeSet<&str> = described.iter().copied().collect();
    assert_eq!(
        unique.len(),
        described.len(),
        "a route with two schemas: {described:?}"
    );
    assert_eq!(unique, readable);
    let stems: BTreeSet<&str> = API_SCHEMAS.iter().map(|api| api.stem).collect();
    assert_eq!(stems.len(), API_SCHEMAS.len(), "duplicate file stem");
}
