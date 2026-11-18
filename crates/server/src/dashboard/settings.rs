//! Per-user dashboard settings (plan §F.6): the rules, checked in the order and with the messages of the web
//! client's mirror (`web/src/components/dashboard/settings-dashboard/validation.ts`; the server is
//! authoritative and adds the time zone and the policy-path characters), the effective MemCan projects (G17) and
//! the settings a refresh reads.

use contracts::api::dashboard::{DashboardSettings, DashboardSettingsView, RepositoryRule};
use contracts::common::RepoRef;
use db::error::DbError;
use db::repo::Db;

/// Most repository rows.
const MAX_REPOSITORIES: usize = 50;
/// Most release branch patterns per row.
const MAX_BRANCH_PATTERNS: usize = 10;
/// Most characters of the ready label.
const MAX_READY_LABEL_CHARS: usize = 50;

/// A rule of plan §F.6 is broken; the message names the field and the rule (422 `invalid_dashboard_settings`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidSettings(String);

/// The settings of a user: stored, else the defaults (`DashboardSettings::default`, plan §F.6).
pub async fn effective(
    db: &Db,
    tenant_id: &str,
    user_id: &str,
) -> Result<DashboardSettings, DbError> {
    Ok(db
        .dashboard()
        .settings(tenant_id, user_id)
        .await?
        .map(|stored| stored.settings)
        .unwrap_or_default())
}

/// Effective MemCan projects (G17): the repository rows' short names (`acme/platform` → `platform`), then the
/// extra names, without repeats (compared as written, like MemCan's project names).
pub fn memcan_projects(settings: &DashboardSettings) -> Vec<String> {
    let mut projects: Vec<String> = Vec::new();
    let names = settings
        .repositories
        .iter()
        .map(|row| &row.repo.name)
        .chain(&settings.memcan_extra_projects);
    for name in names {
        if !projects.contains(name) {
            projects.push(name.clone());
        }
    }
    projects
}

/// The entries of `reported` (repositories `owner/name` and owners the GitHub token could not see at the latest
/// refresh) that still name a repository row or an owner to enumerate of `settings`.
pub fn unreachable_marks(settings: &DashboardSettings, reported: &[String]) -> Vec<String> {
    reported
        .iter()
        .filter(|entry| {
            settings
                .repositories
                .iter()
                .any(|row| same(&repo_text(&row.repo), entry))
                || settings
                    .enumerate_owners
                    .iter()
                    .any(|owner| same(owner, entry))
        })
        .cloned()
        .collect()
}

/// The response of R6 for `settings`.
pub fn view(
    settings: DashboardSettings,
    own_login: String,
    memcan_configured: bool,
    reported_unreachable: &[String],
) -> DashboardSettingsView {
    DashboardSettingsView {
        memcan_projects: memcan_projects(&settings),
        unreachable: unreachable_marks(&settings, reported_unreachable),
        settings,
        defaults: DashboardSettings::default(),
        own_login,
        memcan_configured,
    }
}

/// `settings` ready to store: the ready label trimmed, then every rule of plan §F.6 checked; the error is the
/// first broken rule in form order, worded as the client words it.
pub fn validated(
    mut settings: DashboardSettings,
    own_login: &str,
) -> Result<DashboardSettings, InvalidSettings> {
    settings.ready_label = js_trim(&settings.ready_label).to_owned();
    check_list(ListField::LinkedLogins, &settings.linked_logins, own_login)?;
    check_repositories(&settings.repositories)?;
    check_list(
        ListField::EnumerateOwners,
        &settings.enumerate_owners,
        own_login,
    )?;
    if !(1..=30).contains(&settings.lookback_days) {
        return invalid("Look-back days: use a whole number from 1 to 30.");
    }
    let label_chars = settings.ready_label.chars().count();
    if label_chars == 0 || label_chars > MAX_READY_LABEL_CHARS {
        return invalid("\"Ready for a human\" label: use 1 to 50 characters.");
    }
    check_list(ListField::SkipAuthors, &settings.skip_authors, own_login)?;
    check_list(ListField::KnownBots, &settings.known_bots, own_login)?;
    check_list(
        ListField::MemcanExtraProjects,
        &settings.memcan_extra_projects,
        own_login,
    )?;
    if !valid_time(&settings.schedule_time) {
        return invalid("Time of day: use HH:MM in steps of 15 minutes.");
    }
    if let Some(zone) = &settings.time_zone {
        check_time_zone(zone)?;
    }
    Ok(settings)
}

/// R6b: `zone` must be an IANA time zone.
pub fn check_time_zone(zone: &str) -> Result<(), InvalidSettings> {
    if known_time_zone(zone) {
        Ok(())
    } else {
        invalid(format!("Time zone: \"{zone}\" is not a known time zone."))
    }
}

/// Whether `zone` names an IANA time zone `chrono-tz` knows (the schedule resolves it with the same crate).
fn known_time_zone(zone: &str) -> bool {
    zone.parse::<chrono_tz::Tz>().is_ok()
}

fn invalid<T>(message: impl Into<String>) -> Result<T, InvalidSettings> {
    Err(InvalidSettings(message.into()))
}

/// Lists edited one entry at a time.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ListField {
    LinkedLogins,
    EnumerateOwners,
    SkipAuthors,
    KnownBots,
    MemcanExtraProjects,
}

impl ListField {
    /// Label, most entries, entry rule and its wording.
    fn rule(self) -> (&'static str, usize, fn(&str) -> bool, &'static str) {
        match self {
            Self::LinkedLogins => (
                "Linked logins",
                5,
                valid_login,
                "is not a GitHub login (letters, digits, - or _, up to 39)",
            ),
            Self::EnumerateOwners => (
                "Owners to list",
                5,
                valid_login,
                "is not a GitHub login (letters, digits, - or _, up to 39)",
            ),
            Self::SkipAuthors => (
                "Excluded authors",
                20,
                valid_bot_login,
                "is not a GitHub login (optionally ending [bot])",
            ),
            Self::KnownBots => (
                "Known bots",
                20,
                valid_bot_login,
                "is not a GitHub login (optionally ending [bot])",
            ),
            Self::MemcanExtraProjects => (
                "Extra MemCan projects",
                10,
                valid_project,
                "is not a project name (letters, digits, . _ or -, up to 100)",
            ),
        }
    }
}

/// The client's `checkList`: the count, then each entry against the entries before it.
fn check_list(field: ListField, list: &[String], own_login: &str) -> Result<(), InvalidSettings> {
    let (label, max, valid, rule) = field.rule();
    if list.len() > max {
        return invalid(format!("{label}: at most {max}."));
    }
    for (index, value) in list.iter().enumerate() {
        if !valid(value) {
            return invalid(format!("{label}: \"{value}\" {rule}."));
        }
        // Logins are case-insensitive on GitHub; project names are compared as written.
        let repeated = list[..index].iter().any(|earlier| match field {
            ListField::MemcanExtraProjects => earlier == value,
            _ => same(earlier, value),
        });
        if repeated {
            return invalid(format!("{label}: \"{value}\" is listed twice."));
        }
        if field == ListField::LinkedLogins && same(value, own_login) {
            return invalid(format!(
                "{label}: \"{value}\" is your own login, which is always scanned."
            ));
        }
    }
    Ok(())
}

/// G3–G7 (repository rows), in the client's order: count, then per row repository, short name, policy, branches.
fn check_repositories(rows: &[RepositoryRule]) -> Result<(), InvalidSettings> {
    if rows.len() > MAX_REPOSITORIES {
        return invalid(format!("Repositories: at most {MAX_REPOSITORIES}."));
    }
    // The prefix a row is shown with: its short name, else its bare repository name.
    let prefix = |row: &RepositoryRule| row.alias.clone().unwrap_or_else(|| row.repo.name.clone());
    for (i, row) in rows.iter().enumerate() {
        let label = format!("Repository {}", i + 1);
        let text = repo_text(&row.repo);
        if !valid_repo(&row.repo) {
            return invalid(format!("{label}: use owner/name."));
        }
        if rows[..i]
            .iter()
            .any(|earlier| same(&repo_text(&earlier.repo), &text))
        {
            return invalid(format!("{label}: {text} is listed twice."));
        }
        match &row.alias {
            Some(alias) if !valid_alias(alias) => {
                return invalid(format!(
                    "{label}: the short name uses capital letters, digits and -, starts with a letter or digit, \
                     up to 12."
                ));
            }
            Some(alias) => {
                let taken = rows
                    .iter()
                    .enumerate()
                    .any(|(j, other)| j != i && same(&prefix(other), alias));
                if taken {
                    return invalid(format!(
                        "{label}: the short name {alias} is used by another row."
                    ));
                }
            }
            None => {
                // Two rows falling back to the same bare name: the later one needs its own short name.
                let taken = rows[..i].iter().any(|earlier| {
                    same(&prefix(earlier), &row.repo.name)
                        && !same(&repo_text(&earlier.repo), &text)
                });
                if taken {
                    return invalid(format!(
                        "{label}: its name {} is already another row's short name; set a different short name.",
                        row.repo.name
                    ));
                }
            }
        }
        if let Some(policy) = &row.ownership_policy {
            let repo_given = !policy.repo.owner.is_empty() || !policy.repo.name.is_empty();
            if !repo_given || policy.path.is_empty() {
                let missing = if repo_given { "path" } else { "repository" };
                return invalid(format!(
                    "{label}: the policy {missing} is missing. Give the repository and the path of the policy file, or leave both empty."
                ));
            }
            if !valid_repo(&policy.repo) {
                return invalid(format!("{label}: the policy repository uses owner/name."));
            }
            if !valid_policy_path(&policy.path) {
                return invalid(format!(
                    "{label}: the policy path is relative, ends with .json and has no \"..\"."
                ));
            }
            // Server-only: the path becomes part of a GitHub contents URL.
            if policy.path.contains(['%', '\\']) {
                return invalid(format!(
                    "{label}: the policy path may not contain \"%\" or \"\\\"."
                ));
            }
        }
        if row.branch_patterns.len() > MAX_BRANCH_PATTERNS {
            return invalid(format!(
                "{label}: at most {MAX_BRANCH_PATTERNS} release branch patterns."
            ));
        }
        if let Some(bad) = row.branch_patterns.iter().find(|p| !valid_branch(p)) {
            return invalid(format!(
                "{label}: \"{bad}\" may use only letters, digits, . _ / - and *."
            ));
        }
    }
    Ok(())
}

/// Case-insensitive equality (the client's `toLowerCase` comparison).
fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

fn repo_text(repo: &RepoRef) -> String {
    format!("{}/{}", repo.owner, repo.name)
}

/// JavaScript's `String.prototype.trim`: Unicode white space except U+0085, plus U+FEFF.
fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
}

/// `^[A-Za-z0-9_-]{1,39}$`.
fn valid_login(value: &str) -> bool {
    (1..=39).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

/// `^[A-Za-z0-9_-]{1,39}(\[bot\])?$`.
fn valid_bot_login(value: &str) -> bool {
    valid_login(value.strip_suffix("[bot]").unwrap_or(value))
}

/// `^[A-Za-z0-9._-]{1,100}$` (repository and MemCan project names).
fn valid_project(value: &str) -> bool {
    (1..=100).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Owner a login, name a repository name other than `.` and `..`.
fn valid_repo(repo: &RepoRef) -> bool {
    valid_login(&repo.owner) && valid_project(&repo.name) && repo.name != "." && repo.name != ".."
}

/// `^[A-Z0-9][A-Z0-9-]{0,11}$`.
fn valid_alias(value: &str) -> bool {
    let upper_or_digit = |b: u8| b.is_ascii_uppercase() || b.is_ascii_digit();
    (1..=12).contains(&value.len())
        && value.bytes().next().is_some_and(upper_or_digit)
        && value.bytes().all(|b| upper_or_digit(b) || b == b'-')
}

/// `^[A-Za-z0-9._/*-]+$`.
fn valid_branch(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'*' | b'-'))
}

/// Relative, ending `.json`, no `..` segment once a backslash counts as a separator (the client's rule).
fn valid_policy_path(path: &str) -> bool {
    let normalised = path.replace('\\', "/");
    normalised.ends_with(".json")
        && !normalised.starts_with('/')
        && !normalised.split('/').any(|segment| segment == "..")
}

/// `^([01][0-9]|2[0-3]):(00|15|30|45)$`.
fn valid_time(value: &str) -> bool {
    let Some((hours, minutes)) = value.split_once(':') else {
        return false;
    };
    hours.len() == 2
        && hours.bytes().all(|b| b.is_ascii_digit())
        && hours < "24"
        && matches!(minutes, "00" | "15" | "30" | "45")
}

#[cfg(test)]
mod tests {
    use contracts::api::dashboard::PolicyRef;

    use super::*;

    const OWN: &str = "alice";

    fn row(repo: &str) -> RepositoryRule {
        let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
        RepositoryRule {
            repo: RepoRef {
                owner: owner.to_owned(),
                name: name.to_owned(),
            },
            alias: None,
            watch_issues: false,
            ownership_policy: None,
            branch_patterns: Vec::new(),
        }
    }

    fn aliased(repo: &str, alias: &str) -> RepositoryRule {
        RepositoryRule {
            alias: Some(alias.to_owned()),
            ..row(repo)
        }
    }

    fn with_policy(owner: &str, name: &str, path: &str) -> RepositoryRule {
        RepositoryRule {
            ownership_policy: Some(PolicyRef {
                repo: RepoRef {
                    owner: owner.to_owned(),
                    name: name.to_owned(),
                },
                path: path.to_owned(),
            }),
            ..row("acme/x")
        }
    }

    fn defaults() -> DashboardSettings {
        DashboardSettings {
            time_zone: Some("Europe/Warsaw".to_owned()),
            ..DashboardSettings::default()
        }
    }

    /// The message of the first broken rule, or `None` when the settings are valid.
    fn problem(change: impl FnOnce(&mut DashboardSettings)) -> Option<String> {
        let mut settings = defaults();
        change(&mut settings);
        validated(settings, OWN)
            .err()
            .map(|error| error.to_string())
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_owned()).collect()
    }

    #[test]
    fn absent_settings_are_defaults() {
        let defaults = DashboardSettings::default();
        assert_eq!(defaults.linked_logins, Vec::<String>::new());
        assert!(defaults.repositories.is_empty());
        assert!(defaults.enumerate_owners.is_empty());
        assert_eq!(defaults.lookback_days, 7);
        assert_eq!(defaults.ready_label, "ready-for-human");
        assert_eq!(defaults.skip_authors, ["dependabot", "dependabot[bot]"]);
        assert_eq!(
            defaults.known_bots,
            ["thepastaclaw", "Claudius-Maginificent", "DCG-Claude"]
        );
        assert!(defaults.memcan_extra_projects.is_empty());
        assert!(defaults.schedule_enabled);
        assert_eq!(defaults.schedule_time, "06:00");
        assert_eq!(defaults.time_zone, None);
        assert_eq!(validated(defaults.clone(), OWN), Ok(defaults));
    }

    #[test]
    fn todos_default_on() {
        assert!(DashboardSettings::default().include_todos);
        let shown = view(DashboardSettings::default(), OWN.to_owned(), true, &[]);
        assert!(shown.settings.include_todos);
        assert_eq!(shown.defaults, DashboardSettings::default());
    }

    #[test]
    fn a_fully_filled_valid_form_is_accepted() {
        let mut platform = aliased("acme/platform", "PLAT");
        platform.ownership_policy =
            with_policy("acme", "policies", "policies/platform.json").ownership_policy;
        platform.branch_patterns = strings(&["release/*", "vX.Y-dev", "main"]);
        assert_eq!(
            problem(|s| {
                s.linked_logins = strings(&["alice-ci", "Corp_EMU"]);
                s.repositories = vec![platform, row("acme/.github")];
                s.enumerate_owners = strings(&["acme"]);
                s.lookback_days = 30;
                s.skip_authors = strings(&["renovate[bot]"]);
                s.memcan_extra_projects = strings(&["release-notes", "a.b_c"]);
                s.schedule_time = "23:45".to_owned();
            }),
            None
        );
    }

    #[test]
    fn settings_validation_names_the_field() {
        for (change, message) in [
            (
                Box::new(|s: &mut DashboardSettings| s.linked_logins = strings(&["x y"]))
                    as Box<dyn FnOnce(&mut DashboardSettings)>,
                "Linked logins: \"x y\" is not a GitHub login (letters, digits, - or _, up to 39).",
            ),
            (
                Box::new(|s| s.repositories = vec![row("bad")]),
                "Repository 1: use owner/name.",
            ),
            (
                Box::new(|s| s.enumerate_owners = strings(&["a/b"])),
                "Owners to list: \"a/b\" is not a GitHub login (letters, digits, - or _, up to 39).",
            ),
            (
                Box::new(|s| s.lookback_days = 0),
                "Look-back days: use a whole number from 1 to 30.",
            ),
            (
                Box::new(|s| s.ready_label = " ".to_owned()),
                "\"Ready for a human\" label: use 1 to 50 characters.",
            ),
            (
                Box::new(|s| s.skip_authors = strings(&["bot[robot]"])),
                "Excluded authors: \"bot[robot]\" is not a GitHub login (optionally ending [bot]).",
            ),
            (
                Box::new(|s| s.known_bots = strings(&["two words"])),
                "Known bots: \"two words\" is not a GitHub login (optionally ending [bot]).",
            ),
            (
                Box::new(|s| s.memcan_extra_projects = strings(&["has space"])),
                "Extra MemCan projects: \"has space\" is not a project name (letters, digits, . _ or -, up to 100).",
            ),
            (
                Box::new(|s| s.schedule_time = "06:10".to_owned()),
                "Time of day: use HH:MM in steps of 15 minutes.",
            ),
            (
                Box::new(|s| s.time_zone = Some("Mars/Olympus Mons".to_owned())),
                "Time zone: \"Mars/Olympus Mons\" is not a known time zone.",
            ),
        ] {
            assert_eq!(problem(change).as_deref(), Some(message));
        }
    }

    #[test]
    fn the_first_broken_rule_in_form_order_is_reported() {
        assert_eq!(
            problem(|s| {
                s.linked_logins = strings(&["x y"]);
                s.repositories = vec![row("bad")];
                s.lookback_days = 0;
                s.ready_label = String::new();
                s.schedule_time = "x".to_owned();
            })
            .as_deref(),
            Some(
                "Linked logins: \"x y\" is not a GitHub login (letters, digits, - or _, up to 39)."
            )
        );
        assert_eq!(
            problem(|s| {
                s.repositories = vec![row("acme/x"), row("bad")];
                s.lookback_days = 0;
            })
            .as_deref(),
            Some("Repository 2: use owner/name.")
        );
    }

    #[test]
    fn g2_linked_logins_syntax_count_unique_not_own() {
        assert_eq!(
            problem(|s| s.linked_logins = strings(&["a", "b", "c", "d", "e", "f"])).as_deref(),
            Some("Linked logins: at most 5.")
        );
        assert_eq!(
            problem(|s| s.linked_logins = strings(&["bob", "Bob"])).as_deref(),
            Some("Linked logins: \"Bob\" is listed twice.")
        );
        assert_eq!(
            problem(|s| s.linked_logins = strings(&["Alice"])).as_deref(),
            Some("Linked logins: \"Alice\" is your own login, which is always scanned.")
        );
        assert_eq!(
            problem(|s| s.linked_logins = strings(&["a".repeat(40).as_str()]))
                .map(|m| m.contains("not a GitHub login")),
            Some(true)
        );
        assert_eq!(
            problem(|s| s.linked_logins = strings(&["a".repeat(39).as_str()])),
            None
        );
    }

    #[test]
    fn g3_repositories_syntax_unique_at_most_50() {
        for bad in [
            "acme", "acme/..", "acme/.", "acme/a b", "ac me/x", "/x", "acme/é",
        ] {
            assert_eq!(
                problem(|s| s.repositories = vec![row(bad)]).as_deref(),
                Some("Repository 1: use owner/name."),
                "{bad}"
            );
        }
        assert_eq!(
            problem(|s| s.repositories = vec![aliased("acme/x", "A"), aliased("ACME/X", "B")])
                .as_deref(),
            Some("Repository 2: ACME/X is listed twice.")
        );
        let many: Vec<RepositoryRule> = (0..51).map(|i| row(&format!("acme/r{i}"))).collect();
        assert_eq!(
            problem(|s| s.repositories = many).as_deref(),
            Some("Repositories: at most 50.")
        );
        let fifty: Vec<RepositoryRule> = (0..50).map(|i| row(&format!("acme/r{i}"))).collect();
        assert_eq!(problem(|s| s.repositories = fifty), None);
    }

    #[test]
    fn g4_short_names_pattern_and_unique_against_short_and_bare_names() {
        let short_rule = "Repository 1: the short name uses capital letters, digits and -, starts with a letter or digit, up to 12.";
        for bad in ["plat", "-X", "ABCDEFGHIJKLM", "", "PL AT", "PLÄT"] {
            assert_eq!(
                problem(|s| s.repositories = vec![aliased("acme/x", bad)]).as_deref(),
                Some(short_rule),
                "{bad}"
            );
        }
        assert_eq!(
            problem(|s| s.repositories = vec![aliased("acme/x", "ABCDEFGHIJKL")]),
            None
        );
        assert_eq!(
            problem(|s| s.repositories = vec![aliased("acme/x", "9-A")]),
            None
        );
        assert_eq!(
            problem(|s| s.repositories = vec![aliased("acme/x", "P"), aliased("acme/y", "P")])
                .as_deref(),
            Some("Repository 1: the short name P is used by another row."),
            "both rows clash; the first is reported"
        );
        // Row 2 without a short name is shown as "core"; another row may not take that name, in any case.
        assert_eq!(
            problem(|s| s.repositories = vec![aliased("acme/x", "CORE"), row("acme/core")])
                .as_deref(),
            Some("Repository 1: the short name CORE is used by another row.")
        );
        assert_eq!(
            problem(|s| s.repositories = vec![row("acme/core"), aliased("acme/x", "CORE")])
                .as_deref(),
            Some("Repository 2: the short name CORE is used by another row.")
        );
    }

    #[test]
    fn g4_two_rows_with_the_same_bare_name_and_no_short_name_clash() {
        assert_eq!(
            problem(|s| s.repositories = vec![row("acme/platform"), row("fork/Platform")])
                .as_deref(),
            Some(
                "Repository 2: its name Platform is already another row's short name; set a different short name."
            )
        );
        assert_eq!(
            problem(
                |s| s.repositories = vec![row("acme/platform"), aliased("fork/platform", "FORK")]
            ),
            None
        );
        assert_eq!(
            problem(|s| s.repositories = vec![aliased("acme/x", "PLATFORM"), row("acme/platform")])
                .as_deref(),
            Some("Repository 1: the short name PLATFORM is used by another row.")
        );
    }

    #[test]
    fn g6_ownership_policy_both_or_neither_repository_and_safe_relative_path() {
        let policy = |owner: &str, name: &str, path: &str| {
            let rule = with_policy(owner, name, path);
            problem(|s| s.repositories = vec![rule])
        };
        // The client's wording (validation.ts), so both sides name the missing half alike.
        assert_eq!(
            policy("acme", "p", "").as_deref(),
            Some(
                "Repository 1: the policy path is missing. Give the repository and the path of the policy file, or leave both empty."
            )
        );
        assert_eq!(
            policy("", "", "a.json").as_deref(),
            Some(
                "Repository 1: the policy repository is missing. Give the repository and the path of the policy file, or leave both empty."
            )
        );
        assert_eq!(
            policy("acme", "", "a.json").as_deref(),
            Some("Repository 1: the policy repository uses owner/name.")
        );
        let path_rule = Some(
            "Repository 1: the policy path is relative, ends with .json and has no \"..\"."
                .to_owned(),
        );
        for bad in [
            "/abs.json",
            "a/../b.json",
            "a\\..\\b.json",
            "../b.json",
            "a.yaml",
            "..\\a.json",
        ] {
            assert_eq!(policy("acme", "p", bad), path_rule, "{bad}");
        }
        let characters =
            Some("Repository 1: the policy path may not contain \"%\" or \"\\\".".to_owned());
        for bad in ["a%2e%2e/b.json", "dir\\a.json", "%2F.json"] {
            assert_eq!(policy("acme", "p", bad), characters, "{bad}");
        }
        assert_eq!(policy("acme", "p", "a..b.json"), None);
        assert_eq!(policy("acme", "p", "dir/a.json"), None);
    }

    #[test]
    fn g7_release_branches_at_most_10_of_the_allowed_characters() {
        let patterns = |values: &[&str]| {
            let rule = RepositoryRule {
                branch_patterns: strings(values),
                ..row("acme/x")
            };
            problem(|s| s.repositories = vec![rule])
        };
        assert_eq!(patterns(&["release/*", "vX.Y", "hotfix/1.2_a-b"]), None);
        assert_eq!(
            patterns(&["rel ease"]).as_deref(),
            Some("Repository 1: \"rel ease\" may use only letters, digits, . _ / - and *.")
        );
        assert!(patterns(&["main?"]).is_some());
        assert!(patterns(&[""]).is_some());
        let eleven: Vec<String> = (0..11).map(|i| format!("b{i}")).collect();
        let eleven: Vec<&str> = eleven.iter().map(String::as_str).collect();
        assert_eq!(
            patterns(&eleven).as_deref(),
            Some("Repository 1: at most 10 release branch patterns.")
        );
    }

    #[test]
    fn g8_owners_and_g11_bot_authors_syntax_count_unique() {
        assert_eq!(
            problem(|s| s.enumerate_owners = strings(&["a", "b", "c", "d", "e", "f"])).as_deref(),
            Some("Owners to list: at most 5.")
        );
        assert_eq!(
            problem(|s| s.enumerate_owners = strings(&["acme", "ACME"])).as_deref(),
            Some("Owners to list: \"ACME\" is listed twice.")
        );
        assert_eq!(
            problem(|s| s.enumerate_owners = strings(&["alice"])),
            None,
            "own login allowed"
        );
        let many: Vec<String> = (0..21).map(|i| format!("b{i}")).collect();
        assert_eq!(
            problem(|s| s.skip_authors = many).as_deref(),
            Some("Excluded authors: at most 20.")
        );
        assert_eq!(
            problem(|s| s.skip_authors = strings(&["renovate[bot]", "Renovate[bot]"])).as_deref(),
            Some("Excluded authors: \"Renovate[bot]\" is listed twice.")
        );
        assert_eq!(
            problem(|s| s.skip_authors = strings(&["[bot]"]))
                .map(|m| m.contains("not a GitHub login")),
            Some(true)
        );
        let bots: Vec<String> = (0..21).map(|i| format!("b{i}")).collect();
        assert_eq!(
            problem(|s| s.known_bots = bots).as_deref(),
            Some("Known bots: at most 20.")
        );
        assert_eq!(
            problem(|s| s.known_bots = strings(&["thepastaclaw", "ThePastaClaw"])).as_deref(),
            Some("Known bots: \"ThePastaClaw\" is listed twice.")
        );
        assert_eq!(
            problem(|s| s.known_bots = strings(&["alice", "alice-ci"])),
            None,
            "a linked or own login may also be a known bot"
        );
    }

    #[test]
    fn g9_g10_look_back_and_ready_label_counted_in_characters_and_trimmed() {
        assert!(problem(|s| s.lookback_days = 31).is_some());
        assert_eq!(problem(|s| s.lookback_days = 1), None);
        assert!(problem(|s| s.ready_label = "x".repeat(51)).is_some());
        assert_eq!(problem(|s| s.ready_label = "x".repeat(50)), None);
        assert_eq!(
            problem(|s| s.ready_label = "😀".repeat(50)),
            None,
            "characters, not bytes"
        );
        assert!(
            problem(|s| s.ready_label = "\u{feff}\u{a0}\t".to_owned()).is_some(),
            "JavaScript's trim"
        );
        let mut padded = defaults();
        padded.ready_label = " ready \n".to_owned();
        assert_eq!(
            validated(padded, OWN).map(|s| s.ready_label),
            Ok("ready".to_owned())
        );
        let mut nel = defaults();
        nel.ready_label = "\u{85}".to_owned();
        assert_eq!(
            validated(nel, OWN).map(|s| s.ready_label),
            Ok("\u{85}".to_owned()),
            "U+0085 is not white space to JavaScript"
        );
    }

    #[test]
    fn g17_extra_memcan_projects_syntax_unique_as_written_at_most_10() {
        assert!(
            problem(|s| s.memcan_extra_projects = strings(&["x".repeat(101).as_str()])).is_some()
        );
        assert_eq!(
            problem(|s| s.memcan_extra_projects = strings(&["a", "a"])).as_deref(),
            Some("Extra MemCan projects: \"a\" is listed twice.")
        );
        assert_eq!(
            problem(|s| s.memcan_extra_projects = strings(&["a", "A"])),
            None
        );
        let eleven: Vec<String> = (0..11).map(|i| format!("p{i}")).collect();
        assert_eq!(
            problem(|s| s.memcan_extra_projects = eleven).as_deref(),
            Some("Extra MemCan projects: at most 10.")
        );
    }

    #[test]
    fn g15_time_of_day_in_15_minute_steps() {
        for good in ["00:00", "06:00", "19:15", "23:45"] {
            assert_eq!(
                problem(|s| s.schedule_time = good.to_owned()),
                None,
                "{good}"
            );
        }
        for bad in [
            "06:10", "24:00", "6:00", "06:00 ", "ab:00", "0600", "+6:00", "06:60",
        ] {
            assert!(
                problem(|s| s.schedule_time = bad.to_owned()).is_some(),
                "{bad}"
            );
        }
    }

    #[test]
    fn g16_time_zone_names_and_no_zone_yet() {
        for good in [
            "Europe/Warsaw",
            "America/Argentina/Buenos_Aires",
            "UTC",
            "Etc/GMT+5",
            "America/Port-au-Prince",
        ] {
            assert_eq!(check_time_zone(good), Ok(()), "{good}");
        }
        for bad in [
            "",
            "europe/warsaw",
            "Europe//Warsaw",
            "../etc/passwd",
            "Europe/Warsaw ",
            "/UTC",
        ] {
            assert!(check_time_zone(bad).is_err(), "{bad}");
        }
        assert_eq!(problem(|s| s.time_zone = None), None);
    }

    #[test]
    fn unknown_iana_zone_is_refused() {
        assert!(check_time_zone("Etc/Unknown").is_err());
        assert!(check_time_zone("Mars/Olympus").is_err());
    }

    #[test]
    fn memcan_projects_default_to_repository_short_names() {
        let mut settings = DashboardSettings::default();
        assert!(memcan_projects(&settings).is_empty());
        settings.repositories = vec![
            aliased("acme/platform", "PLAT"),
            row("acme/wallet-ui"),
            row("fork/platform"),
        ];
        assert_eq!(memcan_projects(&settings), ["platform", "wallet-ui"]);
        settings.memcan_extra_projects = strings(&["release-notes", "wallet-ui", "Platform"]);
        assert_eq!(
            memcan_projects(&settings),
            ["platform", "wallet-ui", "release-notes", "Platform"],
            "repeats dropped as written"
        );
    }

    #[test]
    fn unreachable_marks_only_name_current_rows_and_owners() {
        let settings = DashboardSettings {
            repositories: vec![row("acme/legacy-tools"), row("acme/platform")],
            enumerate_owners: strings(&["Acme-Labs"]),
            ..DashboardSettings::default()
        };
        let reported = strings(&["acme/Legacy-Tools", "acme/removed", "acme-labs", "other"]);
        assert_eq!(
            unreachable_marks(&settings, &reported),
            ["acme/Legacy-Tools", "acme-labs"]
        );
        let shown = view(settings, OWN.to_owned(), false, &reported);
        assert_eq!(shown.unreachable, ["acme/Legacy-Tools", "acme-labs"]);
        assert_eq!(shown.memcan_projects, ["legacy-tools", "platform"]);
        assert!(!shown.memcan_configured);
        assert_eq!(shown.own_login, OWN);
    }
}
