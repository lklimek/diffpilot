# flow f4

## introduction
problem_before: A caller that must not retry used send_once(), next to send().
change: send_once() is removed; send() is the only entry point and always retries. Side effect: no way is left to send without retry.
example_before_after: Before: send() and send_once(). After: only send(), which always retries.

## step s1 p6:base:52-61 deleted_site
title: send_once() removed
symbol: RpcClient::send_once
called_by: callers that must not retry (none left in this repository)
decides: Nothing; the code is gone.
next: callers now use send()
why: One entry point instead of two.

### annotation consequence fact 52-53
text: No way is left to send a request exactly once (see flow 1, step 5).
evidence: p6:base:52 p6:base:53

### annotation flag inference 53-53
level: red
text: Potential build break in acme/node-sdk:
scenario: <<END
acme/node-sdk (PR #118) calls send_once().
This pull request removes send_once().
The SDK no longer builds when it updates to this version.
END
not_checked: The SDK's current code was not read.

### annotation flag inference 52-53
level: yellow
text: Potential repeat of a request that must run once:
scenario: <<END
A caller used send_once() for a request that must run at most once.
send_once() is removed, and send() is the only call left.
send() retries on timeout, so the request may run twice.
END
not_checked: Downstream use was not searched.

### assessment
headline: A downstream developer gets a compile error; a user of broadcast_tx loses the only no-retry path.
risk: medium
reason: Public API removal with a known downstream user; ties to the double-send question.
basis_note: Read: 2 files, searched this repository and the stacked PR title
not_checked: other downstream crates
origin: introduced
blocking: blocks_this_pr
smallest_correction: Keep send_once (or add a per-call retry opt-out) until the SDK has moved.
