# flow f5

## introduction
problem_before: RetryPolicy::backoff (flow 1) calls delay helpers that live in the generic util/backoff.rs.
change: The helpers move next to the policy, in rpc/retry.rs; 56 of 58 lines move as they were and 2 lines differ.
example_before_after: Before: JITTER is a tenth, so a delay moves by up to a tenth either way. After: JITTER is a half, so a delay moves by up to half either way.

## step s1 p9:head:38-95 moved
title: Backoff helpers moved
symbol: exp_delay, with_jitter, jitter_fraction
called_by: RetryPolicy::backoff (flow 1)
decides: Computes the exponential delay and the jitter.
next: returns the delay to backoff
why: Keeps the helpers next to the policy that uses them.
moved_from: p10:base:11-68

### annotation behaviour fact 49-50
was: JITTER is a tenth: a delay moves by up to a tenth either way.
is: JITTER is a half: a delay moves by up to half either way.
evidence: p9:head:50 p10:base:23

### annotation question inference 49-50
text: Is the jump from a tenth to a half of the delay intended in a PR that is described as a move?

### assessment
headline: Retries wait noticeably longer or shorter from run to run; few users would see it.
risk: low
reason: A tuning change inside a bounded delay.
basis_note: read 2 files (old and new location); nothing executed
not_checked: how often retries happen in production
origin: introduced
blocking: follow_up
smallest_correction: Say it in the PR description, or keep the tenth in the move.

## step s2 p5:head:8-20 modified
title: Module tree: retry in, backoff out
called_by: the crate root
decides: Declares rpc::retry and drops util::backoff.
next: step 1 (the helpers in their new place)
why: The moved file needs its new module path.

### annotation behaviour fact 13-13
was: util::backoff was a module of util.
is: rpc::retry is a module of rpc; util::backoff is gone.
evidence: p5:head:13 p5:base:18

### assessment
headline: Consumers may need the new module path.
risk: low
reason: The module declarations wire the moved implementation; there is no runtime branch here.
basis_note: Read src/lib.rs and the flow's caller context; nothing executed.
not_checked: External imports of the old module path.
origin: introduced
blocking: no_action_proposed

## edge s2 s1
