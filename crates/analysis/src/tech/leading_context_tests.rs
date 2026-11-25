use super::*;

fn context(path: &str, text: &str, first_line: u32) -> Option<LineRange> {
    plugin_for(path).leading_context(
        &FileSource {
            path,
            side: LineSide::Head,
            text: Some(text),
            other: OtherSide::Unknown,
            changed_lines: &[],
            hunks: &[],
        },
        first_line,
    )
}

fn range(start_line: u32, end_line: u32) -> Option<LineRange> {
    Some(LineRange {
        start_line,
        end_line,
    })
}

#[test]
fn rust_leading_context_doc_comments_and_attributes() {
    let text = concat!(
        "//! Module docs\n",
        "/// Item docs\n",
        "/** Block with /* nested */ comment\n",
        " * and a second line */\n",
        "#[cfg_attr(\n",
        "    feature = \"foo\", doc = r#\"]\"#\n",
        ")]\n",
        "#[inline]\n",
        "pub(crate) async unsafe fn item() {}\n",
    );
    assert_eq!(context("src/lib.rs", text, 9), range(1, 8));
}

#[test]
fn leading_context_only_when_step_starts_an_item() {
    for item in [
        "fn f() {}",
        "pub struct S;",
        "enum E {}",
        "unsafe trait T {}",
        "impl T for S {}",
        "pub(super) mod m;",
        "const N: i32 = 0;",
        "static X: i32 = 0;",
        "type A = u8;",
        "macro_rules! m { () => {}; }",
        "pub(in crate::m) const unsafe fn f() {}",
    ] {
        assert_eq!(
            context("lib.rs", &format!("/// Doc\n{item}"), 2),
            range(1, 1),
            "{item}"
        );
    }
    for line in [
        "let x = 0;",
        "fn(u8)",
        "const { 0 }",
        "unsafe { work() }",
        "structural();",
        "#[inline]\nlet x = 0;",
        "#![allow(dead_code)]\nfn f() {}",
    ] {
        assert_eq!(
            context("lib.rs", &format!("/// Doc\n{line}"), 2),
            None,
            "{line}"
        );
    }
    let string = "const S: &str = r#\"\n/// Fake docs\nfn fake() {}\n\"#;";
    assert_eq!(context("lib.rs", string, 3), None);
    assert_eq!(context("lib.rs", "fn f() {}", 0), None);
    assert_eq!(context("lib.rs", "fn f() {}", 2), None);
}

#[test]
fn leading_context_when_step_starts_at_an_items_outer_attribute() {
    let text = "/// Doc\n#[derive(Debug)]\npub struct S;";
    assert_eq!(context("lib.rs", text, 2), range(1, 1));
    let text = concat!(
        "/// Doc\n",
        "#[cfg_attr(\n",
        "    feature = \"foo\", doc = r#\"]\"#\n",
        ")]\n",
        "/// Test docs\n",
        "#[tokio::test]\n",
        "async fn test() {}\n",
    );
    assert_eq!(context("lib.rs", text, 2), range(1, 1));
    assert_eq!(context("lib.rs", text, 6), range(1, 5));
    let inline = "/// Doc\n#[inline]\nfn f() {}";
    assert_eq!(context("lib.rs", inline, 2), range(1, 1));
}

#[test]
fn leading_context_recognises_extern_functions() {
    for item in [
        r#"extern "C" fn f() {}"#,
        r#"pub unsafe extern "C" fn f() {}"#,
        r#"extern "system" fn f() {}"#,
        r#"const extern "C" fn f() {}"#,
        "extern fn f() {}",
    ] {
        assert_eq!(
            context("lib.rs", &format!("/// Doc\n{item}"), 2),
            range(1, 1),
            "{item}"
        );
    }
    for line in [r#"unsafe extern "C" {}"#, r#"extern "C" fn(u8)"#] {
        assert_eq!(
            context("lib.rs", &format!("/// Doc\n{line}"), 2),
            None,
            "{line}"
        );
    }
}

#[test]
fn blank_or_code_line_ends_leading_context() {
    for barrier in [
        "",
        "fn prev() {}",
        "// ordinary comment",
        "//// ordinary comment",
        "/* ordinary block */",
        "fn prev() {} /** docs */",
    ] {
        let text = format!("/// Previous doc\n{barrier}\n#[inline]\nfn f() {{}}");
        assert_eq!(context("lib.rs", &text, 4), range(3, 3), "{barrier}");
    }
    let text = "/// Older\n/** Line one\n * Line two */ fn prev() {}\nfn f() {}";
    assert_eq!(context("lib.rs", text, 4), None);
    let text = "fn prev() {} /** Line one\n * Line two */\nfn f() {}";
    assert_eq!(context("lib.rs", text, 3), range(2, 2));
}

#[test]
fn leading_context_keeps_30_nearest_lines() {
    let text = format!("/** Doc\n{}*/\nfn f() {{}}", " * line\n".repeat(32));
    assert_eq!(context("lib.rs", &text, 35), range(5, 34));
}

#[test]
fn generic_leading_context_none() {
    assert_eq!(context("lib.txt", "/// Doc\nfn f() {}", 2), None);
}
