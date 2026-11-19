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

/// The other RFC 3339 spellings read as the same instant; a leap second reads as the second before it.
#[test]
fn rfc3339_variants_read_and_serialise_in_the_wire_form() {
    for (input, wire) in [
        ("2026-11-18 21:00:01Z", "2026-11-18T21:00:01Z"),
        ("2026-11-18t21:00:01.5z", "2026-11-18T21:00:01.500Z"),
        ("2026-12-31T23:59:60Z", "2026-12-31T23:59:59Z"),
        ("2026-11-19T06:30:01+09:30", "2026-11-18T21:00:01Z"),
    ] {
        let read: AccessToken = serde_json::from_value(token(input, Value::Null)).expect("reads");
        assert_eq!(
            serde_json::to_value(&read).expect("serialises")["created_at"],
            wire,
            "{input}"
        );
    }
}

/// Input is strict RFC 3339: forms jiff alone would read (RFC 9557 annotations, ISO 8601 basic format, missing
/// seconds, short or sub-minute offsets, comma fractions) and chrono's display form are refused.
#[test]
fn timestamps_without_an_offset_or_not_rfc3339_are_rejected() {
    for input in [
        "2026-11-18T21:00:01",
        "2026-11-18",
        "yesterday",
        "",
        "2026-11-18T21:00:01Z[Europe/Warsaw]",
        "2026-11-18T21:00:01Z[u-ca=iso8601]",
        "20261118T210001Z",
        "2026-11-18T21:00Z",
        "2026-11-18T21:00:01+02",
        "2026-11-18T21:00:01+0200",
        "2026-11-18T21:00:01+00:00:30",
        "2026-11-18T21:00:01+24:00",
        "2026-11-18T21:00:01,5Z",
        "2026-11-18T21:00:01.Z",
        "2026-11-18T21:00:01.1234567891Z",
        "2026-11-18 21:00:01 UTC",
        "2026-11-18T21:00:01UTC",
        " 2026-11-18T21:00:01Z",
        "2026-11-18T21:00:01Z ",
        "2026-1-8T01:02:03Z",
        "+2026-11-18T21:00:01Z",
        "2026-11-31T21:00:01Z",
        "2026-11-18T25:00:01Z",
    ] {
        assert!(
            serde_json::from_value::<AccessToken>(token(input, Value::Null)).is_err(),
            "{input:?} must not read"
        );
    }
}
