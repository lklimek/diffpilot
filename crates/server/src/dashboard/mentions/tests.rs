use super::*;
use contracts::api::dashboard::DashboardSettings;

fn post(author: &str, body: &str, hour: u8, bot: bool) -> Comment {
    Comment {
        author: Some(Actor {
            login: Some(author.into()),
            typename: if bot { "Bot" } else { "User" }.into(),
        }),
        body: body.into(),
        created_at: format!("2026-11-18T{hour:02}:00:00Z").parse().unwrap(),
        url: format!("https://github.com/acme/app/pull/1#issuecomment-{hour}"),
    }
}
fn collect(posts: &[Comment]) -> Vec<Mention> {
    let settings = DashboardSettings {
        linked_logins: vec!["alice-helper".into()],
        known_bots: vec!["known".into()],
        ..Default::default()
    };
    unanswered(
        &ScoreRules::new("alice", &settings),
        &["alice".into(), "alice-helper".into()],
        &ItemRef::new("acme", "app", 1).unwrap(),
        GithubKind::Pr,
        "Title",
        posts,
    )
}
#[test]
fn replies_by_any_scanned_login_clear_older_mentions_only() {
    let posts = [
        post("bob", "@alice earlier", 1, false),
        post("ALICE-helper", "answered", 2, false),
        post("bob", "@ALICE new", 3, false),
    ];
    let found = collect(&posts);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].created_at, posts[2].created_at);
    assert_ne!(collect(&posts[..1])[0].id, found[0].id);
}
#[test]
fn bot_rules_plain_text_and_mention_boundaries() {
    let posts = [
        post("bob", "@alice <script>\nhello", 1, false),
        post("service", "@alice", 2, true),
        post("helper[bot]", "@alice", 3, false),
        post("KNOWN", "@alice", 4, false),
        post("bob", "mail@alice or @alice-other", 5, false),
    ];
    let found = collect(&posts);
    assert_eq!(
        found.iter().map(|m| m.is_bot).collect::<Vec<_>>(),
        [false, true, true, true]
    );
    assert_eq!(found[0].text, "@alice <script>");
    assert!(found[0].url.ends_with("#issuecomment-1"));
}
#[test]
fn self_mentions_and_untrusted_links_never_surface() {
    assert!(collect(&[post("alice", "@alice", 1, false)]).is_empty());
    let mut bad = post("bob", "@alice", 1, false);
    bad.url = "javascript:alert(1)".into();
    assert!(collect(&[bad]).is_empty());
}

#[test]
fn description_is_a_mention_source_but_not_an_answer() {
    let mut description = post("bob", "@alice please look", 1, false);
    description.url = "#body".into();
    let found = collect(&[description.clone()]);
    assert_eq!(found[0].url, "https://github.com/acme/app/pull/1");
    assert!(collect(&[description, post("alice", "answered", 2, false)]).is_empty());
}
