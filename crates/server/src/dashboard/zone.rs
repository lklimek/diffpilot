//! IANA time zones of the dashboard (settings, schedule, chart window, daily judge cap) from jiff's bundled
//! database, so zone names and rules never depend on the host's tzdata (ADR-020).

use std::sync::LazyLock;

use contracts::time::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::{AmbiguousOffset, TimeZone, TimeZoneDatabase};

/// The bundled IANA database.
static ZONES: LazyLock<TimeZoneDatabase> = LazyLock::new(TimeZoneDatabase::bundled);

/// The zone named exactly `name` (IANA, case-sensitive, links included); `None` for any other text.
pub fn parse(name: &str) -> Option<TimeZone> {
    let zone = ZONES.get(name).ok()?;
    (!zone.is_unknown() && zone.iana_name() == Some(name)).then_some(zone)
}

/// The first instant local time `local` names in `zone`: the earlier one in an overlap, `None` in a gap.
pub fn earliest(local: DateTime, zone: &TimeZone) -> Option<Timestamp> {
    let at = |offset: jiff::tz::Offset| offset.to_timestamp(local).ok();
    match zone.to_ambiguous_timestamp(local).offset() {
        AmbiguousOffset::Unambiguous { offset } => at(offset),
        AmbiguousOffset::Gap { .. } => None,
        AmbiguousOffset::Fold { before, after } => {
            [at(before), at(after)].into_iter().flatten().min()
        }
    }
    .map(Timestamp::from)
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::*;

    fn utc(text: &str) -> Option<Timestamp> {
        Some(text.parse().expect("time"))
    }

    #[test]
    fn names_are_exact_iana_names() {
        for name in ["UTC", "Europe/Warsaw", "Etc/UTC", "US/Pacific"] {
            assert_eq!(
                parse(name).and_then(|z| z.iana_name().map(str::to_owned)),
                Some(name.to_owned())
            );
        }
        for name in ["utc", "Europe/warsaw", "Etc/Unknown", "", "/etc/localtime"] {
            assert!(parse(name).is_none(), "{name:?}");
        }
    }

    #[test]
    fn earliest_is_none_in_a_gap_and_the_first_occurrence_in_an_overlap() {
        let warsaw = parse("Europe/Warsaw").expect("zone");
        assert_eq!(
            earliest(date(2026, 3, 29).at(2, 30, 0, 0), &warsaw),
            None,
            "spring forward"
        );
        assert_eq!(
            earliest(date(2026, 10, 25).at(2, 30, 0, 0), &warsaw),
            utc("2026-10-25T00:30:00Z"),
            "fall back: summer time first"
        );
        assert_eq!(
            earliest(date(2026, 11, 18).at(6, 0, 0, 0), &warsaw),
            utc("2026-11-18T05:00:00Z")
        );
    }
}
