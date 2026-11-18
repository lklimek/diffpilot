# flow f3
file p15: src/wallet/mod.rs
file p16: src/sync/peers.rs

## introduction
problem_before: send() could only fail with Timeout or Transport, so callers could not tell “retried and still failed” from one timeout.
change: A third variant, Exhausted { attempts }, carries the attempt count; send() returns it after the last attempt (flow 1, step 7).
example_before_after: Before: a match sees Timeout or Transport. After: a match also sees Exhausted { attempts }; wildcard arms swallow it.

## step s1 p7:head:7-16 added
title: New error variant RpcError::Exhausted
symbol: RpcError
called_by: RpcClient::send (step 7 of flow 1)
decides: Adds a variant that carries the attempt count.
next: every match on RpcError
why: Callers need to tell “retried and failed” from one timeout.

### annotation behaviour fact 12-13
was: Two failure variants.
is: Three; wildcard arms treat the new one as a generic failure.
evidence: p7:head:12 p7:head:13

### annotation consequence fact 13-13
text: Two wildcard match sites outside the diff swallow it (wallet and peers).
evidence: p15:head:121 p16:head:204

### annotation question inference 13-13
text: Should the two wildcard sites handle Exhausted explicitly?

### assessment
headline: A wallet user sees a generic RPC error; a peer that keeps timing out is ignored instead of being marked failed.
risk: medium
reason: Wrong handling of a common failure in two places; no data loss.
basis_note: Read: 3 files and searched the repository for matches on send results
not_checked: matches in other repositories
origin: introduced
blocking: follow_up
smallest_correction: Handle Exhausted explicitly in both match sites.

## step s2 p15:head:118-122 context
title: Affected code: wildcard match on send()
symbol: wallet::fetch_balance
called_by: the wallet balance view
decides: Maps any other error to WalletError::Rpc.
next: returns to the wallet UI
why: Same pattern × 2; the second site is src/sync/peers.rs:204.

### annotation consequence fact 121-121
text: The user message differs from the Timeout case.
evidence: p15:head:120 p15:head:121

### annotation question inference 121-121
text: Should Exhausted map to WalletError::Offline like Timeout?

### assessment
risk: low
reason: This existing wildcard branch receives the new variant; its changed exposure is assessed at the variant definition.

## impact s1 p16:head:201-205
symbol: Prober::probe
reason: Err(_) => {} drops Exhausted: a peer that keeps timing out is never marked failed.
basis: fact

## repetition s2
description: A wildcard Err(_) arm after explicit RpcError arms swallows Exhausted.
sites: p16:head:201-205
