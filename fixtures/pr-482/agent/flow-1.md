# flow f1
file p15: src/mempool/mod.rs
file p16: src/wallet/mod.rs

## introduction
problem_before: A wallet payment or a mempool broadcast calls RpcClient::send. If the node answers late or not at all, one lost reply fails the whole call with Timeout and the wallet UI shows “offline”.
change: send tries up to 3 times with exponential backoff (200 ms, doubling, capped at 4 s, plus jitter), then gives up with the new Exhausted error.
example_before_after: Before: send() → Err(Timeout) after one attempt. After: send() → attempt 1 times out → wait about 200 ms → attempt 2 → Ok.

## step s1 p15:head:126-132 context
title: Trigger: callers of send
symbol: mempool::broadcast_tx
called_by: a user action in the wallet or the relay (outside the diff)
decides: Nothing here; it builds the request and calls client.send.
next: step 2
why: Shows who reaches the retry loop, in particular a call that is not safe to repeat.

### annotation consequence fact 130-130
text: broadcast_tx now reaches a retrying send.
evidence: p15:head:129 p15:head:130 p6:head:99

### annotation question inference 129-130
text: Not visible in this repository: does the node reject a repeated broadcast_tx for the same transaction, so that a second send is harmless?
evidence: p15:head:129

### assessment
headline: A caller may see a repeated request after a timeout.
risk: low
reason: This context step submits one request; repetition is introduced by the downstream retry decision.
basis_note: Read src/mempool/mod.rs and the flow's caller context; nothing executed.
not_checked: Downstream handling of repeated requests.
origin: introduced
blocking: no_action_proposed

## step s2 p6:head:88-96 modified
title: Attempt: build the request and send once
symbol: RpcClient::send
called_by: step 1
decides: Counts the attempt; rebuilds the request on every turn.
next: step 3
why: The counter feeds the retry decision; the request is rebuilt because execute consumes it.

### annotation behaviour fact 91-95
was: Built the request and sent it once.
is: Builds and sends once per loop turn; the turn count is kept in attempt.
evidence: p6:head:91 p6:head:94 p6:head:95

### annotation consequence inference 93-94
text: Every attempt gets the full per-attempt timeout, so the worst case is 3 × timeout plus delays.

### assessment
headline: A rebuilt request may carry different data on a later attempt.
risk: low
reason: The attempt counter and request construction are local; the retry decision controls whether another request is sent.
basis_note: Read src/rpc/client.rs and the flow's caller context; nothing executed.
not_checked: Request equality across attempts.
origin: introduced
blocking: no_action_proposed

## step s3 p6:head:97-97 modified
title: Outcome of the attempt?
symbol: RpcClient::send
called_by: step 2
decides: Branches on the result: Ok, Timeout, other error.
next: step 4 (Ok), step 5 (Timeout), step 8 (other error)
why: Only timeouts are retried; every other error keeps the old behaviour.

### annotation behaviour fact 97-97
was: execute(...).await? returned every error to the caller.
is: match on the result with three outcomes.
evidence: p6:head:97

### assessment
headline: Callers receive the result selected by this branch.
risk: low
reason: Only timeouts take the new path; success and other errors return directly.
basis_note: Read src/rpc/client.rs and the flow's caller context; nothing executed.
not_checked: Transport error classification at runtime.
origin: introduced
blocking: no_action_proposed

## step s4 p6:head:98-98 modified
title: Return the response
symbol: RpcClient::send
called_by: step 3 (Ok)
decides: Nothing; ends the call.
next: back to the caller
why: A successful attempt ends the loop at once.

### annotation behaviour fact 98-98
was: Returned the response.
is: Returns the response from any attempt.
evidence: p6:head:98

### assessment
headline: The caller receives the successful response.
risk: low
reason: This unchanged return exits the loop immediately without another send.
basis_note: Read src/rpc/client.rs and the flow's caller context; nothing executed.
not_checked: Runtime response decoding.
origin: introduced
blocking: no_action_proposed

## step s5 p6:head:99-99 added
title: Retry allowed?
symbol: RpcClient::send
called_by: step 3 (Timeout)
decides: Asks RetryPolicy::should_retry(attempt); yes goes to step 6, no goes to step 7.
next: step 6 or step 7
why: The loop needs a rule for when to stop; the rule is a plain attempt limit.

### annotation behaviour fact 99-99
was: No retry decision; the timeout went straight to the caller.
is: Retries while attempt < max_attempts (3). The method of the request is not an input.
evidence: p6:head:99 p9:head:23 p9:head:24

### annotation consequence inference 99-99
text: mempool::broadcast_tx may submit the same transaction twice if attempt 1 reached the node and only the reply was late.
evidence: p15:head:129

### annotation question inference 99-99
text: should_retry receives only the attempt number and no method or idempotency input: can broadcast_tx be sent twice when the reply arrives late?
evidence: p9:head:23

### assessment
headline: A wallet user could see one payment broadcast twice after a slow node reply; an operator would see duplicate-transaction rejections in the node log.
risk: high
reason: A repeated value-moving request at system scale (wallet and relay both call send). It needs a reply delayed past 5 s, which flow 2 makes more likely.
basis_note: read 4 files (client.rs, retry.rs, mempool/mod.rs, wallet/pay.rs); nothing executed
not_checked: node-side deduplication (not in this repository)
origin: introduced
blocking: blocks_this_pr
smallest_correction: Pass an idempotency input into should_retry (or skip retry for broadcast_tx); add a test with a transport that delivers the request and then times out.

## step s6 p6:head:100-103 added
title: Wait, then try again
symbol: RpcClient::send
called_by: step 5 (yes)
decides: Computes the delay and sleeps.
next: step 2 (loop)
why: Spaces the attempts so a briefly overloaded node can recover.

### annotation behaviour fact 100-102
was: No delay.
is: Sleeps backoff(attempt): 200 ms doubling, capped at 4 s, plus jitter; then loops to step 2.
evidence: p6:head:100 p6:head:102 p9:head:29 p9:head:30

### annotation question inference 100-100
text: With jitter applied after the cap, can one sleep exceed max_delay (4 s)?
evidence: p9:head:30

### assessment
headline: With the default policy the two waits stay at most 300 ms and 600 ms; a policy with more attempts could wait up to half longer than max_delay.
risk: low
reason: Latent: the 4 s cap is out of reach while every client uses the default of 3 attempts, and no setter can change it.
basis_note: read 2 files (retry.rs, client.rs); nothing executed
not_checked: the jitter range at run time
origin: introduced
blocking: follow_up
smallest_correction: Apply the jitter before min(max_delay).

## step s7 p6:head:104-106 added
title: Give up: Exhausted
symbol: RpcClient::send
called_by: step 5 (no)
decides: Returns the new error instead of Timeout.
next: back to the caller
why: Callers need to tell a retried failure from a single timeout.

### annotation behaviour fact 104-105
was: Returned Err(Timeout).
is: Returns Err(Exhausted { attempts }) once the policy refuses another attempt.
evidence: p6:head:104 p6:head:105 p7:head:13

### annotation consequence fact 105-105
text: Callers that matched on Timeout no longer see it from send; wallet::fetch_balance maps only Timeout to WalletError::Offline.
evidence: p16:head:120 p16:head:121

### annotation question inference 104-106
text: Two wildcard sites were found (flow 3): do all callers of send still handle the case that used to be Timeout?

### assessment
headline: Wallet users get a generic RPC error instead of the “offline” message once retries run out.
risk: medium
reason: User-visible message change on a common failure; no data is affected.
basis_note: read 3 files; nothing executed
not_checked: callers in other repositories
origin: introduced
blocking: follow_up
smallest_correction: Map Exhausted to the same wallet error as Timeout.

## step s8 p6:head:107-109 modified
title: Other errors returned as before
symbol: RpcClient::send
called_by: step 3 (other error)
decides: Nothing; ends the call.
next: back to the caller
why: Only timeouts are worth another attempt.

### annotation behaviour fact 107-107
was: Returned any error.
is: Returns any non-timeout error at once.
evidence: p6:head:107

### assessment
headline: The caller receives a transport error without another attempt.
risk: low
reason: The existing error return remains separate from the timeout retry path.
basis_note: Read src/rpc/client.rs and the flow's caller context; nothing executed.
not_checked: Other transport error variants at runtime.
origin: introduced
blocking: no_action_proposed

## step s9 p6:head:30-43 modified
title: Client holds a retry policy
symbol: RpcClient::new
called_by: every caller that builds an RpcClient
decides: Stores RetryPolicy::default() in the new retry field.
next: step 2 reads it as policy
why: send needs a policy; new gives every client the default of 3 attempts.

### annotation behaviour fact 35-42
was: RpcClient had no retry settings.
is: Every client carries a RetryPolicy, set to the default in new.
evidence: p6:head:35 p6:head:42

### annotation question inference 42-42
text: No setter or constructor argument takes a policy: how does a caller that must not retry change it?
evidence: p6:head:42

### assessment
headline: New clients use the default retry limits.
risk: low
reason: Policy construction is bounded to client setup; retry exposure is assessed at the decision step.
basis_note: Read src/rpc/client.rs and the flow's caller context; nothing executed.
not_checked: Configuration in downstream clients.
origin: introduced
blocking: no_action_proposed

## step s10 p9:head:3-32 added
title: Retry policy: attempt limit and delays
symbol: RetryPolicy
called_by: step 5 (should_retry) and step 6 (backoff)
decides: Allows attempts while attempt < max_attempts; computes the delay before the next attempt.
next: back to step 5 or step 6
why: The loop needs a rule for when to stop and how long to wait; both live in one policy value.

### annotation behaviour fact 21-25
was: No retry rule; util::backoff had only the MAX_ATTEMPTS constant.
is: should_retry allows another attempt while attempt < max_attempts (3).
evidence: p9:head:23 p9:head:24 p10:base:6

### annotation consequence fact 28-30
text: Jitter is applied after the cap, so one delay can exceed max_delay by up to half; with the default 3 attempts the delays are 200 ms and 400 ms before jitter and never reach the cap.
evidence: p9:head:17 p9:head:30 p9:head:46 p9:head:54

### assessment
headline: The policy controls how long the caller waits.
risk: low
reason: The default attempt limit bounds the loop; the send and jitter steps carry the concrete concerns.
basis_note: Read src/rpc/retry.rs and the flow's caller context; nothing executed.
not_checked: Runtime delays and custom policy use.
origin: introduced
blocking: no_action_proposed

## edge s1 s2

## edge s2 s3

## edge s3 s4
label: Ok

## edge s3 s5
label: Timeout

## edge s3 s8
label: other error

## edge s5 s6
label: yes

## edge s5 s7
label: no

## edge s6 s2
label: loop

## edge s9 s2
label: policy

## edge s5 s10
label: should_retry

## edge s6 s10
label: backoff

## thread t1 still_valid
reason: should_retry still retries every method, broadcast_tx included.

## thread t2 stale
reason: The line now calls with_jitter; the random draw it refers to lives in jitter_fraction, moved in flow 5.
