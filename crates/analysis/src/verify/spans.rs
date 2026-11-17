//! Inclusive line spans `(first, last)` and the interval operations verification and coverage use, so every
//! pass works on ranges, never on single lines of a large hunk.

use contracts::common::LineRange;

use crate::diff::Hunk;

/// Lines `first..=last`.
pub(crate) type Span = (u32, u32);

/// Sorted, merged spans (overlapping and adjacent spans joined; inverted spans dropped).
pub(crate) fn merge(mut spans: Vec<Span>) -> Vec<Span> {
    spans.retain(|&(first, last)| first <= last);
    spans.sort_unstable();
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for (first, last) in spans {
        match out.last_mut() {
            Some(previous) if u64::from(first) <= u64::from(previous.1) + 1 => {
                previous.1 = previous.1.max(last);
            }
            _ => out.push((first, last)),
        }
    }
    out
}

/// `a` without `b`; both sorted and merged. Each span of `a` finds its first cut by binary search, so the work is
/// O(|a| log |b| + cuts) and a short `a` never scans a long `b`.
pub(crate) fn subtract(a: &[Span], b: &[Span]) -> Vec<Span> {
    let mut out = Vec::new();
    let mut next = 0;
    for &(first, last) in a {
        next += b[next..].partition_point(|&(_, cut_last)| cut_last < first);
        let mut start = u64::from(first);
        let mut index = next;
        while index < b.len() && b[index].0 <= last && start <= u64::from(last) {
            let (cut_first, cut_last) = b[index];
            if u64::from(cut_first) > start {
                out.push((start_u32(start), cut_first - 1));
            }
            start = start.max(u64::from(cut_last) + 1);
            index += 1;
        }
        if start <= u64::from(last) {
            out.push((start_u32(start), last));
        }
    }
    out
}

fn start_u32(start: u64) -> u32 {
    u32::try_from(start).unwrap_or(u32::MAX)
}

/// `a` within `b`; both sorted and merged. Walks the shorter list and finds its first overlap in the longer one
/// by binary search: O(min log max + output), so one span against a long list costs a search, not a scan.
pub(crate) fn intersect(a: &[Span], b: &[Span]) -> Vec<Span> {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let mut out = Vec::new();
    let mut next = 0;
    for &(first, last) in short {
        next += long[next..].partition_point(|&(_, end)| end < first);
        for &(start, end) in long[next..].iter().take_while(|&&(start, _)| start <= last) {
            out.push((start.max(first), end.min(last)));
        }
    }
    out
}

/// The spans of the sorted, merged `spans` sharing a line with `span`: two binary searches.
pub(crate) fn overlapping(spans: &[Span], (first, last): Span) -> &[Span] {
    let from = spans.partition_point(|&(_, end)| end < first);
    let to = from + spans[from..].partition_point(|&(start, _)| start <= last);
    &spans[from..to]
}

/// The lines of `span` outside the sorted, merged `spans`, lazily and in order: a binary search, then one step
/// per gap taken.
pub(crate) fn gaps(spans: &[Span], (first, last): Span) -> impl Iterator<Item = Span> + '_ {
    let mut inside = overlapping(spans, (first, last)).iter();
    let mut start = Some(u64::from(first));
    std::iter::from_fn(move || {
        while let Some(from) = start.filter(|&from| from <= u64::from(last)) {
            match inside.next() {
                Some(&(cut_first, cut_last)) => {
                    start = Some(u64::from(cut_last) + 1);
                    if u64::from(cut_first) > from {
                        return Some((start_u32(from), cut_first - 1));
                    }
                }
                None => {
                    start = None;
                    return Some((start_u32(from), last));
                }
            }
        }
        None
    })
}

/// Whether `span` shares a line with the sorted, merged `spans`.
pub(crate) fn touches(spans: &[Span], (first, last): Span) -> bool {
    let index = spans.partition_point(|&(_, end)| end < first);
    spans.get(index).is_some_and(|&(start, _)| start <= last)
}

/// Spans covered by at least two of `layers` (each layer sorted and merged), in order.
pub(crate) fn covered_twice(layers: &[Vec<Span>]) -> Vec<Span> {
    let mut events: Vec<(u64, i32)> = Vec::new();
    for &(first, last) in layers.iter().flatten() {
        events.push((u64::from(first), 1));
        events.push((u64::from(last) + 1, -1));
    }
    events.sort_unstable();
    let mut out = Vec::new();
    let mut depth = 0;
    let mut opened = 0u64;
    for (position, delta) in events {
        let before = depth;
        depth += delta;
        if before < 2 && depth >= 2 {
            opened = position;
        } else if before >= 2 && depth < 2 && position > opened {
            out.push((start_u32(opened), start_u32(position - 1)));
        }
    }
    merge(out)
}

/// The changed lines of a hunk on one side as spans: added head lines or removed base lines.
pub(crate) fn hunk_spans(hunk: &Hunk, head: bool) -> Vec<Span> {
    let lines = if head { &hunk.added } else { &hunk.removed };
    crate::lines::ranges(lines)
        .into_iter()
        .map(|range| (range.start_line, range.end_line))
        .collect()
}

/// The head line after which a hunk's removed lines are shown (0 = before line 1).
pub(crate) fn marker_of(hunk: &Hunk) -> u32 {
    if hunk.head.len == 0 {
        hunk.head.start
    } else {
        hunk.head.start.saturating_sub(1)
    }
}

/// Whether a deletion marker after head line `marker` lies inside head lines `first..=last` (the code view shows
/// it there: `lines::deletion_markers`).
pub(crate) fn marker_inside(marker: u32, (first, last): Span) -> bool {
    first <= marker.saturating_add(1) && marker <= last
}

/// Spans of `LineRange`s.
pub(crate) fn from_ranges(ranges: &[LineRange]) -> Vec<Span> {
    merge(ranges.iter().map(|r| (r.start_line, r.end_line)).collect())
}

/// `LineRange`s of spans.
pub(crate) fn to_ranges(spans: &[Span]) -> Vec<LineRange> {
    spans
        .iter()
        .map(|&(start_line, end_line)| LineRange {
            start_line,
            end_line,
        })
        .collect()
}

/// `a-b, c` for the first `limit` spans, then ` and more` when there are others: a message stays short however
/// many spans there are.
pub(crate) fn describe_first(spans: impl IntoIterator<Item = Span>, limit: usize) -> String {
    let mut spans = spans.into_iter();
    let shown: Vec<Span> = spans.by_ref().take(limit).collect();
    let more = if spans.next().is_some() {
        " and more"
    } else {
        ""
    };
    format!("{}{more}", describe(&shown))
}

/// `a-b, c` for spans.
pub(crate) fn describe(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|&(first, last)| {
            if first == last {
                first.to_string()
            } else {
                format!("{first}-{last}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;

    #[test]
    fn interval_operations() {
        assert_eq!(
            merge(vec![(5, 6), (1, 2), (3, 3), (9, 8), (6, 7)]),
            [(1, 3), (5, 7)]
        );
        assert_eq!(
            subtract(&[(1, 10), (20, 30)], &[(0, 2), (5, 6), (10, 22)]),
            [(3, 4), (7, 9), (23, 30)]
        );
        assert_eq!(subtract(&[(1, u32::MAX)], &[(1, u32::MAX)]), []);
        assert_eq!(
            intersect(&[(1, 10), (20, 30)], &[(5, 25)]),
            [(5, 10), (20, 25)]
        );
        assert!(touches(&[(1, 3), (8, 9)], (3, 5)) && !touches(&[(1, 3), (8, 9)], (4, 7)));
        assert_eq!(
            covered_twice(&[vec![(1, 10)], vec![(5, 6), (9, 12)], vec![(6, 9)]]),
            [(5, 10)]
        );
        assert!(
            marker_inside(9, (10, 12))
                && marker_inside(12, (10, 12))
                && !marker_inside(9, (11, 12))
        );
        assert_eq!(describe(&[(1, 1), (3, 5)]), "1, 3-5");
        assert_eq!(
            describe_first([(1, 1), (3, 5), (7, 7)], 2),
            "1, 3-5 and more"
        );
        assert_eq!(describe_first([(1, 1)], 2), "1");
        assert_eq!(
            overlapping(&[(1, 3), (5, 6), (8, 9)], (3, 7)),
            [(1, 3), (5, 6)]
        );
        assert_eq!(
            gaps(&[(1, 3), (5, 6), (8, 9)], (2, 12)).collect::<Vec<_>>(),
            [(4, 4), (7, 7), (10, 12)]
        );
        assert_eq!(gaps(&[(0, u32::MAX)], (5, 9)).next(), None);
    }

    /// Up to eight spans within lines 0-40.
    fn span_list() -> impl Strategy<Value = Vec<Span>> {
        proptest::collection::vec((0u32..40, 0u32..6), 0..8)
            .prop_map(|spans| merge(spans.into_iter().map(|(at, len)| (at, at + len)).collect()))
    }

    fn lines(spans: &[Span]) -> BTreeSet<u32> {
        spans
            .iter()
            .flat_map(|&(first, last)| first..=last)
            .collect()
    }

    proptest! {
        /// The binary-searching operations agree with plain set arithmetic and return sorted, merged-able spans.
        #[test]
        fn proptest_span_operations_match_sets(a in span_list(), b in span_list(), span in (0u32..40, 0u32..10)) {
            let span = (span.0, span.0 + span.1);
            let (set_a, set_b) = (lines(&a), lines(&b));
            let within = intersect(&a, &b);
            prop_assert_eq!(lines(&within), &set_a & &set_b);
            prop_assert_eq!(&within, &intersect(&b, &a));
            prop_assert!(within.windows(2).all(|pair| pair[0].1 < pair[1].0));
            let without = subtract(&a, &b);
            prop_assert_eq!(lines(&without), &set_a - &set_b);
            prop_assert!(without.windows(2).all(|pair| pair[0].1 < pair[1].0));
            let gaps: Vec<Span> = gaps(&b, span).collect();
            prop_assert_eq!(&gaps, &subtract(&[span], &b));
            let near = overlapping(&b, span);
            prop_assert_eq!(near.len(), b.iter().filter(|&&s| intersect(&[s], &[span]).len() == 1).count());
        }
    }
}
