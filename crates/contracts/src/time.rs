//! UTC instants (jiff) with the stack's wire form: RFC 3339 with `Z` and the fewest of 0, 3, 6 or 9 fraction
//! digits that keep the value exact (ADR-020 addendum). jiff's own serde form drops trailing zeros, so wire and
//! stored documents use [`Timestamp`] instead.

use std::borrow::Cow;
use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::str::FromStr;

use jiff::tz::{Offset, TimeZone};
use jiff::{SignedDuration, Zoned};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// A UTC instant; serialises as RFC 3339 text with `Z` (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(jiff::Timestamp);

impl Timestamp {
    /// 1970-01-01T00:00:00Z.
    pub const UNIX_EPOCH: Self = Self(jiff::Timestamp::UNIX_EPOCH);

    /// The current system time.
    pub fn now() -> Self {
        Self(jiff::Timestamp::now())
    }

    /// The instant `second` whole seconds after the Unix epoch; `None` outside jiff's range.
    pub fn from_second(second: i64) -> Option<Self> {
        jiff::Timestamp::from_second(second).ok().map(Self)
    }

    /// Whole seconds since the Unix epoch, rounded towards zero.
    pub fn as_second(self) -> i64 {
        self.0.as_second()
    }

    /// The jiff instant.
    pub fn get(self) -> jiff::Timestamp {
        self.0
    }

    /// This instant in time zone `zone`.
    pub fn to_zoned(self, zone: TimeZone) -> Zoned {
        self.0.to_zoned(zone)
    }

    /// Signed time from `earlier` to `self`.
    pub fn duration_since(self, earlier: Self) -> SignedDuration {
        self.0.duration_since(earlier.0)
    }

    /// `self + duration`; `None` outside jiff's range.
    pub fn checked_add(self, duration: SignedDuration) -> Option<Self> {
        self.0.checked_add(duration).ok().map(Self)
    }

    /// RFC 3339 text with `+00:00` instead of `Z` (same fraction digits as the wire form).
    pub fn to_rfc3339_offset(self) -> String {
        format!(
            "{:.*}",
            self.fraction_digits(),
            self.0.display_with_offset(Offset::UTC)
        )
    }

    /// The fewest of 0, 3, 6 or 9 fraction digits that keep the value exact.
    fn fraction_digits(self) -> usize {
        match self.0.subsec_nanosecond() {
            0 => 0,
            nanos if nanos % 1_000_000 == 0 => 3,
            nanos if nanos % 1_000 == 0 => 6,
            _ => 9,
        }
    }
}

impl From<jiff::Timestamp> for Timestamp {
    fn from(at: jiff::Timestamp) -> Self {
        Self(at)
    }
}

impl From<Timestamp> for jiff::Timestamp {
    fn from(at: Timestamp) -> Self {
        at.0
    }
}

/// The wire form.
impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.*}", self.fraction_digits(), self.0)
    }
}

/// Why a text is not a [`Timestamp`].
#[derive(Debug, thiserror::Error)]
pub enum ParseTimestampError {
    /// Not RFC 3339 `date-time` text.
    #[error("not an RFC 3339 date-time (YYYY-MM-DDThh:mm:ss, optional fraction, Z or ±hh:mm)")]
    NotRfc3339,
    /// RFC 3339 text naming no valid instant (a field out of range).
    #[error("not a valid RFC 3339 date-time")]
    Invalid(#[source] jiff::Error),
}

/// Strict RFC 3339 `date-time` (§5.6): `T`, `t` or a space between date and time, seconds required, 1 to 9
/// fraction digits, `Z`, `z` or `±hh:mm` (hours 00–23), converted to UTC. A leap second (`:60`) reads as `:59`.
/// Rejected although jiff reads them: the basic format, missing seconds, `±hh` and sub-minute offsets, comma
/// fractions and RFC 9557 `[…]` annotations.
impl FromStr for Timestamp {
    type Err = ParseTimestampError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if !rfc3339_shape(text.as_bytes()) {
            return Err(ParseTimestampError::NotRfc3339);
        }
        text.parse().map(Self).map_err(ParseTimestampError::Invalid)
    }
}

/// Whether `text` has the shape of an RFC 3339 `date-time`; field ranges are left to jiff.
fn rfc3339_shape(text: &[u8]) -> bool {
    let [
        y1,
        y2,
        y3,
        y4,
        b'-',
        mo1,
        mo2,
        b'-',
        d1,
        d2,
        b'T' | b't' | b' ',
        h1,
        h2,
        b':',
        mi1,
        mi2,
        b':',
        s1,
        s2,
        rest @ ..,
    ] = text
    else {
        return false;
    };
    let digits = |field: &[&u8]| field.iter().all(|digit| digit.is_ascii_digit());
    if !digits(&[y1, y2, y3, y4, mo1, mo2, d1, d2, h1, h2, mi1, mi2, s1, s2]) {
        return false;
    }
    let offset = match rest {
        [b'.', fraction @ ..] => {
            let count = fraction
                .iter()
                .take_while(|digit| digit.is_ascii_digit())
                .count();
            if !(1..=9).contains(&count) {
                return false;
            }
            fraction.get(count..).unwrap_or_default()
        }
        _ => rest,
    };
    match offset {
        [b'Z' | b'z'] => true,
        [b'+' | b'-', oh1, oh2, b':', om1, om2] => {
            digits(&[oh1, oh2, om1, om2]) && [*oh1, *oh2] <= *b"23" && *om1 <= b'5'
        }
        _ => false,
    }
}

impl Add<SignedDuration> for Timestamp {
    type Output = Self;

    /// Panics outside jiff's range, like `jiff::Timestamp + SignedDuration`.
    fn add(self, duration: SignedDuration) -> Self {
        Self(self.0 + duration)
    }
}

impl AddAssign<SignedDuration> for Timestamp {
    fn add_assign(&mut self, duration: SignedDuration) {
        *self = *self + duration;
    }
}

impl Sub<SignedDuration> for Timestamp {
    type Output = Self;

    /// Panics outside jiff's range, like `jiff::Timestamp - SignedDuration`.
    fn sub(self, duration: SignedDuration) -> Self {
        Self(self.0 - duration)
    }
}

impl SubAssign<SignedDuration> for Timestamp {
    fn sub_assign(&mut self, duration: SignedDuration) {
        *self = *self - duration;
    }
}

impl Sub for Timestamp {
    type Output = SignedDuration;

    /// Signed time from `earlier` to `self` ([`Timestamp::duration_since`]).
    fn sub(self, earlier: Self) -> SignedDuration {
        self.duration_since(earlier)
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl de::Visitor<'_> for Visitor {
            type Value = Timestamp;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an RFC 3339 formatted date and time string")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Timestamp, E> {
                text.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

impl JsonSchema for Timestamp {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        jiff::Timestamp::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        jiff::Timestamp::schema_id()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        jiff::Timestamp::json_schema(generator)
    }
}

impl ts_rs::TS for Timestamp {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;

    fn ident(_: &ts_rs::Config) -> String {
        "string".to_owned()
    }

    fn name(_: &ts_rs::Config) -> String {
        "string".to_owned()
    }

    fn inline(_: &ts_rs::Config) -> String {
        "string".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().expect("timestamp")
    }

    #[test]
    fn display_keeps_the_fewest_of_0_3_6_or_9_fraction_digits() {
        for (input, shown) in [
            ("1970-01-01T00:00:00Z", "1970-01-01T00:00:00Z"),
            ("2026-11-18T21:00:01.100Z", "2026-11-18T21:00:01.100Z"),
            ("2026-11-18T21:00:01.000100Z", "2026-11-18T21:00:01.000100Z"),
            (
                "2026-11-18T21:00:01.000000100Z",
                "2026-11-18T21:00:01.000000100Z",
            ),
            ("1969-12-31T23:59:59.5Z", "1969-12-31T23:59:59.500Z"),
            ("1969-12-31T23:59:59.000001Z", "1969-12-31T23:59:59.000001Z"),
        ] {
            assert_eq!(at(input).to_string(), shown, "{input}");
        }
    }

    #[test]
    fn offset_form_matches_the_wire_form_but_for_the_offset() {
        for (input, shown) in [
            ("2026-11-16T19:00:00Z", "2026-11-16T19:00:00+00:00"),
            (
                "2026-11-16T21:00:00.12+02:00",
                "2026-11-16T19:00:00.120+00:00",
            ),
            (
                "2026-11-16T19:00:00.000001Z",
                "2026-11-16T19:00:00.000001+00:00",
            ),
            (
                "2026-11-16T19:00:00.000000001Z",
                "2026-11-16T19:00:00.000000001+00:00",
            ),
        ] {
            assert_eq!(at(input).to_rfc3339_offset(), shown, "{input}");
        }
    }

    #[test]
    fn arithmetic_and_seconds() {
        let start = at("2026-11-16T19:00:00Z");
        let later = start + SignedDuration::from_mins(90);
        assert_eq!(later, at("2026-11-16T20:30:00Z"));
        assert_eq!(later - start, SignedDuration::from_mins(90));
        assert_eq!(start - later, SignedDuration::from_mins(-90));
        assert_eq!(later - SignedDuration::from_mins(90), start);
        assert_eq!(Timestamp::from_second(start.as_second()), Some(start));
        assert_eq!(Timestamp::UNIX_EPOCH, Timestamp::default());
        assert_eq!(Timestamp::from_second(i64::MAX), None);
    }
}
