//! Wire form of timestamps: values written by the running stack read back and serialise to the same text.

use contracts::api::access_tokens::AccessToken;
use serde_json::{Value, json};

/// (stored or received text, text serialised after reading it back); captured from the chrono-based stack.
const CASES: [(&str, &str); 14] = [
    ("2026-11-18T21:00:01Z", "2026-11-18T21:00:01Z"),
    ("2026-11-18T21:00:01.000000Z", "2026-11-18T21:00:01Z"),
    ("2026-11-18T21:00:01.120Z", "2026-11-18T21:00:01.120Z"),
    ("2026-11-18T21:00:01.120000Z", "2026-11-18T21:00:01.120Z"),
    ("2026-11-18T21:00:01.5Z", "2026-11-18T21:00:01.500Z"),
    ("2026-11-18T21:00:01.123456Z", "2026-11-18T21:00:01.123456Z"),
    ("2026-11-18T21:00:01.123450Z", "2026-11-18T21:00:01.123450Z"),
    (
        "2026-11-18T21:00:01.123456789Z",
        "2026-11-18T21:00:01.123456789Z",
    ),
    (
        "2026-11-18T21:00:01.123456780Z",
        "2026-11-18T21:00:01.123456780Z",
    ),
    (
        "2026-11-18T21:00:01.000000001Z",
        "2026-11-18T21:00:01.000000001Z",
    ),
    (
        "2026-11-18T23:00:01.123456+02:00",
        "2026-11-18T21:00:01.123456Z",
    ),
    (
        "2026-11-18T21:00:01.123456789+00:00",
        "2026-11-18T21:00:01.123456789Z",
    ),
    ("2026-11-18T16:00:01-05:00", "2026-11-18T21:00:01Z"),
    ("1999-01-01T00:00:00Z", "1999-01-01T00:00:00Z"),
];

fn token(created_at: &str, last_used_at: Value) -> Value {
    json!({
        "id": "abcdefgh",
        "name": "agent",
        "prefix": "srp_abcdefgh",
        "scopes": ["read"],
        "created_at": created_at,
        "expires_at": "2027-02-16T10:00:00Z",
        "last_used_at": last_used_at,
    })
}

#[test]
fn timestamps_serialise_in_utc_with_the_fewest_of_0_3_6_or_9_fraction_digits() {
    for (input, wire) in CASES {
        let read: AccessToken =
            serde_json::from_value(token(input, json!(input))).expect("reads back");
        let written = serde_json::to_value(&read).expect("serialises");
        assert_eq!(written["created_at"], wire, "required field, from {input}");
        assert_eq!(
            written["last_used_at"], wire,
            "optional field, from {input}"
        );
        let again: AccessToken = serde_json::from_value(written).expect("own output reads back");
        assert_eq!(again, read, "{input}");
    }
}

#[test]
fn a_missing_optional_timestamp_stays_null() {
    let read: AccessToken =
        serde_json::from_value(token("2026-11-18T21:00:01Z", Value::Null)).expect("reads");
    assert_eq!(read.last_used_at, None);
    assert_eq!(
        serde_json::to_value(&read).expect("serialises")["last_used_at"],
        Value::Null
    );
}

#[test]
fn timestamps_without_an_offset_or_not_rfc3339_are_rejected() {
    for input in ["2026-11-18T21:00:01", "2026-11-18", "yesterday", ""] {
        assert!(
            serde_json::from_value::<AccessToken>(token(input, Value::Null)).is_err(),
            "{input:?} must not read"
        );
    }
}
