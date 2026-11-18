use std::os::unix::fs::{PermissionsExt, symlink};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::json;
use tempfile::TempDir;

use super::*;

/// A fake unsigned JWT whose `https://api.openai.com/auth` claims are `claims`; `jti` tells tokens with the
/// same claims apart.
fn jwt_as(claims: serde_json::Value, jti: &str) -> String {
    let segment = |value: serde_json::Value| URL_SAFE_NO_PAD.encode(value.to_string());
    format!(
        "{}.{}.c2lnbmF0dXJl",
        segment(json!({"alg": "none"})),
        segment(json!({"https://api.openai.com/auth": claims, "jti": jti}))
    )
}

fn jwt(claims: serde_json::Value) -> String {
    jwt_as(claims, "first")
}

/// A fake token of `user` in `account`.
fn token(account: &str, user: &str) -> String {
    jwt(json!({"chatgpt_account_id": account, "chatgpt_user_id": user}))
}

/// The access token of `user-1` in `account` issued together with the refresh token `refresh`.
fn access_token(account: &str, refresh: &str) -> String {
    jwt_as(
        json!({"chatgpt_account_id": account, "chatgpt_user_id": "user-1"}),
        refresh,
    )
}

/// A ChatGPT `auth.json` with the stored account field `account` and the given id and access tokens.
fn auth_with(refresh: &str, account: &str, id_token: &str, access_token: &str) -> Vec<u8> {
    json!({"auth_mode": "chatgpt", "tokens": {"id_token": id_token, "access_token": access_token, "refresh_token": refresh, "account_id": account}})
        .to_string()
        .into_bytes()
}

/// The credential of `user-1` in `account`; every refresh has its own access token, the id token stays.
fn auth(refresh: &str, account: &str) -> Vec<u8> {
    auth_with(
        refresh,
        account,
        &token(account, "user-1"),
        &access_token(account, refresh),
    )
}

struct Dirs {
    _root: TempDir,
    master: PathBuf,
    homes: PathBuf,
    home: PathBuf,
}

fn dirs() -> Dirs {
    let root = TempDir::new().unwrap();
    let master = root.path().join("credentials/auth.json");
    std::fs::create_dir_all(master.parent().unwrap()).unwrap();
    std::fs::write(&master, auth("rt-original", "acct-1")).unwrap();
    let homes = root.path().join("codex-homes");
    let home = homes.join("job-1");
    std::fs::create_dir_all(&home).unwrap();
    Dirs {
        _root: root,
        master,
        homes,
        home,
    }
}

fn install<'a>(credential: &'a CodexCredential, d: &Dirs) -> JobCredential<'a> {
    JobCredential::install(
        credential,
        &d.home,
        JobId::new_random(),
        Scrubber::default(),
    )
    .unwrap()
}

fn master(d: &Dirs) -> Vec<u8> {
    std::fs::read(&d.master).unwrap()
}

#[test]
fn unchanged_copy_leaves_the_credential_usable() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    drop(install(&credential, &d));
    assert!(credential.usable());
    assert_eq!(master(&d), auth("rt-original", "acct-1"));
    assert!(
        !base_file(&d.home).exists(),
        "the CAS base is removed once settled"
    );
}

#[test]
fn auth_writeback_cas_makes_a_refreshed_copy_the_master() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    let mut job = install(&credential, &d);
    std::fs::write(d.home.join(AUTH_FILE), auth("rt-rotated", "acct-1")).unwrap();
    assert!(job.settle(false));
    assert_eq!(master(&d), auth("rt-rotated", "acct-1"));
    assert_eq!(
        std::fs::metadata(&d.master).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(credential.usable(), "a refresh keeps the credential usable");
}

#[test]
fn refreshed_copy_is_written_back_when_the_call_is_dropped() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    let job = install(&credential, &d);
    std::fs::write(d.home.join(AUTH_FILE), auth("rt-rotated", "acct-1")).unwrap();
    drop(job);
    assert_eq!(master(&d), auth("rt-rotated", "acct-1"));
}

#[test]
fn auth_writeback_rejects_account_change() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    let mut job = install(&credential, &d);
    std::fs::write(d.home.join(AUTH_FILE), auth("rt-attacker", "acct-OTHER")).unwrap();
    job.settle(false);
    assert_eq!(master(&d), auth("rt-original", "acct-1"));
    assert!(
        !credential.usable(),
        "the original refresh token may be spent"
    );
}

/// The agent can write the `account_id` field, so the identity is read from the token claims.
#[test]
fn auth_writeback_rejects_another_identity_behind_the_master_account_field() {
    let own = token("acct-1", "user-1");
    let other_account = token("acct-OTHER", "user-1");
    let other_user = token("acct-1", "user-OTHER");
    let cases = [
        (
            "tokens of another account",
            auth_with("rt-attacker", "acct-1", &other_account, &other_account),
        ),
        (
            "tokens of another user",
            auth_with("rt-attacker", "acct-1", &other_user, &other_user),
        ),
        (
            "an access token of another account",
            auth_with("rt-attacker", "acct-1", &own, &other_account),
        ),
        (
            "an id token of another user",
            auth_with("rt-attacker", "acct-1", &other_user, &own),
        ),
        (
            "tokens without identity claims",
            auth_with("rt-attacker", "acct-1", "opaque-id", "opaque-access"),
        ),
        (
            "tokens with empty identity claims",
            auth_with("rt-attacker", "acct-1", &token("", ""), &token("", "")),
        ),
    ];
    for (case, copy) in cases {
        let d = dirs();
        let credential = CodexCredential::new(d.master.clone());
        let mut job = install(&credential, &d);
        std::fs::write(d.home.join(AUTH_FILE), copy).unwrap();
        job.settle(false);
        assert_eq!(master(&d), auth("rt-original", "acct-1"), "{case}");
        assert!(!credential.usable(), "{case}: the master may be spent");
    }
}

/// Claim layouts Codex accepts (`login/src/token_data.rs`: every auth claim is optional, the access token is a
/// plain string; `auth/manager.rs`: every field of a refresh response is optional), with fake values.
#[test]
fn write_back_follows_the_claim_layouts_codex_allows() {
    let id = |account: &str, user: Option<&str>| match user {
        Some(user) => token(account, user),
        None => jwt(json!({"chatgpt_account_id": account})),
    };
    let access = |account: &str, user: &str, jti: &str| {
        jwt_as(
            json!({"chatgpt_account_id": account, "chatgpt_user_id": user}),
            jti,
        )
    };
    let full = id("acct-1", Some("user-1"));
    let no_user = id("acct-1", None);
    let legacy = jwt(json!({"chatgpt_account_id": "acct-1", "user_id": "user-1"}));
    // (case, master id token, master access token, copy account field, copy id token, copy access token, accepted)
    let cases = [
        (
            "a refresh response without an id token: Codex keeps the old one",
            full.clone(),
            access("acct-1", "user-1", "old"),
            "acct-1",
            full.clone(),
            access("acct-1", "user-1", "new"),
            true,
        ),
        (
            "an access token that is not a JWT, before and after",
            full.clone(),
            "access-token".to_owned(),
            "acct-1",
            full.clone(),
            "new-access-token".to_owned(),
            true,
        ),
        (
            "an opaque access token replaced by one naming the master's account",
            full.clone(),
            "access-token".to_owned(),
            "acct-1",
            full.clone(),
            access("acct-1", "user-1", "new"),
            true,
        ),
        (
            "no user id in any token",
            no_user.clone(),
            jwt_as(json!({"chatgpt_account_id": "acct-1"}), "old"),
            "acct-1",
            no_user.clone(),
            jwt_as(json!({"chatgpt_account_id": "acct-1"}), "new"),
            true,
        ),
        (
            "an id token that gains a user id",
            no_user.clone(),
            "access-token".to_owned(),
            "acct-1",
            full.clone(),
            "new-access-token".to_owned(),
            true,
        ),
        (
            "the `user_id` claim Codex falls back to",
            legacy.clone(),
            jwt_as(
                json!({"chatgpt_account_id": "acct-1", "user_id": "user-1"}),
                "old",
            ),
            "acct-1",
            legacy.clone(),
            jwt_as(
                json!({"chatgpt_account_id": "acct-1", "user_id": "user-1"}),
                "new",
            ),
            true,
        ),
        (
            "an id token that drops the master's user id",
            full.clone(),
            access("acct-1", "user-1", "old"),
            "acct-1",
            no_user.clone(),
            access("acct-1", "user-1", "new"),
            false,
        ),
        (
            "an access token that drops the master's claims",
            full.clone(),
            access("acct-1", "user-1", "old"),
            "acct-1",
            full.clone(),
            "new-access-token".to_owned(),
            false,
        ),
        (
            "account changed",
            full.clone(),
            access("acct-1", "user-1", "old"),
            "acct-2",
            id("acct-2", Some("user-1")),
            access("acct-2", "user-1", "new"),
            false,
        ),
        (
            "user changed",
            full.clone(),
            access("acct-1", "user-1", "old"),
            "acct-1",
            id("acct-1", Some("user-2")),
            access("acct-1", "user-1", "new"),
            false,
        ),
        (
            "account field kept but id token of another account",
            full.clone(),
            access("acct-1", "user-1", "old"),
            "acct-1",
            id("acct-2", Some("user-1")),
            access("acct-1", "user-1", "new"),
            false,
        ),
        (
            "access token of another user",
            full.clone(),
            access("acct-1", "user-1", "old"),
            "acct-1",
            full.clone(),
            access("acct-1", "user-2", "new"),
            false,
        ),
        (
            "an opaque access token replaced by one naming another account",
            full.clone(),
            "access-token".to_owned(),
            "acct-1",
            full.clone(),
            access("acct-2", "user-1", "new"),
            false,
        ),
    ];
    for (case, master_id, master_access, account, copy_id, copy_access, accepted) in cases {
        let d = dirs();
        let original = auth_with("rt-original", "acct-1", &master_id, &master_access);
        std::fs::write(&d.master, &original).unwrap();
        let credential = CodexCredential::new(d.master.clone());
        let mut job = install(&credential, &d);
        let copy = auth_with("rt-rotated", account, &copy_id, &copy_access);
        std::fs::write(d.home.join(AUTH_FILE), &copy).unwrap();
        job.settle(false);
        let expected = if accepted { &copy } else { &original };
        assert_eq!(&master(&d), expected, "{case}");
        assert_eq!(credential.usable(), accepted, "{case}");
    }
}

/// A master whose refreshed copies could never be accepted is refused before a job spends its refresh token.
#[test]
fn master_without_an_account_bound_id_token_is_refused_before_a_job_starts() {
    let own = token("acct-1", "user-1");
    let masters = [
        (
            "no stored account id",
            json!({"auth_mode": "chatgpt", "tokens": {"id_token": own, "access_token": own, "refresh_token": "rt-original"}})
                .to_string()
                .into_bytes(),
        ),
        (
            "an id token that is not a JWT",
            auth_with("rt-original", "acct-1", "opaque-id", &own),
        ),
        (
            "an id token without an account claim",
            auth_with(
                "rt-original",
                "acct-1",
                &jwt(json!({"chatgpt_user_id": "user-1"})),
                &own,
            ),
        ),
        (
            "an id token of another account than the stored one",
            auth_with(
                "rt-original",
                "acct-1",
                &token("acct-2", "user-1"),
                &own,
            ),
        ),
    ];
    for (case, bytes) in masters {
        let d = dirs();
        std::fs::write(&d.master, &bytes).unwrap();
        let credential = CodexCredential::new(d.master.clone());
        assert!(!credential.usable(), "{case}");
        assert!(
            matches!(
                JobCredential::install(
                    &credential,
                    &d.home,
                    JobId::new_random(),
                    Scrubber::default()
                ),
                Err(ProviderError::CredentialUnusable)
            ),
            "{case}"
        );
        assert!(!d.home.join(AUTH_FILE).exists(), "{case}: no copy is made");
        assert_eq!(master(&d), bytes, "{case}");
        assert!(
            !CodexCredential::new(d.master.clone()).usable(),
            "{case}: also after a restart"
        );
        std::fs::write(&d.master, auth("rt-new-login", "acct-1")).unwrap();
        assert!(credential.usable(), "{case}: a new login is usable");
    }
}

#[test]
fn auth_writeback_rejects_symlink() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    let mut job = install(&credential, &d);
    let target = d.home.join("other-secret");
    std::fs::write(&target, auth("rt-other", "acct-1")).unwrap();
    std::fs::remove_file(d.home.join(AUTH_FILE)).unwrap();
    symlink(&target, d.home.join(AUTH_FILE)).unwrap();
    assert!(
        !job.settle(false),
        "an unreadable copy leaves the scrubber uncertain"
    );
    assert_eq!(
        master(&d),
        auth("rt-original", "acct-1"),
        "a symlink target is never written back"
    );
    assert!(!credential.usable());
}

#[test]
fn oversized_or_non_credential_copy_is_rejected() {
    for contents in [vec![b' '; 1024 * 1024], b"not json".to_vec()] {
        let d = dirs();
        let credential = CodexCredential::new(d.master.clone());
        let job = install(&credential, &d);
        std::fs::write(d.home.join(AUTH_FILE), &contents).unwrap();
        drop(job);
        assert_eq!(master(&d), auth("rt-original", "acct-1"));
        assert!(!credential.usable());
    }
}

#[test]
fn cas_conflict_marks_the_credential_unusable() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    let job = install(&credential, &d);
    std::fs::write(&d.master, auth("rt-elsewhere", "acct-1")).unwrap();
    std::fs::write(d.home.join(AUTH_FILE), auth("rt-rotated", "acct-1")).unwrap();
    drop(job);
    assert_eq!(
        master(&d),
        auth("rt-elsewhere", "acct-1"),
        "a changed master is never overwritten"
    );
    assert!(!credential.usable());
}

#[test]
fn refresh_reused_marks_credential_unusable_until_a_new_login() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    install(&credential, &d).settle(true);
    assert!(!credential.usable());
    assert!(
        !CodexCredential::new(d.master.clone()).usable(),
        "the state survives a runner restart"
    );
    std::fs::write(&d.master, auth("rt-new-login", "acct-1")).unwrap();
    assert!(
        credential.usable(),
        "signing in again replaces the master and clears the state"
    );
    assert!(CodexCredential::new(d.master.clone()).usable());
}

#[test]
fn unusable_credential_is_refused_at_install() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    install(&credential, &d).settle(true);
    std::fs::remove_dir_all(&d.home).unwrap();
    std::fs::create_dir(&d.home).unwrap();
    assert!(matches!(
        JobCredential::install(
            &credential,
            &d.home,
            JobId::new_random(),
            Scrubber::default()
        ),
        Err(ProviderError::CredentialUnusable)
    ));
    assert!(!d.home.join(AUTH_FILE).exists());
}

#[test]
fn missing_or_foreign_master_is_unusable() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    std::fs::write(&d.master, json!({"auth_mode": "apikey"}).to_string()).unwrap();
    assert!(!credential.usable());
    std::fs::remove_file(&d.master).unwrap();
    assert!(!credential.usable());
}

#[test]
fn rotated_tokens_join_the_scrubber() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    let mut job = install(&credential, &d);
    std::fs::remove_file(d.home.join(AUTH_FILE)).unwrap();
    std::fs::write(d.home.join(AUTH_FILE), auth("rt-new-SECRET", "acct-1")).unwrap();
    job.watch();
    let rotated = access_token("acct-1", "rt-new-SECRET");
    assert_ne!(rotated, access_token("acct-1", "rt-original"));
    assert_eq!(
        job.scrubber()
            .text(&format!("rt-new-SECRET {rotated} rt-original")),
        "[redacted] [redacted] [redacted]"
    );
}

#[test]
fn rewriting_the_copy_too_often_ends_the_call_without_forgetting_secrets() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    let mut job = install(&credential, &d);
    let mut accepted = 0;
    for round in 0..20 {
        std::fs::write(
            d.home.join(AUTH_FILE),
            auth(&format!("rt-round-{round}"), "acct-1"),
        )
        .unwrap();
        if !job.watch() {
            break;
        }
        accepted += 1;
    }
    assert!(accepted < 20, "the number of remembered tokens is capped");
    assert_eq!(job.scrubber().text("rt-round-0"), "[redacted]");
}

#[test]
fn fifo_copy_does_not_block_the_reader() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    let job = install(&credential, &d);
    std::fs::remove_file(d.home.join(AUTH_FILE)).unwrap();
    mkfifo(&d.home.join(AUTH_FILE));
    drop(job);
    assert!(!credential.usable());
}

#[test]
fn fifo_at_the_temp_path_does_not_hang_the_write_back() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    let job = install(&credential, &d);
    mkfifo(&d.master.with_file_name("auth.json.tmp"));
    std::fs::write(d.home.join(AUTH_FILE), auth("rt-rotated", "acct-1")).unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            drop(job);
            let _ = done.send(());
        });
        assert!(
            finished.recv_timeout(Duration::from_secs(10)).is_ok(),
            "write-back hung"
        );
    });
    assert_eq!(master(&d), auth("rt-rotated", "acct-1"));
}

fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn failed_write_back_keeps_the_copy_for_the_next_start() {
    let d = dirs();
    let credentials = d.master.parent().unwrap().to_path_buf();
    let credential = CodexCredential::new(d.master.clone());
    let job = install(&credential, &d);
    std::fs::write(d.home.join(AUTH_FILE), auth("rt-rotated", "acct-1")).unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o500)).unwrap();
    drop(job);
    let usable_meanwhile = credential.usable();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!usable_meanwhile, "the master's refresh token may be spent");
    assert!(!d.home.exists(), "moved out of the job cleanup's way");
    let restarted = CodexCredential::new(d.master.clone());
    assert_eq!(restarted.recover(&d.homes).unwrap(), 1);
    assert_eq!(master(&d), auth("rt-rotated", "acct-1"));
    assert!(restarted.usable());
    assert_eq!(std::fs::read_dir(&d.homes).unwrap().count(), 0);
}

#[test]
fn startup_recovers_newer_auth_from_crashed_job_dir() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    let job = install(&credential, &d);
    std::fs::write(d.home.join(AUTH_FILE), auth("rt-rotated", "acct-1")).unwrap();
    // A crash: the job never settles.
    std::mem::forget(job);
    std::fs::create_dir(d.homes.join("job-2")).unwrap();
    let restarted = CodexCredential::new(d.master.clone());
    assert_eq!(restarted.recover(&d.homes).unwrap(), 1);
    assert_eq!(master(&d), auth("rt-rotated", "acct-1"));
    assert!(restarted.usable());
    assert_eq!(
        std::fs::read_dir(&d.homes).unwrap().count(),
        0,
        "every leftover home is removed"
    );
}

#[test]
fn recovery_rejects_a_foreign_copy_and_marks_the_credential_unusable() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    let job = install(&credential, &d);
    std::fs::write(d.home.join(AUTH_FILE), auth("rt-x", "acct-OTHER")).unwrap();
    std::mem::forget(job);
    let restarted = CodexCredential::new(d.master.clone());
    assert_eq!(restarted.recover(&d.homes).unwrap(), 0);
    assert_eq!(master(&d), auth("rt-original", "acct-1"));
    assert!(!restarted.usable());
    assert_eq!(std::fs::read_dir(&d.homes).unwrap().count(), 0);
}

#[test]
fn recovery_of_an_unchanged_copy_only_cleans_up() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    std::mem::forget(install(&credential, &d));
    let restarted = CodexCredential::new(d.master.clone());
    assert_eq!(restarted.recover(&d.homes).unwrap(), 0);
    assert!(restarted.usable());
    assert_eq!(std::fs::read_dir(&d.homes).unwrap().count(), 0);
}

/// After a crash the owner signed in again, or a later job wrote a newer credential back: the leftover copy of
/// the older master is stale and must not cost the newer one.
#[test]
fn recovery_of_a_copy_from_an_older_master_leaves_the_newer_master_alone() {
    let d = dirs();
    let credential = CodexCredential::new(d.master.clone());
    let job = install(&credential, &d);
    std::fs::write(d.home.join(AUTH_FILE), auth("rt-rotated", "acct-1")).unwrap();
    std::mem::forget(job);
    std::fs::write(&d.master, auth("rt-newer-login", "acct-1")).unwrap();
    let restarted = CodexCredential::new(d.master.clone());
    assert_eq!(restarted.recover(&d.homes).unwrap(), 0);
    assert_eq!(master(&d), auth("rt-newer-login", "acct-1"));
    assert!(restarted.usable());
    assert_eq!(std::fs::read_dir(&d.homes).unwrap().count(), 0);
}
