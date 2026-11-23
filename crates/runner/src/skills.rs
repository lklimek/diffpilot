//! The claudius skills the agent may use (ADR-002, S2): a runner-owned checkout of the claudius repository at
//! a configured ref, refreshed between jobs at an interval, and the files of the allow-listed skills a job
//! asks for, read from it for the provider to copy into the job. Only regular files are read; symlinks are
//! skipped.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use contracts::agent::JobKind;

use crate::git;

/// Largest total size of the skill files given to one job.
const MAX_SKILL_BYTES: u64 = 4 * 1024 * 1024;
/// Longest a refresh (fetch and checkout) may take; a stalled git server must not hold up the next claim.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(120);
/// Subdirectory of the claudius repository holding one directory per skill.
const SKILLS_DIR: &str = "skills";

/// The claudius checkout and the skills the agent may be given.
#[derive(Debug)]
pub struct Skills {
    url: String,
    reference: String,
    checkout: PathBuf,
    allowed: Vec<String>,
    interval: Duration,
    timeout: Duration,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    commit: Option<String>,
    attempted: Option<Instant>,
}

/// The files of the skills a job gets, relative to the skills root, and the claudius commit they come from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillFiles {
    /// The claudius commit.
    pub commit: String,
    /// `(path below the skills root, contents)`, e.g. `severity/SKILL.md`.
    pub files: Vec<(PathBuf, Vec<u8>)>,
}

/// Required analysis skills could not be supplied to a job.
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    /// No successfully refreshed checkout is configured.
    #[error("the analysis skills are unavailable; check the runner skills configuration")]
    Unavailable,
    /// A requested skill is not in the allow-list.
    #[error(
        "the job needs the skill {0}, which SR_RUNNER_SKILLS does not list; add it there (default \
         severity,check-pr-comments) or remove the setting"
    )]
    NotAllowed(String),
    /// A requested skill is missing or empty in the claudius checkout.
    #[error(
        "the job needs the skill {0}, which the claudius checkout lacks; check SR_RUNNER_CLAUDIUS_URL and \
         SR_RUNNER_CLAUDIUS_REF"
    )]
    Missing(String),
    /// The runner cannot read its skill checkout.
    #[error("the analysis skills could not be read; check the runner skills permissions")]
    Read(#[from] io::Error),
}

impl Skills {
    /// Skills from `url` at `reference`, checked out in `checkout`, refreshed every `interval`; only names in
    /// `allowed` are ever given.
    pub fn new(
        url: String,
        reference: String,
        checkout: PathBuf,
        allowed: Vec<String>,
        interval: Duration,
    ) -> Self {
        Self {
            url,
            reference,
            checkout,
            allowed,
            interval,
            timeout: REFRESH_TIMEOUT,
            state: Mutex::default(),
        }
    }

    /// The longest a refresh may take (default two minutes).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Fetches the ref when the interval has passed since the last attempt; a failure keeps the last checkout.
    pub async fn refresh_if_due(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state
                .attempted
                .is_some_and(|at| at.elapsed() < self.interval)
            {
                return;
            }
            state.attempted = Some(Instant::now());
        }
        // Dropping the git step on timeout kills its whole process group.
        let synced = tokio::time::timeout(
            self.timeout,
            git::sync_ref(&self.checkout, &self.url, &self.reference),
        )
        .await;
        let Ok(synced) = synced else {
            tracing::warn!(
                timeout_secs = self.timeout.as_secs(),
                "the claudius skills refresh timed out; the last checkout stays in use"
            );
            return;
        };
        match synced {
            Ok(commit) => {
                tracing::info!(%commit, reference = %self.reference, "claudius skills refreshed");
                self.state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .commit = Some(commit);
            }
            Err(error) => tracing::warn!(
                %error,
                "cannot refresh the claudius skills; check SR_RUNNER_CLAUDIUS_URL and SR_RUNNER_CLAUDIUS_REF and the runner's proxy"
            ),
        }
    }

    /// Gives every required skill or fails before the agent can run without its instructions.
    pub fn required_files(&self, requested: &[String]) -> Result<SkillFiles, SkillError> {
        if let Some(name) = requested.iter().find(|name| !self.allowed.contains(name)) {
            return Err(SkillError::NotAllowed(name.clone()));
        }
        let given = self.files(requested)?.ok_or(SkillError::Unavailable)?;
        for name in requested {
            let manifest = Path::new(name).join("SKILL.md");
            if !given
                .files
                .iter()
                .any(|(path, bytes)| path == &manifest && !bytes.is_empty())
            {
                return Err(SkillError::Missing(name.clone()));
            }
        }
        Ok(given)
    }

    /// The skills some job kind requires that the allow-list leaves out, in job-kind order.
    pub fn missing_required(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        for kind in JobKind::ALL {
            for skill in kind.required_skills() {
                if !self.allowed.iter().any(|allowed| allowed == skill) && !missing.contains(skill)
                {
                    missing.push(*skill);
                }
            }
        }
        missing
    }

    /// The files of the requested skills that are allowed and present; `None` before the first refresh.
    pub fn files(&self, requested: &[String]) -> io::Result<Option<SkillFiles>> {
        let Some(commit) = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .commit
            .clone()
        else {
            return Ok(None);
        };
        let mut files = Vec::new();
        let mut budget = MAX_SKILL_BYTES;
        for name in requested {
            if !self.allowed.contains(name) {
                tracing::warn!(skill = %name, "a job asked for a skill outside SR_RUNNER_SKILLS; not given");
                continue;
            }
            let dir = self.checkout.join(SKILLS_DIR).join(name);
            if !dir.symlink_metadata().is_ok_and(|meta| meta.is_dir()) {
                tracing::warn!(skill = %name, "the claudius checkout has no such skill");
                continue;
            }
            collect(&dir, Path::new(name), &mut files, &mut budget)?;
        }
        Ok(Some(SkillFiles { commit, files }))
    }
}

fn collect(
    dir: &Path,
    rel: &Path,
    files: &mut Vec<(PathBuf, Vec<u8>)>,
    budget: &mut u64,
) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let rel = rel.join(entry.file_name());
        if kind.is_dir() {
            collect(&entry.path(), &rel, files, budget)?;
        } else if kind.is_file() {
            let len = entry.metadata()?.len();
            *budget = budget.checked_sub(len).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "the skills exceed the size cap")
            })?;
            files.push((rel, std::fs::read(entry.path())?));
        }
    }
    files.sort();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;

    /// A claudius-like repository with two skills and a symlink; returns its URL.
    fn claudius(root: &Path) -> String {
        let repo = root.join("claudius");
        std::fs::create_dir_all(repo.join("skills/severity/references")).unwrap();
        std::fs::create_dir_all(repo.join("skills/delegate")).unwrap();
        std::fs::write(
            repo.join("skills/severity/SKILL.md"),
            "---\nname: severity\n---\n",
        )
        .unwrap();
        std::fs::write(repo.join("skills/severity/references/bands.md"), "bands\n").unwrap();
        std::fs::write(repo.join("skills/delegate/SKILL.md"), "delegate\n").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", repo.join("skills/severity/passwd")).unwrap();
        for args in [
            &["init", "--quiet", "-b", "main"][..],
            &["add", "-A"],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "-m",
                "skills",
            ],
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        format!("file://{}", repo.display())
    }

    /// A pinned `SR_RUNNER_SKILLS` without a skill some job kind requires is named at start-up, and a job asking
    /// for it fails with a message naming the setting to change.
    #[tokio::test]
    async fn required_skills_outside_the_allow_list_are_named() {
        let root = TempDir::new().unwrap();
        let pinned = Skills::new(
            claudius(root.path()),
            "main".to_owned(),
            root.path().join("checkout"),
            vec!["severity".to_owned()],
            Duration::from_secs(3600),
        );
        assert_eq!(pinned.missing_required(), ["check-pr-comments"]);
        pinned.refresh_if_due().await;
        let error = pinned
            .required_files(&["severity".to_owned(), "check-pr-comments".to_owned()])
            .unwrap_err();
        assert!(
            matches!(&error, SkillError::NotAllowed(name) if name == "check-pr-comments"),
            "{error:?}"
        );
        assert!(error.to_string().contains("SR_RUNNER_SKILLS"), "{error}");
        let error = pinned
            .required_files(&["severity".to_owned(), "delegate".to_owned()])
            .unwrap_err();
        assert!(matches!(error, SkillError::NotAllowed(_)), "{error:?}");
        let default = Skills::new(
            String::new(),
            "main".to_owned(),
            root.path().join("other"),
            vec!["severity".to_owned(), "check-pr-comments".to_owned()],
            Duration::from_secs(3600),
        );
        assert!(default.missing_required().is_empty());
    }

    #[tokio::test]
    async fn refresh_checks_out_the_ref_and_gives_only_allowed_skills() {
        let root = TempDir::new().unwrap();
        let skills = Skills::new(
            claudius(root.path()),
            "main".to_owned(),
            root.path().join("checkout"),
            vec!["severity".to_owned()],
            Duration::from_secs(3600),
        );
        assert_eq!(skills.files(&["severity".to_owned()]).unwrap(), None);
        skills.refresh_if_due().await;
        let given = skills
            .files(&[
                "severity".to_owned(),
                "delegate".to_owned(),
                "../x".to_owned(),
            ])
            .unwrap()
            .expect("refreshed");
        assert_eq!(given.commit.len(), 40);
        let paths: Vec<&Path> = given.files.iter().map(|(rel, _)| rel.as_path()).collect();
        assert_eq!(
            paths,
            [
                Path::new("severity/SKILL.md"),
                Path::new("severity/references/bands.md")
            ],
            "not allow-listed skills and symlinks are never given"
        );
    }

    #[tokio::test]
    async fn refresh_waits_for_the_interval_and_keeps_the_last_checkout_on_failure() {
        let root = TempDir::new().unwrap();
        let url = claudius(root.path());
        let skills = Skills::new(
            url,
            "main".to_owned(),
            root.path().join("checkout"),
            vec!["severity".to_owned()],
            Duration::from_secs(3600),
        );
        skills.refresh_if_due().await;
        let first = skills.files(&[]).unwrap().unwrap().commit;
        std::fs::remove_dir_all(root.path().join("claudius")).unwrap();
        skills.refresh_if_due().await;
        assert_eq!(skills.files(&[]).unwrap().unwrap().commit, first);
        let broken = Skills::new(
            "file:///nonexistent/claudius".to_owned(),
            "--upload-pack=x".to_owned(),
            root.path().join("other"),
            vec![],
            Duration::ZERO,
        );
        broken.refresh_if_due().await;
        assert_eq!(broken.files(&[]).unwrap(), None);
    }

    #[tokio::test]
    async fn stalled_git_server_ends_the_refresh_at_its_timeout() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/claudius.git", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            // Accepts and never answers.
            let held = listener.accept();
            std::thread::sleep(Duration::from_secs(60));
            drop(held);
        });
        let root = TempDir::new().unwrap();
        let skills = Skills::new(
            url,
            "main".to_owned(),
            root.path().join("checkout"),
            vec!["severity".to_owned()],
            Duration::ZERO,
        )
        .with_timeout(Duration::from_secs(1));
        let started = Instant::now();
        skills.refresh_if_due().await;
        assert!(started.elapsed() < Duration::from_secs(20));
        assert_eq!(skills.files(&[]).unwrap(), None);
    }

    #[tokio::test]
    async fn a_commit_ref_pins_the_checkout() {
        let root = TempDir::new().unwrap();
        let url = claudius(root.path());
        let head = String::from_utf8(
            Command::new("git")
                .arg("-C")
                .arg(root.path().join("claudius"))
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();
        let pinned = Skills::new(
            url.clone(),
            head.clone(),
            root.path().join("pinned"),
            vec![],
            Duration::ZERO,
        );
        pinned.refresh_if_due().await;
        assert_eq!(pinned.files(&[]).unwrap().unwrap().commit, head);
        let wrong = Skills::new(
            url,
            "0".repeat(40),
            root.path().join("wrong"),
            vec![],
            Duration::ZERO,
        );
        wrong.refresh_if_due().await;
        assert_eq!(wrong.files(&[]).unwrap(), None);
    }
}
