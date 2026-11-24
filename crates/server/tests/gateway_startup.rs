//! Gateway startup failures identify settings without exposing synthetic secret contents.

use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn invalid_secret_files_are_named_without_printing_their_contents() {
    for (file, content, mode, setting) in [
        ("key", None, 0o400, "SR_GATEWAY_SECRET_KEY_FILE"),
        (
            "key",
            Some("CANARY-invalid-base64".to_owned()),
            0o400,
            "SR_GATEWAY_SECRET_KEY_FILE",
        ),
        (
            "key",
            Some(STANDARD.encode([0_u8; 31])),
            0o400,
            "SR_GATEWAY_SECRET_KEY_FILE",
        ),
        (
            "key",
            Some(STANDARD.encode([0_u8; 33])),
            0o400,
            "SR_GATEWAY_SECRET_KEY_FILE",
        ),
        (
            "key",
            Some(STANDARD.encode([0_u8; 32])),
            0o444,
            "SR_GATEWAY_SECRET_KEY_FILE",
        ),
        (
            "credentials",
            None,
            0o400,
            "SR_GATEWAY_CREDENTIALS_TOKEN_FILE",
        ),
        ("keys", None, 0o400, "SR_GATEWAY_KEYS_TOKEN_FILE"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        for (name, value) in [
            ("key", STANDARD.encode([0_u8; 32])),
            (
                "credentials",
                "CANARY-credentials-scope-0123456789".to_owned(),
            ),
            ("keys", "CANARY-keys-scope-0123456789abcdef".to_owned()), // gitleaks:allow (test canary)
        ] {
            std::fs::write(dir.path().join(name), value).unwrap();
            std::fs::set_permissions(
                dir.path().join(name),
                std::fs::Permissions::from_mode(0o400),
            )
            .unwrap();
        }
        let path = dir.path().join(file);
        if let Some(content) = &content {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::write(&path, content).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_diffpilot-gateway"))
            .env_clear()
            .env("SR_GATEWAY_MODEL_ADDR", "127.0.0.1:8090")
            .env("SR_GATEWAY_ADMIN_ADDR", "127.0.0.2:8091")
            .env("SR_GATEWAY_STORE", dir.path().join("store"))
            .env("SR_GATEWAY_SECRET_KEY_FILE", dir.path().join("key"))
            .env(
                "SR_GATEWAY_CREDENTIALS_TOKEN_FILE",
                dir.path().join("credentials"),
            )
            .env("SR_GATEWAY_KEYS_TOKEN_FILE", dir.path().join("keys"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("gateway accepted invalid {setting}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        let message = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(message.contains(setting), "{file}: {message}");
        assert!(!message.contains("CANARY"), "secret printed");
        if let Some(content) = content {
            assert!(!message.contains(&content), "secret printed");
        }
    }
}
