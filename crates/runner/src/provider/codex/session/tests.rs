use std::os::unix::fs::symlink;

use serde_json::json;
use tempfile::TempDir;

use super::*;

const THREAD: &str = "019a8f2c-4820-7000-8000-000000000482";
const OTHER_THREAD: &str = "019a8f2c-4820-7000-8000-000000000999";
const DAY: &str = "2026/11/16";

fn rollout_name(thread: &str) -> String {
    format!("rollout-2026-11-16T10-00-00-{thread}.jsonl")
}

fn token_count(input: u64, used_percent: f64, resets_at: i64) -> String {
    json!({"timestamp": "2026-11-16T10:00:01.000Z", "type": "event_msg", "payload": {"type": "token_count",
        "info": {"total_token_usage": {"input_tokens": input, "cached_input_tokens": 1, "output_tokens": 2}},
        "rate_limits": {"limit_id": "codex", "primary": {"used_percent": used_percent, "window_minutes": 300, "resets_at": resets_at},
            "secondary": {"used_percent": 41.0, "window_minutes": 10080, "resets_at": 1_900_000_000}}}})
    .to_string()
}

struct Dirs {
    _root: TempDir,
    home: PathBuf,
    store: PathBuf,
}

fn dirs() -> Dirs {
    let root = TempDir::new().unwrap();
    let home = root.path().join("codex-home");
    let store = root.path().join("sessions/analysis-a");
    std::fs::create_dir_all(&home).unwrap();
    Dirs {
        _root: root,
        home,
        store,
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn home_rollout(d: &Dirs, thread: &str) -> PathBuf {
    d.home
        .join(SESSIONS_DIR)
        .join(DAY)
        .join(rollout_name(thread))
}

fn store_rollout(d: &Dirs, thread: &str) -> PathBuf {
    d.store.join(DAY).join(rollout_name(thread))
}

fn files_below(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_below(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn restore_copies_the_stored_rollout_and_reports_its_usage() {
    let d = dirs();
    write(
        &store_rollout(&d, THREAD),
        &format!(
            "{{\"type\":\"session_meta\"}}\n{}\n{}\n",
            token_count(100, 10.0, 1),
            token_count(250, 12.0, 1)
        ),
    );
    let usage = restore(&d.store, THREAD, &d.home).unwrap();
    assert_eq!(usage.input_tokens, 250, "the latest cumulative total");
    assert_eq!(
        std::fs::read_to_string(home_rollout(&d, THREAD)).unwrap(),
        std::fs::read_to_string(store_rollout(&d, THREAD)).unwrap()
    );
}

#[test]
fn lost_session_is_session_unavailable() {
    let d = dirs();
    assert!(matches!(
        restore(&d.store, THREAD, &d.home),
        Err(ProviderError::SessionUnavailable)
    ));
    write(&store_rollout(&d, OTHER_THREAD), "{}\n");
    assert!(
        matches!(
            restore(&d.store, THREAD, &d.home),
            Err(ProviderError::SessionUnavailable)
        ),
        "another thread's rollout is not this session"
    );
    assert!(matches!(
        restore(&d.store, "../../etc", &d.home),
        Err(ProviderError::SessionUnavailable)
    ));
}

#[test]
fn session_files_exclude_auth() {
    let d = dirs();
    write(&d.home.join("auth.json"), "{\"tokens\":{}}");
    write(&d.home.join("config.toml"), "model = \"x\"");
    write(&d.home.join(SESSIONS_DIR).join("auth.json"), "{}");
    write(&home_rollout(&d, THREAD), "{}\n");
    save(&d.home, THREAD, &d.store, &Scrubber::default()).unwrap();
    assert_eq!(files_below(&d.store), [store_rollout(&d, THREAD)]);
}

#[test]
fn session_files_only_own_analysis_thread() {
    let d = dirs();
    write(&home_rollout(&d, THREAD), "{}\n");
    write(&home_rollout(&d, OTHER_THREAD), "{}\n");
    write(&store_rollout(&d, OTHER_THREAD), "{}\n");
    save(&d.home, THREAD, &d.store, &Scrubber::default()).unwrap();
    assert_eq!(
        files_below(&d.store),
        [store_rollout(&d, THREAD)],
        "only this thread is stored; an older thread of the analysis is replaced"
    );
}

#[test]
fn copy_back_rejects_symlink_and_foreign_thread() {
    let d = dirs();
    let secret = d.home.join("master-auth.json");
    write(&secret, "{\"refresh_token\":\"SECRET\"}");
    std::fs::create_dir_all(home_rollout(&d, THREAD).parent().unwrap()).unwrap();
    symlink(&secret, home_rollout(&d, THREAD)).unwrap();
    assert!(save(&d.home, THREAD, &d.store, &Scrubber::default()).is_err());
    assert!(!d.store.exists(), "nothing stored");

    let d = dirs();
    let secret = d.home.join("other.jsonl");
    write(&secret, "{}\n");
    std::fs::create_dir_all(home_rollout(&d, THREAD).parent().unwrap()).unwrap();
    std::fs::hard_link(&secret, home_rollout(&d, THREAD)).unwrap();
    assert!(
        save(&d.home, THREAD, &d.store, &Scrubber::default()).is_err(),
        "a hard link to another file is refused"
    );

    let d = dirs();
    std::fs::create_dir_all(d.home.join(SESSIONS_DIR)).unwrap();
    symlink(
        d.home.join("elsewhere"),
        d.home.join(SESSIONS_DIR).join("2026"),
    )
    .unwrap();
    write(
        &d.home.join("elsewhere/11/16").join(rollout_name(THREAD)),
        "{}\n",
    );
    assert!(
        save(&d.home, THREAD, &d.store, &Scrubber::default()).is_err(),
        "a symlinked directory is not followed"
    );

    let d = dirs();
    write(&home_rollout(&d, OTHER_THREAD), "{}\n");
    assert!(
        save(&d.home, THREAD, &d.store, &Scrubber::default()).is_err(),
        "a foreign thread alone is no session"
    );
}

#[test]
fn copy_back_size_capped() {
    let d = dirs();
    let path = home_rollout(&d, THREAD);
    write(&path, "{}\n");
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(MAX_ROLLOUT_BYTES + 1)
        .unwrap();
    assert!(save(&d.home, THREAD, &d.store, &Scrubber::default()).is_err());
}

#[test]
fn stored_session_files_are_scrubbed() {
    let d = dirs();
    write(
        &home_rollout(&d, THREAD),
        "{\"cmd\":\"echo ghs_CANARY\"}\n{\"auth\":\"rt-CANARY\"}\n",
    );
    save(
        &d.home,
        THREAD,
        &d.store,
        &Scrubber::new(["ghs_CANARY", "rt-CANARY"]),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(store_rollout(&d, THREAD)).unwrap(),
        "{\"cmd\":\"echo [redacted]\"}\n{\"auth\":\"[redacted]\"}\n"
    );
}

#[test]
fn usage_limit_resets_at_comes_from_the_exhausted_window() {
    let d = dirs();
    write(
        &home_rollout(&d, THREAD),
        &format!(
            "{}\n{}\n",
            token_count(1, 99.0, 1_700_000_000),
            token_count(2, 100.0, 1_794_990_905)
        ),
    );
    assert_eq!(
        usage_limit_resets_at(&d.home, THREAD),
        Timestamp::from_second(1_794_990_905)
    );
    write(
        &home_rollout(&d, THREAD),
        &token_count(1, 50.0, 1_700_000_000),
    );
    assert_eq!(
        usage_limit_resets_at(&d.home, THREAD),
        None,
        "no exhausted window: unknown"
    );
}

#[test]
fn thread_ids_are_hex_and_dashes_only() {
    assert!(valid_thread_id(THREAD));
    for bad in ["", "../x", "a/b", "019a 8f2c", &"a".repeat(65)] {
        assert!(!valid_thread_id(bad), "{bad}");
    }
}

#[test]
fn archived_turn_count_uses_completed_tasks_not_token_events() {
    let d = dirs();
    let complete = r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#;
    write(
        &home_rollout(&d, THREAD),
        &format!("{complete}\n{}\n{complete}\n", token_count(50, 1.0, 0)),
    );
    assert_eq!(
        save(&d.home, THREAD, &d.store, &Scrubber::default()).unwrap(),
        Some(2)
    );
    write(&home_rollout(&d, THREAD), &token_count(50, 1.0, 0));
    assert_eq!(
        save(&d.home, THREAD, &d.store, &Scrubber::default()).unwrap(),
        None
    );
}

#[test]
fn save_refuses_more_rollouts_of_the_thread_than_an_archive_holds() {
    let d = dirs();
    for hour in 10..15 {
        let name = format!("rollout-2026-11-16T{hour}-00-00-{THREAD}.jsonl");
        write(&d.home.join(SESSIONS_DIR).join(DAY).join(name), "{}\n");
    }
    assert!(save(&d.home, THREAD, &d.store, &Scrubber::default()).is_err());
    assert!(!d.store.exists(), "nothing is archived");
}

#[test]
fn save_refuses_rollouts_over_the_total_cap_before_copying() {
    let d = dirs();
    // Sparse files, each below the per-file cap: nothing is allocated and nothing may be read.
    for hour in 10..13 {
        let name = format!("rollout-2026-11-16T{hour}-00-00-{THREAD}.jsonl");
        let path = d.home.join(SESSIONS_DIR).join(DAY).join(name);
        write(&path, "");
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(200 * 1024 * 1024)
            .unwrap();
    }
    let started = std::time::Instant::now();
    assert!(save(&d.home, THREAD, &d.store, &Scrubber::default()).is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(!d.store.exists(), "nothing is archived");
}
