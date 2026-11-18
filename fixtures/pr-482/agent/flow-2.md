# flow f2
file p15: src/sync/mod.rs

## introduction
problem_before: Every RPC call built from RpcConfig::default() waits up to 30 s; the wallet UI froze for 30 s on an unreachable peer (issue #470).
change: Every attempt now waits at most 5 s.
example_before_after: Before: timeout = Duration::from_secs(30). After: timeout = Duration::from_secs(5), for each of up to 3 attempts.

## step s1 p4:head:12-26 modified
title: Default timeout 30 s → 5 s
symbol: RpcConfig::default
called_by: RpcClient::new (src/rpc/client.rs:40)
decides: Sets the default per-attempt timeout.
next: every send call
why: The wallet UI froze for 30 s on an unreachable peer (issue #470).

### annotation behaviour fact 23-23
was: timeout = 30 s.
is: timeout = 5 s, for each of up to 3 attempts.
evidence: p4:head:23

### annotation consequence fact 23-23
text: sync::fetch_headers relied on the longer wait; CI: sync_headers::slow_peer_2000_headers fails with a timeout after 5 s.
evidence: p15:head:63 p14:head:11

### annotation flag inference 23-23
level: red
text: Potential header sync timeout on slow peers:
scenario: <<END
A node syncs from a slow peer and requests 2000 headers.
The reply takes 10 to 20 s, as the comment beside the setting says.
The new 5 s timeout ends each attempt first, and every retry ends the same way.
Header sync stalls; the failing CI test is about this path.
END
not_checked: The CI test was not run here; peer timings were not measured.
evidence: p15:head:63

### assessment
headline: Header sync from slow peers may fail where it used to finish; an operator would see sync stall on slow links.
risk: medium
reason: Affects node sync on slow links, not wallet funds; CI already shows a failing test for it.
basis_note: Read: 3 files (config.rs, sync/mod.rs, the CI log)
not_checked: real peer latencies
origin: introduced
blocking: blocks_this_pr
smallest_correction: Keep a longer timeout for get_headers or pass a per-request timeout in fetch_headers.

## step s2 p15:head:61-67 context
title: Affected code: sync::fetch_headers
symbol: sync::fetch_headers
called_by: the sync loop (src/sync/mod.rs:41)
decides: Requests up to 2000 headers in one reply.
next: decodes the headers
why: Shown because the default timeout reaches it, although the file is not in the diff.

### annotation consequence fact 64-65
text: A reply that needs 10-20 s now times out on every attempt.
evidence: p15:head:63 p15:head:65

### annotation note inference 64-64
text: Same risk as the red flag on step 1: a 2000-header reply from a slow peer may take longer than 5 s.

### assessment
risk: low
reason: This caller requests a large batch; the timeout change that reaches it is assessed in the preceding step.

## impact s1 p14:head:8-14
symbol: slow_peer_2000_headers
reason: The failing CI test: its peer answers after 12 s, longer than the new 5 s timeout.
basis: fact
